use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaChatMessage, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use tokio::sync::{OnceCell, mpsc};
use tokio_util::sync::CancellationToken;

use crate::session::{Message, Role};

use super::{Backend, BackendError, InferOptions, StreamEvent};

struct InferenceJob {
    messages: Vec<Message>,
    cancel: CancellationToken,
    tx: mpsc::Sender<StreamEvent>,
}

// Held by LocalBackend — a sender to the persistent inference thread.
struct ModelHandle {
    job_tx: std::sync::mpsc::Sender<InferenceJob>,
}

// Tracks the system-prompt portion already decoded into the KV cache.
struct PrefixCache {
    system_key: String, // hash of the system message content
    prefix_len: i32,    // token count of the cached prefix
}

pub struct LocalBackend {
    name: String,
    hf_repo: String,
    hf_file: String,
    cw: usize,
    no_download: bool,
    handle: OnceCell<Arc<ModelHandle>>,
}

impl LocalBackend {
    pub fn new(model_name: impl Into<String>, no_download: bool) -> Self {
        let name = model_name.into();
        let (hf_repo, hf_file, cw) = resolve_model(&name);
        Self {
            name,
            hf_repo,
            hf_file,
            cw,
            no_download,
            handle: OnceCell::new(),
        }
    }

    async fn get_handle(&self) -> Result<Arc<ModelHandle>, BackendError> {
        self.handle
            .get_or_try_init(|| async {
                let hf_repo = self.hf_repo.clone();
                let hf_file = self.hf_file.clone();
                let no_download = self.no_download;
                let cw = self.cw;
                tokio::task::spawn_blocking(move || {
                    start_inference_thread(&hf_repo, &hf_file, no_download, cw)
                })
                .await
                .map_err(|e| BackendError::Unavailable(e.to_string()))?
            })
            .await
            .cloned()
    }

    /// Forces model load and context creation before the daemon binds its port.
    pub async fn warm_up(&self) -> Result<(), BackendError> {
        self.get_handle().await.map(|_| ())
    }
}

fn load_model(
    hf_repo: &str,
    hf_file: &str,
    no_download: bool,
) -> Result<(LlamaBackend, LlamaModel), BackendError> {
    let model_path: PathBuf = if hf_file.is_empty() {
        PathBuf::from(hf_repo)
    } else {
        // When no_download=true the CLI has already cached the file; hf_hub will
        // find it in the cache directory without making any network requests.
        let api = hf_hub::api::sync::ApiBuilder::new()
            .build()
            .map_err(|e| BackendError::Unavailable(e.to_string()))?;
        api.model(hf_repo.to_string()).get(hf_file).map_err(|_| {
            if no_download {
                BackendError::ModelNotCached
            } else {
                BackendError::Unavailable(format!("failed to download {hf_file} from {hf_repo}"))
            }
        })?
    };

    let backend = LlamaBackend::init().map_err(|e| BackendError::Unavailable(e.to_string()))?;
    let model = LlamaModel::load_from_file(&backend, &model_path, &LlamaModelParams::default())
        .map_err(|e| BackendError::Unavailable(e.to_string()))?;

    Ok((backend, model))
}

/// Loads the model, creates the KV-cache context once, then loops receiving
/// inference jobs. The context is reused across requests with prefix caching.
///
/// LlamaContext is !Send, so it must be created on the thread that uses it.
fn start_inference_thread(
    hf_repo: &str,
    hf_file: &str,
    no_download: bool,
    cw: usize,
) -> Result<Arc<ModelHandle>, BackendError> {
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), BackendError>>();
    let (job_tx, job_rx) = std::sync::mpsc::channel::<InferenceJob>();

    let hf_repo = hf_repo.to_string();
    let hf_file = hf_file.to_string();

    std::thread::spawn(move || {
        let (backend, model) = match load_model(&hf_repo, &hf_file, no_download) {
            Ok(x) => x,
            Err(e) => {
                let _ = ready_tx.send(Err(e));
                return;
            }
        };

        let ctx_params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(cw as u32))
            .with_n_threads(1)
            .with_n_threads_batch(1);
        let mut ctx = match model.new_context(&backend, ctx_params) {
            Ok(c) => c,
            Err(e) => {
                let _ = ready_tx.send(Err(BackendError::Unavailable(e.to_string())));
                return;
            }
        };

        let template = match model.chat_template(None) {
            Ok(t) => t,
            Err(e) => {
                let _ = ready_tx.send(Err(BackendError::Unavailable(e.to_string())));
                return;
            }
        };

        let _ = ready_tx.send(Ok(()));

        // ctx borrows model; Rust drops ctx before model (reverse declaration order).
        let mut prefix_cache: Option<PrefixCache> = None;
        while let Ok(job) = job_rx.recv() {
            if let Err(e) = run_with_prefix_cache(&template, &mut ctx, &job, cw, &mut prefix_cache)
            {
                eprintln!("axon-daemon: inference error: {e}");
            }
            let _ = job.tx.blocking_send(StreamEvent {
                delta: String::new(),
                done: true,
            });
        }
    });

    ready_rx
        .recv()
        .map_err(|_| BackendError::Unavailable("inference thread exited before ready".into()))
        .and_then(|r| r)?;

    Ok(Arc::new(ModelHandle { job_tx }))
}

#[async_trait]
impl Backend for LocalBackend {
    async fn stream(
        &self,
        messages: &[Message],
        options: &InferOptions,
        cancel: CancellationToken,
        tx: mpsc::Sender<StreamEvent>,
    ) -> Result<(), BackendError> {
        let _ = options;
        let handle = self.get_handle().await?;
        let job = InferenceJob {
            messages: messages.to_vec(),
            cancel,
            tx,
        };
        // Unbounded channel — send never blocks. The daemon's Semaphore(1) ensures
        // only one job is in flight at a time.
        handle
            .job_tx
            .send(job)
            .map_err(|_| BackendError::Unavailable("inference thread closed".into()))
    }

    fn model_name(&self) -> &str {
        &self.name
    }

    fn context_window(&self) -> usize {
        self.cw
    }
}

/// Converts axon messages to llama-cpp-2 chat messages.
fn to_llama_msgs(messages: &[Message]) -> Result<Vec<LlamaChatMessage>, BackendError> {
    messages
        .iter()
        .map(|m| {
            let role = match m.role {
                Role::System => "system",
                Role::User => "user",
                Role::Assistant => "assistant",
            };
            LlamaChatMessage::new(role.to_string(), m.content.clone())
                .map_err(|e| BackendError::Inference(format!("message: {e}")))
        })
        .collect()
}

/// Returns the content of all system messages joined, used as the prefix cache key.
fn system_key(messages: &[Message]) -> String {
    messages
        .iter()
        .filter(|m| m.role == Role::System)
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Computes how many tokens the system-message portion of the chat template occupies.
/// Returns 0 if the template cannot be applied to system messages alone.
fn compute_prefix_len(
    template: &llama_cpp_2::model::LlamaChatTemplate,
    model: &LlamaModel,
    messages: &[Message],
) -> i32 {
    let sys_msgs: Result<Vec<LlamaChatMessage>, _> = messages
        .iter()
        .filter(|m| m.role == Role::System)
        .map(|m| LlamaChatMessage::new("system".to_string(), m.content.clone()))
        .collect();
    let Ok(sys_msgs) = sys_msgs else { return 0 };
    if sys_msgs.is_empty() {
        return 0;
    }
    let Ok(prefix_text) = model.apply_chat_template(template, &sys_msgs, false) else {
        return 0;
    };
    model
        .str_to_token(&prefix_text, AddBos::Always)
        .map(|t| t.len() as i32)
        .unwrap_or(0)
}

/// Runs one inference job, reusing the KV-cached system prompt when possible.
///
/// Strategy:
///   - If the system prompt hasn't changed since the last request, the prefix
///     (system prompt tokens) is already decoded in the KV cache. We only need
///     to prefill the new user message tokens, cutting prefill time by ~8-10×.
///   - After generation, we trim the KV cache back to just the prefix so the
///     next request can reuse it.
fn run_with_prefix_cache(
    template: &llama_cpp_2::model::LlamaChatTemplate,
    ctx: &mut LlamaContext<'_>,
    job: &InferenceJob,
    cw: usize,
    prefix_cache: &mut Option<PrefixCache>,
) -> Result<(), BackendError> {
    let model = ctx.model;

    let llama_msgs = to_llama_msgs(&job.messages)?;
    let full_prompt = model
        .apply_chat_template(template, &llama_msgs, true)
        .map_err(|e| BackendError::Inference(format!("apply template: {e}")))?;
    let all_tokens = model
        .str_to_token(&full_prompt, AddBos::Always)
        .map_err(|e| BackendError::Inference(e.to_string()))?;
    if all_tokens.is_empty() {
        return Ok(());
    }
    let prompt_len = all_tokens.len() as i32;

    let key = system_key(&job.messages);
    let prefix_len = compute_prefix_len(template, model, &job.messages);

    let can_reuse = prefix_cache
        .as_ref()
        .map(|pc| pc.system_key == key && pc.prefix_len == prefix_len && prefix_len > 0)
        .unwrap_or(false);

    let decode_start = if can_reuse {
        // Invalidate only the tokens after the cached prefix (user + assistant turns).
        let _ = ctx.clear_kv_cache_seq(None, Some(prefix_len as u32), None);
        prefix_len
    } else {
        let _ = ctx.clear_kv_cache_seq(None, None, None);
        0
    };

    // Decode only the new portion of the prompt in chunks to stay within n_batch.
    // A large skill content can easily exceed the default n_batch (512), so we
    // must split the prefill into fixed-size chunks rather than one giant batch.
    const PREFILL_CHUNK: usize = 512;
    let new_tokens = &all_tokens[decode_start as usize..];
    let mut last_batch_pos = 0i32;
    if !new_tokens.is_empty() {
        let last_tok_idx = new_tokens.len() - 1;
        let mut processed = 0usize;
        for chunk in new_tokens.chunks(PREFILL_CHUNK) {
            let mut batch = LlamaBatch::new(chunk.len(), 1);
            for (ci, &token) in chunk.iter().enumerate() {
                let pos = decode_start + (processed + ci) as i32;
                let want_logits = processed + ci == last_tok_idx;
                batch
                    .add(token, pos, &[0], want_logits)
                    .map_err(|e| BackendError::Inference(e.to_string()))?;
                if want_logits {
                    last_batch_pos = ci as i32;
                }
            }
            ctx.decode(&mut batch)
                .map_err(|e| BackendError::Inference(e.to_string()))?;
            processed += chunk.len();
        }
    }

    // Generation loop — one token at a time.
    let max_new = (cw as i32).saturating_sub(prompt_len);
    let mut n_cur = prompt_len;
    let mut sampler = LlamaSampler::greedy();
    let mut decoder = encoding_rs::UTF_8.new_decoder();

    'generation: loop {
        if n_cur >= prompt_len + max_new {
            break;
        }
        if job.cancel.is_cancelled() {
            break;
        }

        let token = sampler.sample(ctx, last_batch_pos);
        sampler.accept(token);

        if model.is_eog_token(token) {
            break 'generation;
        }

        let text = model
            .token_to_piece(token, &mut decoder, true, None)
            .unwrap_or_default();

        if !text.is_empty()
            && job
                .tx
                .blocking_send(StreamEvent {
                    delta: text,
                    done: false,
                })
                .is_err()
        {
            break;
        }

        let mut batch = LlamaBatch::new(1, 1);
        batch
            .add(token, n_cur, &[0], true)
            .map_err(|e| BackendError::Inference(e.to_string()))?;
        n_cur += 1;
        ctx.decode(&mut batch)
            .map_err(|e| BackendError::Inference(e.to_string()))?;
        last_batch_pos = 0;
    }

    // Trim KV cache back to the prefix so the next request can reuse it.
    if prefix_len > 0 {
        let _ = ctx.clear_kv_cache_seq(None, Some(prefix_len as u32), None);
        *prefix_cache = Some(PrefixCache {
            system_key: key,
            prefix_len,
        });
    } else {
        let _ = ctx.clear_kv_cache_seq(None, None, None);
        *prefix_cache = None;
    }

    Ok(())
}

/// Returns just the context window for a named model without loading it.
pub fn resolve_cw(name: &str) -> usize {
    resolve_model(name).2
}

/// Maps short model names to (HuggingFace repo, GGUF filename, context_window).
/// User-registered models from `~/.axon/config.toml` take priority over the built-in table.
/// For local `.gguf` paths, returns (full_path, "", cw).
pub(crate) fn resolve_model(name: &str) -> (String, String, usize) {
    let config = crate::config::AxonConfig::load();
    if let Some(entry) = config.find_model(name) {
        return (
            entry.hf_repo.clone(),
            entry.hf_file.clone(),
            entry.context_window,
        );
    }
    match name {
        "qwen3.5:2b" => (
            "unsloth/Qwen3.5-2B-GGUF".into(),
            "Qwen3.5-2B-Q4_K_M.gguf".into(),
            32768,
        ),
        "qwen3.5:4b-mlx" | "qwen3.5-4b" => (
            "unsloth/Qwen3.5-4B-GGUF".into(),
            "Qwen3.5-4B-Q4_K_M.gguf".into(),
            32768,
        ),
        "qwen3-4b-q4_k_m" | "qwen3:4b" | "qwen3-4b" => (
            "unsloth/Qwen3-4B-GGUF".into(),
            "Qwen3-4B-Q4_K_M.gguf".into(),
            32768,
        ),
        _ => {
            if name.ends_with(".gguf") {
                (name.to_string(), String::new(), 2048)
            } else {
                (name.to_string(), String::new(), 4096)
            }
        }
    }
}

/// Returns the list of built-in model names.
pub fn known_models() -> Vec<&'static str> {
    vec!["qwen3.5:2b", "qwen3.5:4b-mlx", "qwen3:4b"]
}

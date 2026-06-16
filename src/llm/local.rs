use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaChatMessage, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use tokio::sync::{OnceCell, mpsc};
use tokio_util::sync::CancellationToken;

use crate::session::{Message, Role};

use super::{Backend, BackendError, StreamEvent};

struct ModelInner {
    backend: LlamaBackend,
    model: LlamaModel,
}

pub struct LocalBackend {
    name: String,
    hf_repo: String,
    hf_file: String,
    cw: usize,
    no_download: bool,
    inner: OnceCell<Arc<ModelInner>>,
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
            inner: OnceCell::new(),
        }
    }

    async fn get_inner(&self) -> Result<Arc<ModelInner>, BackendError> {
        self.inner
            .get_or_try_init(|| async {
                let hf_repo = self.hf_repo.clone();
                let hf_file = self.hf_file.clone();
                let no_download = self.no_download;
                tokio::task::spawn_blocking(move || {
                    load_model_inner(&hf_repo, &hf_file, no_download)
                })
                .await
                .map_err(|e| BackendError::Unavailable(e.to_string()))?
            })
            .await
            .cloned()
    }

    /// Forces model load before the daemon binds its port (the readiness signal).
    pub async fn warm_up(&self) -> Result<(), BackendError> {
        self.get_inner().await.map(|_| ())
    }
}

fn load_model_inner(
    hf_repo: &str,
    hf_file: &str,
    no_download: bool,
) -> Result<Arc<ModelInner>, BackendError> {
    let model_path: PathBuf = if hf_file.is_empty() {
        // Local .gguf path or bare HF repo ID with no file — use as-is.
        PathBuf::from(hf_repo)
    } else {
        if no_download {
            // SAFETY: daemon warm_up runs once before accepting connections;
            // no other threads are reading/writing env vars at this point.
            unsafe { std::env::set_var("HF_HUB_OFFLINE", "1") };
        }
        let api = hf_hub::api::sync::ApiBuilder::new()
            .with_progress(true)
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

    Ok(Arc::new(ModelInner { backend, model }))
}

#[async_trait]
impl Backend for LocalBackend {
    async fn stream(
        &self,
        messages: &[Message],
        cancel: CancellationToken,
        tx: mpsc::Sender<StreamEvent>,
    ) -> Result<(), BackendError> {
        let inner = self.get_inner().await?;
        let messages = messages.to_vec();
        let cw = self.cw;

        tokio::task::spawn_blocking(move || run_inference(&inner, &messages, cw, &cancel, &tx))
            .await
            .map_err(|e| BackendError::Inference(e.to_string()))?
    }

    fn model_name(&self) -> &str {
        &self.name
    }

    fn context_window(&self) -> usize {
        self.cw
    }
}

fn run_inference(
    inner: &ModelInner,
    messages: &[Message],
    cw: usize,
    cancel: &CancellationToken,
    tx: &mpsc::Sender<StreamEvent>,
) -> Result<(), BackendError> {
    let model = &inner.model;
    let backend = &inner.backend;

    let template = model
        .chat_template(None)
        .map_err(|e| BackendError::Inference(format!("chat template: {e}")))?;

    let llama_msgs: Vec<LlamaChatMessage> = messages
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
        .collect::<Result<_, _>>()?;

    let prompt = model
        .apply_chat_template(&template, &llama_msgs, true)
        .map_err(|e| BackendError::Inference(format!("apply template: {e}")))?;

    let tokens = model
        .str_to_token(&prompt, AddBos::Always)
        .map_err(|e| BackendError::Inference(e.to_string()))?;

    if tokens.is_empty() {
        let _ = tx.blocking_send(StreamEvent {
            delta: String::new(),
            done: true,
        });
        return Ok(());
    }

    let ctx_params = LlamaContextParams::default().with_n_ctx(NonZeroU32::new(cw as u32));
    let mut ctx = model
        .new_context(backend, ctx_params)
        .map_err(|e| BackendError::Inference(e.to_string()))?;

    let prompt_len = tokens.len() as i32;
    let mut batch = LlamaBatch::new(tokens.len().max(512), 1);
    for (i, &token) in (0i32..).zip(tokens.iter()) {
        batch
            .add(token, i, &[0], i == prompt_len - 1)
            .map_err(|e| BackendError::Inference(e.to_string()))?;
    }
    ctx.decode(&mut batch)
        .map_err(|e| BackendError::Inference(e.to_string()))?;

    let max_new = (cw as i32).saturating_sub(prompt_len);
    let mut n_cur = prompt_len;
    let mut sampler = LlamaSampler::greedy();
    let mut decoder = encoding_rs::UTF_8.new_decoder();

    while n_cur < prompt_len + max_new {
        if cancel.is_cancelled() {
            break;
        }

        let token = sampler.sample(&ctx, batch.n_tokens() - 1);
        sampler.accept(token);

        if model.is_eog_token(token) {
            break;
        }

        let text = model
            .token_to_piece(token, &mut decoder, true, None)
            .unwrap_or_default();

        if !text.is_empty()
            && tx
                .blocking_send(StreamEvent {
                    delta: text,
                    done: false,
                })
                .is_err()
        {
            break;
        }

        batch.clear();
        batch
            .add(token, n_cur, &[0], true)
            .map_err(|e| BackendError::Inference(e.to_string()))?;

        n_cur += 1;
        ctx.decode(&mut batch)
            .map_err(|e| BackendError::Inference(e.to_string()))?;
    }

    let _ = tx.blocking_send(StreamEvent {
        delta: String::new(),
        done: true,
    });
    Ok(())
}

/// Returns just the context window for a named model without loading it.
pub fn resolve_cw(name: &str) -> usize {
    resolve_model(name).2
}

/// Maps short model names to (HuggingFace repo, GGUF filename, context_window).
/// For local `.gguf` paths, returns (full_path, "", cw).
pub(crate) fn resolve_model(name: &str) -> (String, String, usize) {
    match name {
        "qwen2.5-coder-1.5b-instruct-q4_k_m" | "qwen2.5-coder:1.5b" | "qwen2.5-coder-1.5b" => (
            "bartowski/Qwen2.5-Coder-1.5B-Instruct-GGUF".into(),
            "Qwen2.5-Coder-1.5B-Instruct-Q4_K_M.gguf".into(),
            4096,
        ),
        "qwen2.5-coder-3b-instruct-q4_k_m" | "qwen2.5-coder:3b" | "qwen2.5-coder-3b" => (
            "bartowski/Qwen2.5-Coder-3B-Instruct-GGUF".into(),
            "Qwen2.5-Coder-3B-Instruct-Q4_K_M.gguf".into(),
            4096,
        ),
        "qwen2.5-coder-7b-instruct-q4_k_m" | "qwen2.5-coder:7b" | "qwen2.5-coder-7b" => (
            "bartowski/Qwen2.5-Coder-7B-Instruct-GGUF".into(),
            "Qwen2.5-Coder-7B-Instruct-Q4_K_M.gguf".into(),
            8192,
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

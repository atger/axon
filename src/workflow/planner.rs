use std::sync::Arc;

use color_eyre::eyre::{Result, eyre};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::parser::WorkflowDef;
use crate::llm::{Backend, InferOptions, StreamEvent};
use crate::session::Message;

#[derive(Debug, Serialize, Deserialize)]
pub struct ExecutionPlan {
    pub name: String,
    pub steps: Vec<Step>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Step {
    Shell {
        id: String,
        name: String,
        cmd: String,
        output_var: String,
    },
    Llm {
        id: String,
        name: String,
        prompt: String,
        output_var: String,
    },
}

impl Step {
    pub fn id(&self) -> &str {
        match self {
            Step::Shell { id, .. } | Step::Llm { id, .. } => id,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Step::Shell { name, .. } | Step::Llm { name, .. } => name,
        }
    }

    pub fn output_var(&self) -> &str {
        match self {
            Step::Shell { output_var, .. } | Step::Llm { output_var, .. } => output_var,
        }
    }

    /// Directory name used in the workspace: `step-01-collect-commits`.
    pub fn dir_name(&self) -> String {
        format!("{}-{}", self.id(), self.name())
    }
}

const COMPILE_SYSTEM: &str = "\
You are a workflow compiler. Convert a natural-language workflow into a precise JSON execution plan. \
Output ONLY valid JSON — no markdown fences, no explanation, no surrounding text.

Available step types:
- shell: run a shell command, capture stdout as output
- llm: call the AI model with a prompt, capture response as output

Tools available on the system: gh (GitHub CLI), jq, python3, curl, git, mail.

Variable references: {{var_name}} in a step is replaced with the shell-quoted path to that variable's output file.
- Pass as a file argument:  jq '...' {{commits}}
- Read content inline:      $(cat {{commits}})
- Pass to a Python script:  python3 fill.py {{summary}} template.docx
In llm prompts, {{var_name}} is replaced with the actual file content before sending.

Step ids must be sequential: step-01, step-02, step-03, ...
Step names must be short kebab-case: collect-commits, group-repos, summarize-work, ...

Output this JSON structure exactly:
{
  \"name\": \"workflow-name\",
  \"steps\": [
    {\"type\": \"shell\", \"id\": \"step-01\", \"name\": \"kebab-name\", \"cmd\": \"shell command\", \"output_var\": \"var_name\"},
    {\"type\": \"llm\",   \"id\": \"step-02\", \"name\": \"kebab-name\", \"prompt\": \"prompt with {{var}} refs\", \"output_var\": \"var_name\"}
  ]
}";

pub async fn compile(backend: &Arc<dyn Backend>, def: &WorkflowDef) -> Result<ExecutionPlan> {
    let user_content = if let Some(desc) = &def.description {
        format!(
            "Compile this workflow to a JSON execution plan.\nDescription: {desc}\n\n{}",
            def.raw_steps
        )
    } else {
        format!(
            "Compile this workflow to a JSON execution plan:\n\n{}",
            def.raw_steps
        )
    };

    let messages = vec![Message::system(COMPILE_SYSTEM), Message::user(user_content)];

    let (tx, mut rx) = mpsc::channel::<StreamEvent>(256);
    let cancel = CancellationToken::new();
    let b = Arc::clone(backend);
    let opts = InferOptions::default();

    tokio::spawn(async move {
        let _ = b.stream(&messages, &opts, cancel, tx).await;
    });

    let mut buf = String::new();
    while let Some(ev) = rx.recv().await {
        if !ev.delta.is_empty() {
            buf.push_str(&ev.delta);
        }
        if ev.done {
            break;
        }
    }

    let json_str = extract_json(&buf);
    serde_json::from_str::<ExecutionPlan>(json_str)
        .map_err(|e| eyre!("failed to parse compiled plan ({e})\nRaw response:\n{buf}"))
}

/// Strip `<think>…</think>` blocks and extract the JSON object from the response.
/// Uses rfind for `}` so that trailing markdown fences are excluded.
fn extract_json(s: &str) -> &str {
    let s = if let (Some(start), Some(end_tag)) = (s.find("<think>"), s.find("</think>")) {
        let end = end_tag + "</think>".len();
        if start == 0 { s[end..].trim_start() } else { s }
    } else {
        s
    };
    let trimmed = s.trim();
    match (trimmed.find('{'), trimmed.rfind('}')) {
        (Some(start), Some(end)) if end >= start => &trimmed[start..=end],
        _ => trimmed,
    }
}

use std::collections::HashMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use color_eyre::eyre::{Result, eyre};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::planner::{ExecutionPlan, Step};
use super::workspace::WorkspaceManager;
use crate::llm::{Backend, InferOptions, StreamEvent};
use crate::session::Message;

pub async fn execute(
    plan: &ExecutionPlan,
    ws: &WorkspaceManager,
    backend: Arc<dyn Backend>,
) -> Result<()> {
    // var_name → path to that step's output.txt
    let mut vars: HashMap<String, PathBuf> = HashMap::new();

    for step in &plan.steps {
        let dir_name = step.dir_name();
        let start = Instant::now();

        // Resumability: skip if a previous run already produced this output.
        if ws.output_exists(&dir_name) {
            vars.insert(step.output_var().to_string(), ws.output_path(&dir_name));
            eprintln!("  (cached)  {}", step.name());
            continue;
        }

        let output = match step {
            Step::Shell { name, cmd, .. } => run_shell(name, cmd, &vars, ws, &dir_name).await?,
            Step::Llm { prompt, .. } => run_llm(prompt, &vars, ws, &dir_name, &backend).await?,
        };

        ws.write_file(&dir_name, "output.txt", &output)?;
        vars.insert(step.output_var().to_string(), ws.output_path(&dir_name));

        eprintln!(
            "  ✓  {}  ({:.1}s)",
            step.name(),
            start.elapsed().as_secs_f64()
        );
    }

    Ok(())
}

async fn run_shell(
    name: &str,
    cmd_template: &str,
    vars: &HashMap<String, PathBuf>,
    ws: &WorkspaceManager,
    dir_name: &str,
) -> Result<String> {
    let resolved = resolve_path_vars(cmd_template, vars);
    ws.write_file(dir_name, "cmd.txt", &resolved)?;
    ws.write_file(dir_name, "input.json", &vars_to_json(vars))?;

    eprint!("  Run {}? [y/N] ", name);
    std::io::stderr().flush().ok();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).unwrap_or(0);
    if !matches!(line.trim().to_lowercase().as_str(), "y" | "yes") {
        return Err(eyre!("step '{name}' not confirmed — aborting"));
    }

    let out = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(&resolved)
        .output()
        .await
        .map_err(|e| eyre!("failed to spawn shell for '{name}': {e}"))?;

    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();

    ws.write_file(
        dir_name,
        "log.txt",
        &format!(
            "exit: {}\nstderr:\n{}",
            out.status.code().unwrap_or(-1),
            stderr.trim_end()
        ),
    )?;

    if !out.status.success() {
        let code = out.status.code().unwrap_or(-1);
        return Err(eyre!(
            "step '{name}' failed (exit {code})\nstderr: {}",
            stderr.trim_end()
        ));
    }

    Ok(stdout)
}

async fn run_llm(
    prompt_template: &str,
    vars: &HashMap<String, PathBuf>,
    ws: &WorkspaceManager,
    dir_name: &str,
    backend: &Arc<dyn Backend>,
) -> Result<String> {
    let resolved = resolve_content_vars(prompt_template, vars)?;
    ws.write_file(dir_name, "prompt.txt", &resolved)?;
    ws.write_file(dir_name, "input.json", &vars_to_json(vars))?;

    let messages = vec![Message::user(resolved)];
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

    let response = strip_think(&buf);
    ws.write_file(dir_name, "log.txt", &format!("~{} tokens", buf.len() / 4))?;

    Ok(response)
}

/// Replace `{{var}}` with the shell-quoted path to that variable's output file.
fn resolve_path_vars(template: &str, vars: &HashMap<String, PathBuf>) -> String {
    let mut result = template.to_string();
    for (k, v) in vars {
        let path = v.to_string_lossy();
        let quoted = format!("'{}'", path.replace('\'', "'\\''"));
        result = result.replace(&format!("{{{{{k}}}}}"), &quoted);
    }
    result
}

/// Replace `{{var}}` with the actual file content for LLM prompts.
fn resolve_content_vars(template: &str, vars: &HashMap<String, PathBuf>) -> Result<String> {
    let mut result = template.to_string();
    for (k, v) in vars {
        let placeholder = format!("{{{{{k}}}}}");
        if result.contains(&placeholder) {
            let content = std::fs::read_to_string(v)
                .map_err(|e| eyre!("failed to read variable '{k}': {e}"))?;
            result = result.replace(&placeholder, &content);
        }
    }
    Ok(result)
}

fn vars_to_json(vars: &HashMap<String, PathBuf>) -> String {
    let map: serde_json::Map<String, serde_json::Value> = vars
        .iter()
        .map(|(k, v)| {
            (
                k.clone(),
                serde_json::Value::String(v.to_string_lossy().into_owned()),
            )
        })
        .collect();
    serde_json::to_string_pretty(&serde_json::Value::Object(map)).unwrap_or_default()
}

fn strip_think(s: &str) -> String {
    if let (Some(start), Some(end_tag)) = (s.find("<think>"), s.find("</think>")) {
        let end = end_tag + "</think>".len();
        if start == 0 {
            return s[end..].trim_start().to_string();
        }
    }
    s.to_string()
}

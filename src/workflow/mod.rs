use std::io::{self, Write as _};
use std::path::Path;
use std::sync::Arc;

use color_eyre::eyre::{Result, eyre};

use crate::llm::Backend;

mod executor;
mod parser;
mod planner;
mod workspace;

pub use planner::{ExecutionPlan, Step};

pub struct WorkflowEngine {
    backend: Arc<dyn Backend>,
}

impl WorkflowEngine {
    pub fn new(backend: Arc<dyn Backend>) -> Self {
        Self { backend }
    }

    pub async fn run_workflow(&self, path: &Path, compile_only: bool) -> Result<()> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| eyre!("failed to read {}: {e}", path.display()))?;
        let def = parser::parse_workflow_md(&raw)?;

        eprintln!("Compiling workflow '{}'...", def.name);
        let plan = planner::compile(&self.backend, &def).await?;
        let plan_json = serde_json::to_string_pretty(&plan)?;

        print_plan(&plan);

        if compile_only {
            println!("\n{plan_json}");
            return Ok(());
        }

        eprint!("\nProceed? [y/N] ");
        io::stderr().flush()?;
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        if !matches!(line.trim().to_lowercase().as_str(), "y" | "yes") {
            eprintln!("Aborted.");
            return Ok(());
        }

        let ws = workspace::WorkspaceManager::new(&def.name)?;
        ws.write_plan(&plan_json)?;
        eprintln!("\nWorkspace: {}", ws.root().display());

        executor::execute(&plan, &ws, Arc::clone(&self.backend)).await?;

        eprintln!("\nDone. Traces at: {}", ws.root().display());
        Ok(())
    }
}

fn print_plan(plan: &ExecutionPlan) {
    println!(
        "\nCompiled plan: {} ({} step{})\n",
        plan.name,
        plan.steps.len(),
        if plan.steps.len() == 1 { "" } else { "s" }
    );
    for (i, step) in plan.steps.iter().enumerate() {
        match step {
            Step::Shell {
                name,
                cmd,
                output_var,
                ..
            } => {
                println!("  {:>2}  [shell]  {}", i + 1, name);
                println!("            $ {}", cmd);
                println!("            → {}", output_var);
            }
            Step::Llm {
                name,
                prompt,
                output_var,
                ..
            } => {
                let display = if prompt.len() > 80 {
                    format!("{}…", &prompt[..80])
                } else {
                    prompt.clone()
                };
                println!("  {:>2}  [llm  ]  {}", i + 1, name);
                println!("            {}", display);
                println!("            → {}", output_var);
            }
        }
        println!();
    }
}

use std::fs;

mod git;
use git::git_branch;

const DEFAULT_AGENTS_MD: &str = "You are Axon, a concise local AI coding assistant.\n";

pub struct ContextProvider {
    /// Git branch shown in the status bar (not injected into the system prompt).
    branch: Option<String>,
    agents_md: String,
    skill_content: Option<String>,
}

impl ContextProvider {
    pub fn new(skill_content: Option<String>) -> Self {
        Self {
            branch: git_branch(),
            agents_md: load_agents_md(),
            skill_content,
        }
    }

    /// Git branch for the status bar display.
    pub fn branch(&self) -> Option<&str> {
        self.branch.as_deref()
    }

    /// System prompt: AGENTS.md content, followed by the active skill body (if any).
    pub fn system_prompt(&self) -> String {
        let mut prompt = self.agents_md.clone();
        if let Some(skill) = &self.skill_content {
            if !prompt.ends_with('\n') {
                prompt.push('\n');
            }
            prompt.push('\n');
            prompt.push_str(skill);
            if !prompt.ends_with('\n') {
                prompt.push('\n');
            }
        }
        prompt
    }
}

/// Read `~/.axon/AGENTS.md`. On first run, create it with a default prompt.
fn load_agents_md() -> String {
    let Some(home) = dirs::home_dir() else {
        return DEFAULT_AGENTS_MD.to_string();
    };
    let axon_dir = home.join(".axon");
    let path = axon_dir.join("AGENTS.md");

    if path.exists() {
        return fs::read_to_string(&path).unwrap_or_else(|_| DEFAULT_AGENTS_MD.to_string());
    }

    // First run: create ~/.axon/AGENTS.md with the default prompt.
    let _ = fs::create_dir_all(&axon_dir);
    let _ = fs::write(&path, DEFAULT_AGENTS_MD);
    DEFAULT_AGENTS_MD.to_string()
}

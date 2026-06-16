mod git;
pub use git::{git_branch, git_summary};

pub struct ContextProvider {
    branch: Option<String>,
    summary: Option<String>,
}

impl ContextProvider {
    pub fn new() -> Self {
        Self {
            branch: git_branch(),
            summary: git_summary(),
        }
    }

    pub fn branch(&self) -> Option<&str> {
        self.branch.as_deref()
    }

    /// Short system prompt — kept minimal for 1B model compatibility.
    pub fn system_prompt(&self) -> String {
        let mut prompt = String::from("You are Axon, a concise local AI coding assistant.\n");
        if let Some(b) = &self.branch {
            prompt.push_str(&format!("Git branch: {b}\n"));
        }
        if let Some(s) = &self.summary {
            prompt.push_str(&format!("Recent commits:\n{s}\n"));
        }
        prompt
    }
}

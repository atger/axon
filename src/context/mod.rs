mod git;
use git::git_branch;

pub struct ContextProvider {
    branch: Option<String>,
    skill_content: Option<String>,
    cwd: Option<String>,
}

impl ContextProvider {
    pub fn new(skill_content: Option<String>) -> Self {
        let cwd = std::env::current_dir()
            .ok()
            .and_then(|p| p.to_str().map(str::to_owned));
        Self {
            branch: git_branch(),
            skill_content,
            cwd,
        }
    }

    pub fn branch(&self) -> Option<&str> {
        self.branch.as_deref()
    }

    pub fn system_prompt(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(cwd) = &self.cwd {
            parts.push(format!("Working directory: {cwd}"));
        }
        if let Some(skill) = &self.skill_content {
            parts.push(skill.clone());
        }
        parts.join("\n")
    }
}

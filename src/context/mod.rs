mod git;
use git::git_branch;

pub struct ContextProvider {
    branch: Option<String>,
    skill_content: Option<String>,
}

impl ContextProvider {
    pub fn new(skill_content: Option<String>) -> Self {
        Self {
            branch: git_branch(),
            skill_content,
        }
    }

    pub fn branch(&self) -> Option<&str> {
        self.branch.as_deref()
    }

    pub fn system_prompt(&self) -> String {
        self.skill_content.clone().unwrap_or_default()
    }
}

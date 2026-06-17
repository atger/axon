use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use color_eyre::eyre::{Context, Result, eyre};

pub struct WorkspaceManager {
    root: PathBuf,
}

impl WorkspaceManager {
    pub fn new(workflow_name: &str) -> Result<Self> {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let root = dirs::home_dir()
            .ok_or_else(|| eyre!("cannot determine home directory"))?
            .join(".axon")
            .join("workflows")
            .join(workflow_name)
            .join(ts.to_string());

        fs::create_dir_all(&root).wrap_err("failed to create workflow workspace")?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn write_plan(&self, content: &str) -> Result<()> {
        fs::write(self.root.join("plan.json"), content).wrap_err("failed to write plan.json")
    }

    pub fn step_dir(&self, dir_name: &str) -> PathBuf {
        let dir = self.root.join(dir_name);
        let _ = fs::create_dir_all(&dir);
        dir
    }

    pub fn output_path(&self, dir_name: &str) -> PathBuf {
        self.root.join(dir_name).join("output.txt")
    }

    pub fn output_exists(&self, dir_name: &str) -> bool {
        self.output_path(dir_name).exists()
    }

    pub fn write_file(&self, dir_name: &str, file_name: &str, content: &str) -> Result<()> {
        let dir = self.step_dir(dir_name);
        fs::write(dir.join(file_name), content)
            .wrap_err_with(|| format!("failed to write {file_name} for {dir_name}"))
    }
}

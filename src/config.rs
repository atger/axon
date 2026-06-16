use std::path::PathBuf;

use color_eyre::eyre::WrapErr;

fn default_context_window() -> usize {
    4096
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct ModelEntry {
    pub name: String,
    pub hf_repo: String,
    pub hf_file: String,
    #[serde(default = "default_context_window")]
    pub context_window: usize,
}

#[derive(Debug, Default, serde::Deserialize, serde::Serialize)]
pub struct AxonConfig {
    pub model: Option<String>,
    pub backend: Option<String>,
    pub ollama_url: Option<String>,
    pub context_window: Option<usize>,
    pub no_download: Option<bool>,
    #[serde(default)]
    pub models: Vec<ModelEntry>,
}

impl AxonConfig {
    pub fn path() -> color_eyre::Result<PathBuf> {
        let home = std::env::var("HOME")
            .map(PathBuf::from)
            .wrap_err("HOME environment variable not set")?;
        Ok(home.join(".axon").join("config.toml"))
    }

    pub fn load() -> Self {
        let Ok(path) = Self::path() else {
            return Self::default();
        };
        let Ok(contents) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        toml::from_str(&contents).unwrap_or_default()
    }

    pub fn save(&self) -> color_eyre::Result<()> {
        let path = Self::path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).wrap_err("failed to create config directory")?;
        }
        let contents = toml::to_string_pretty(self).wrap_err("failed to serialize config")?;
        std::fs::write(&path, contents).wrap_err("failed to write config file")
    }

    pub fn set(&mut self, key: &str, value: &str) -> color_eyre::Result<()> {
        match key {
            "model" => self.model = Some(value.to_string()),
            "backend" => self.backend = Some(value.to_string()),
            "ollama-url" => self.ollama_url = Some(value.to_string()),
            "context-window" => {
                self.context_window = Some(
                    value
                        .parse::<usize>()
                        .wrap_err("context-window must be a positive integer")?,
                );
            }
            "no-download" => {
                self.no_download = Some(
                    value
                        .parse::<bool>()
                        .wrap_err("no-download must be true or false")?,
                );
            }
            _ => color_eyre::eyre::bail!(
                "unknown config key '{key}'. Valid keys: model, backend, ollama-url, context-window, no-download"
            ),
        }
        Ok(())
    }

    pub fn find_model(&self, name: &str) -> Option<&ModelEntry> {
        self.models.iter().find(|m| m.name == name)
    }
}

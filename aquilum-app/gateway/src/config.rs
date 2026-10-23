use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    pub bind: SocketAddr,
    pub workspace: PathBuf,
    #[serde(default)]
    pub aquilum_settings: PathBuf,
    pub tokens_file: PathBuf,
    pub allowed_origins: Vec<String>,
    pub requests_per_minute: u32,
    pub burst: u32,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let mut config: Self =
            serde_json::from_slice(&std::fs::read(path).context("Cannot read config")?)
                .context("Invalid config JSON")?;
        let base = path.parent().unwrap_or(Path::new("."));
        for value in [
            &mut config.workspace,
            &mut config.aquilum_settings,
            &mut config.tokens_file,
        ] {
            if !value.as_os_str().is_empty() && value.is_relative() {
                *value = base.join(&*value);
            }
        }
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&mut self) -> Result<()> {
        let config = self;
        ensure!(
            config.bind.ip().is_loopback(),
            "Bind must be loopback; expose the API through IIS HTTPS"
        );
        ensure!(
            (1..=6000).contains(&config.requests_per_minute),
            "Invalid request limit"
        );
        ensure!((1..=100).contains(&config.burst), "Invalid burst limit");
        ensure!(
            config.allowed_origins.len() <= 16,
            "Too many allowed origins"
        );
        for origin in &config.allowed_origins {
            let url = url::Url::parse(origin).context("Invalid allowed origin")?;
            ensure!(
                url.scheme() == "https" && url.origin().ascii_serialization() == *origin,
                "Allowed origins must be exact HTTPS origins, without paths or trailing slashes"
            );
        }
        if config.enabled || config.workspace.is_dir() {
            ensure!(
                !config.workspace.as_os_str().is_empty(),
                "Choose a notes folder first"
            );
            config.workspace = config
                .workspace
                .canonicalize()
                .context("Notes folder must exist")?;
            ensure!(
                config.workspace.is_dir(),
                "Notes folder must be a directory"
            );
        }
        Ok(())
    }
}

fn default_enabled() -> bool {
    true
}

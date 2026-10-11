use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

/// Optional `pinr.toml` in the scanned directory.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Host for GitLab `include: project` and `$CI_SERVER_FQDN`.
    pub gitlab_host: String,
    /// Skip occurrences whose path or reference contains one of these strings.
    pub ignore: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            gitlab_host: "gitlab.com".into(),
            ignore: Vec::new(),
        }
    }
}

pub fn load(dir: &Path) -> Result<Config> {
    let path = dir.join("pinr.toml");
    if !path.exists() {
        return Ok(Config::default());
    }
    let src =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&src).with_context(|| format!("parsing {}", path.display()))
}

use std::path::Path;

use anyhow::{Context, Result};

/// The only way the app reads and writes project files, so tests can mock it.
pub trait FileSystem: Send + Sync {
    fn read(&self, path: &Path) -> Result<String>;
    fn write(&self, path: &Path, contents: &str) -> Result<()>;
}

pub struct RealFs;

impl FileSystem for RealFs {
    fn read(&self, path: &Path) -> Result<String> {
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
    }

    /// Write to a temp file next to the target, then rename, so a crash never
    /// leaves a half-written file behind.
    fn write(&self, path: &Path, contents: &str) -> Result<()> {
        let name = path
            .file_name()
            .context("path has no file name")?
            .to_string_lossy();
        let tmp = path.with_file_name(format!(".{name}.pinr-tmp"));
        std::fs::write(&tmp, contents).with_context(|| format!("writing {}", tmp.display()))?;
        if let Ok(meta) = std::fs::metadata(path) {
            std::fs::set_permissions(&tmp, meta.permissions())?;
        }
        std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))
    }
}

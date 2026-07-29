//! Data/config directory resolution (XDG on Linux/macOS).

use std::path::PathBuf;

/// Root for all derived data. Override with $ROGREP_DATA_DIR.
pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("ROGREP_DATA_DIR") {
        return PathBuf::from(dir);
    }
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        let p = PathBuf::from(xdg);
        if p.is_absolute() {
            return p.join("rogrep");
        }
    }
    home_dir().join(".local/share/rogrep")
}

/// Config file path. Override with $ROGREP_CONFIG.
pub fn config_path() -> PathBuf {
    if let Some(p) = std::env::var_os("ROGREP_CONFIG") {
        return PathBuf::from(p);
    }
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        let p = PathBuf::from(xdg);
        if p.is_absolute() {
            return p.join("rogrep/config.toml");
        }
    }
    home_dir().join(".config/rogrep/config.toml")
}

pub fn home_dir() -> PathBuf {
    #[allow(deprecated)] // un-deprecated in recent Rust; harmless either way
    std::env::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

pub struct DataLayout {
    pub root: PathBuf,
}

impl DataLayout {
    pub fn new(root: PathBuf) -> Self {
        DataLayout { root }
    }

    pub fn default_layout() -> Self {
        DataLayout::new(data_dir())
    }

    pub fn db_path(&self) -> PathBuf {
        self.root.join("db/rogrep.sqlite3")
    }

    pub fn index_dir(&self, schema_version: u32) -> PathBuf {
        self.root.join(format!("index/v{schema_version}"))
    }

    pub fn index_parent(&self) -> PathBuf {
        self.root.join("index")
    }

    pub fn spool_dir(&self, provider: &str) -> PathBuf {
        self.root.join("spool").join(provider)
    }

    pub fn writer_lock_path(&self) -> PathBuf {
        self.root.join("locks/writer.lock")
    }
}

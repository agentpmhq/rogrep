use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub sources: SourcesConfig,
    pub index: IndexConfig,
    pub stats: StatsConfig,
    pub remote: RemoteConfig,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SourcesConfig {
    /// Extra JSONL roots to scan with the generic parser.
    pub extra_roots: Vec<String>,
    /// Providers to skip entirely (by kind name, e.g. "grok").
    pub disabled_providers: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IndexConfig {
    /// Max bytes of turn text stored in the search index (0 = unlimited).
    /// Excerpts for truncated turns fall back to source-file byte offsets.
    pub store_text_bytes_per_turn: u64,
    /// Recency half-life in days for search ranking.
    pub recency_half_life_days: f64,
}

impl Default for IndexConfig {
    fn default() -> Self {
        IndexConfig {
            store_text_bytes_per_turn: 0,
            recency_half_life_days: 30.0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StatsConfig {
    /// IANA timezone for day/hour bucketing; empty = system local zone.
    pub timezone: String,
    /// Extra model pricing overrides: model name prefix -> per-MTok USD.
    pub pricing: Vec<PricingOverride>,
}

impl Default for StatsConfig {
    fn default() -> Self {
        StatsConfig {
            timezone: String::new(),
            pricing: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PricingOverride {
    pub model_prefix: String,
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    #[serde(default)]
    pub cache_read_per_mtok: f64,
    #[serde(default)]
    pub cache_write_per_mtok: f64,
}

/// Remote analysis is OFF by default and stays off unless explicitly enabled.
/// No rogrep code path other than the remote module ever performs network I/O.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RemoteConfig {
    pub enabled: bool,
    pub endpoint: String,
}

impl Config {
    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|e| ConfigError::Parse(path.display().to_string(), e.to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(ConfigError::Io(path.display().to_string(), e.to_string())),
        }
    }

    pub fn load_default() -> Result<Config, ConfigError> {
        Config::load(&crate::paths::config_path())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read config {0}: {1}")]
    Io(String, String),
    #[error("failed to parse config {0}: {1}")]
    Parse(String, String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_roundtrips() {
        let c = Config::default();
        let s = toml::to_string(&c).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert!(!back.remote.enabled);
    }

    #[test]
    fn partial_config_parses() {
        let c: Config = toml::from_str("[sources]\nextra_roots=[\"/x\"]\n").unwrap();
        assert_eq!(c.sources.extra_roots, vec!["/x"]);
        assert_eq!(c.index.recency_half_life_days, 30.0);
    }
}

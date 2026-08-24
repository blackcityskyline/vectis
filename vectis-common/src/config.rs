use crate::types::{GpuPowerBackend, TdpLimit};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const DEFAULT_CONFIG_PATH: &str = "/etc/vectis/config.toml";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub power_backend: PowerBackendKind,
    #[serde(default)]
    pub tdp: TdpConfig,
    #[serde(default)]
    pub gpu: GpuConfig,
    #[serde(default)]
    pub conflicts: ConflictConfig,
}

/// Auto probes tlp → ppd → asusd
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PowerBackendKind {
    #[default]
    Auto,
    Tlp,
    Ppd,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TdpConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(flatten)]
    pub default_limit: TdpLimit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuConfig {
    #[serde(default)]
    pub backend: GpuPowerBackend,
    pub acpi_call_off: Option<String>,
    pub acpi_call_on: Option<String>,
}

impl Default for GpuConfig {
    fn default() -> Self {
        Self { backend: GpuPowerBackend::Auto, acpi_call_off: None, acpi_call_on: None }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictConfig {
    #[serde(default = "default_conflicting_services")]
    pub watch_services: Vec<String>,
}

impl Default for ConflictConfig {
    fn default() -> Self {
        Self { watch_services: default_conflicting_services() }
    }
}

fn default_conflicting_services() -> Vec<String> {
    vec![
        "supergfxd.service".to_string(),
        "optimus-manager.service".to_string(),
        "bumblebeed.service".to_string(),
    ]
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, crate::error::Error> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path)?;
        let cfg: Config = toml::from_str(&text)?;
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_roundtrips_toml() {
        let cfg = Config::default();
        let s = toml::to_string(&cfg).expect("serialize");
        let back: Config = toml::from_str(&s).expect("deserialize");
        assert_eq!(back.power_backend, cfg.power_backend);
    }

    #[test]
    fn missing_file_falls_back_to_default() {
        let cfg = Config::load(Path::new("/nonexistent/vectis.toml")).unwrap();
        assert_eq!(cfg.power_backend, PowerBackendKind::Auto);
    }

    #[test]
    fn tlp_and_ppd_services_excluded_from_default_conflict_list() {
        let cfg = Config::default();
        assert!(!cfg.conflicts.watch_services.contains(&"tlp.service".to_string()));
        assert!(!cfg.conflicts.watch_services.contains(&"power-profiles-daemon.service".to_string()));
    }
}

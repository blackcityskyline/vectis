use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    Powersave,
    Balanced,
    Performance,
}

impl fmt::Display for Profile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Profile::Powersave => "powersave",
            Profile::Balanced => "balanced",
            Profile::Performance => "performance",
        };
        write!(f, "{s}")
    }
}

impl std::str::FromStr for Profile {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            // `power-saver` is the name every shell and PPD client uses; accept
            // it here too so the CLI and the D-Bus API take the same spelling.
            "powersave" | "power-saver" => Ok(Profile::Powersave),
            "balanced" => Ok(Profile::Balanced),
            "performance" => Ok(Profile::Performance),
            other => Err(format!(
                "unknown profile '{other}' (expected powersave|power-saver|balanced|performance)"
            )),
        }
    }
}

impl Profile {
    /// Wire name used by power-profiles-daemon's D-Bus API. TLP happens to
    /// accept the same names as subcommands, so both backends share this.
    pub fn ppd_name(self) -> &'static str {
        match self {
            Profile::Powersave => "power-saver",
            Profile::Balanced => "balanced",
            Profile::Performance => "performance",
        }
    }

    pub fn from_ppd_name(s: &str) -> Option<Self> {
        match s {
            "power-saver" => Some(Profile::Powersave),
            "balanced" => Some(Profile::Balanced),
            "performance" => Some(Profile::Performance),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuMode {
    /// Discrete GPU powered off, only iGPU
    Integrated,
    /// Discrete GPU on + driver loaded, iGPU drives display
    Hybrid,
    /// Discrete GPU as exclusive display source (degrades to hybrid on muxless)
    Discrete,
}

impl fmt::Display for GpuMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            GpuMode::Integrated => "integrated",
            GpuMode::Discrete => "discrete",
            GpuMode::Hybrid => "hybrid",
        };
        write!(f, "{s}")
    }
}

impl std::str::FromStr for GpuMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "integrated" => Ok(GpuMode::Integrated),
            "discrete" => Ok(GpuMode::Discrete),
            "hybrid" => Ok(GpuMode::Hybrid),
            other => Err(format!("unknown gpu mode '{other}' (expected integrated|discrete|hybrid)")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuVendor {
    Nvidia,
    Amd,
}

/// Only meaningful for nvidia (Auto=proprietary, OpenSource=nouveau). AMD always uses amdgpu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GpuDriverPref {
    #[default]
    Auto,
    OpenSource,
}

impl std::str::FromStr for GpuDriverPref {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "auto" => Ok(GpuDriverPref::Auto),
            "open" | "open_source" | "opensource" => Ok(GpuDriverPref::OpenSource),
            other => Err(format!("unknown gpu driver preference '{other}' (expected auto|open)")),
        }
    }
}

/// Priority order for Auto: runtime_pm → bbswitch → acpi_call
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GpuPowerBackend {
    #[default]
    Auto,
    Bbswitch,
    AcpiCall,
    RuntimePm,
}

/// hard_lock sets PL2=PL1, removing the boost window entirely
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TdpLimit {
    pub pl1_watts: Option<u32>,
    pub pl2_watts: Option<u32>,
    pub tau_us: Option<u32>,
    #[serde(default)]
    pub hard_lock: bool,
}

impl TdpLimit {
    pub fn resolve(&self) -> (Option<u32>, Option<u32>, Option<u32>) {
        if self.hard_lock {
            let pl1 = self.pl1_watts;
            (pl1, pl1, self.tau_us)
        } else {
            (self.pl1_watts, self.pl2_watts, self.tau_us)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub active_profile: Option<Profile>,
    pub power_backend: String,
    pub tlp_service_active: bool,
    pub tdp_pl1_watts: Option<u32>,
    pub tdp_pl2_watts: Option<u32>,
    pub gpu_mode: Option<GpuMode>,
    pub gpu_power_state: String,
    pub gpu_pending: Option<GpuMode>,
    pub gpu_last_auto_apply_error: Option<String>,
    pub conflicting_services: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn profile_parses_both_vectis_and_ppd_spellings() {
        assert_eq!(Profile::from_str("powersave"), Ok(Profile::Powersave));
        assert_eq!(Profile::from_str("power-saver"), Ok(Profile::Powersave));
        assert_eq!(Profile::from_str("balanced"), Ok(Profile::Balanced));
        assert_eq!(Profile::from_str("performance"), Ok(Profile::Performance));
        assert!(Profile::from_str("bogus").is_err());
    }
}

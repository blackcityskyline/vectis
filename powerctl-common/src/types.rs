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
            "powersave" => Ok(Profile::Powersave),
            "balanced" => Ok(Profile::Balanced),
            "performance" => Ok(Profile::Performance),
            other => Err(format!("unknown profile '{other}' (expected powersave|balanced|performance)")),
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

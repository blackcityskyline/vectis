use zbus::dbus_proxy;

pub const BUS_NAME: &str = "org.vectis.Daemon1";
pub const OBJECT_PATH: &str = "/org/vectis/Daemon";

/// power-profiles-daemon's current identity (since 0.20, project moved under
/// UPower). Shells and Quickshell-based desktops use this one exclusively.
pub const PPD_MODERN_BUS_NAME: &str = "org.freedesktop.UPower.PowerProfiles";
pub const PPD_MODERN_PATH: &str = "/org/freedesktop/UPower/PowerProfiles";
/// Pre-0.20 identity, still advertised by PPD/tuned-ppd for older clients.
pub const PPD_LEGACY_BUS_NAME: &str = "net.hadess.PowerProfiles";
pub const PPD_LEGACY_PATH: &str = "/net/hadess/PowerProfiles";

#[dbus_proxy(
    interface = "org.vectis.Daemon1",
    default_service = "org.vectis.Daemon1",
    default_path = "/org/vectis/Daemon"
)]
pub trait Daemon {
    fn get_status(&self) -> zbus::Result<String>;
    fn apply_profile(&self, profile: &str) -> zbus::Result<()>;
    fn set_tdp(&self, pl1_watts: u32, pl2_watts: u32, tau_us: u32, hard_lock: bool) -> zbus::Result<()>;
    fn clear_tdp(&self) -> zbus::Result<()>;
    fn set_gpu_mode(&self, mode: &str, driver: &str, force: bool) -> zbus::Result<()>;
    /// Drops a queued GPU switch. Returns whether one was queued.
    fn clear_gpu_pending(&self) -> zbus::Result<bool>;
    fn list_gpu_devices(&self) -> zbus::Result<String>;
    fn get_conflicts(&self) -> zbus::Result<Vec<String>>;
}

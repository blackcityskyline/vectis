use crate::procutil::{run_shell_with_timeout, run_with_timeout};
use std::time::Duration;

pub const GPU_HOLDER_PROCESSES: &[&str] = &[
    "Hyprland", "niri", "sway", "mango", "dwl", "kwin_wayland", "gnome-shell", "cinnamon",
    "bspwm", "dwm", "oxwm", "kwin_x11", "xfwm4", "openbox", "icewm",
    "Xorg", "Xwayland",
];

const KILL_TIMEOUT: Duration = Duration::from_secs(5);

/// SIGKILL GPU holders + fuser -k on DRI/nvidia device nodes
pub fn kill_gpu_holders() {
    for name in GPU_HOLDER_PROCESSES {
        let _ = run_with_timeout("pkill", &["-9", "-x", name], KILL_TIMEOUT);
    }
    let _ = run_shell_with_timeout("fuser -k -9 /dev/dri/* /dev/nvidia* >/dev/null 2>&1", KILL_TIMEOUT);
}

/// Kill only nvidia holders (safe for unattended paths where greeter compositor holds Intel DRI)
pub fn kill_nvidia_holders_only() {
    let _ = run_shell_with_timeout("fuser -k -9 /dev/nvidia* >/dev/null 2>&1", KILL_TIMEOUT);
}

/// True if any process holds /dev/nvidia*. Fails closed (true) on errors.
pub fn nvidia_device_in_use() -> bool {
    match run_shell_with_timeout("fuser /dev/nvidia* >/dev/null 2>&1", KILL_TIMEOUT) {
        Ok(Some(status)) => status.success(),
        Ok(None) | Err(_) => true,
    }
}

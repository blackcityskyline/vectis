use crate::procutil::{run_shell_capture, run_shell_with_timeout, run_with_timeout};
use std::fs;
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

/// Parses `fuser -v` output into ["sunshine(7902)", "Xorg(1234)"], order kept, deduped.
pub fn parse_fuser_holders(output: &str) -> Vec<String> {
    let mut holders: Vec<String> = Vec::new();
    for line in output.lines() {
        let mut it = line.split_whitespace();
        let Some(first) = it.next() else { continue };
        if first.starts_with("USER") {
            continue; // header line
        }
        // device lines start with "/dev/...:", continuation lines start with USER
        let user = if first.starts_with("/dev/") { it.next() } else { Some(first) };
        let (Some(_user), Some(pid), Some(_access)) = (user, it.next(), it.next()) else {
            continue;
        };
        let Some(cmd) = it.next() else { continue };
        if !pid.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let holder = format!("{cmd}({pid})");
        if !holders.contains(&holder) {
            holders.push(holder);
        }
    }
    holders
}

/// Kernel modules that bind the discrete card and would keep it from powering down.
fn loaded_gpu_modules() -> Vec<String> {
    let Ok(text) = fs::read_to_string("/proc/modules") else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|l| l.split_whitespace().next())
        .filter(|m| m.starts_with("nvidia") || *m == "nouveau")
        .map(|m| m.to_string())
        .collect()
}

/// Who keeps the GPU busy right now: userspace holders of /dev/nvidia* and
/// loaded card-binding kernel modules. Empty = nothing found.
// ponytail: /dev/nvidia* only - dGPU renderD* nodes would need a sysfs walk to
// distinguish from iGPU; add if Prime-Vulkan-only holders ever matter.
pub fn gpu_holders_report() -> String {
    let output = run_shell_capture("fuser -v /dev/nvidia* 2>&1", KILL_TIMEOUT)
        .ok()
        .flatten()
        .unwrap_or_default();
    let holders = parse_fuser_holders(&output);
    let modules = loaded_gpu_modules();

    let mut parts = Vec::new();
    if !holders.is_empty() {
        parts.push(format!("held by {}", holders.join(", ")));
    }
    if !modules.is_empty() {
        parts.push(format!("kernel modules: {}", modules.join(", ")));
    }
    parts.join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fuser_verbose_table() {
        let out = "\
                     USER        PID ACCESS COMMAND
/dev/nvidiactl:      black      7902 F.... sunshine
/dev/dri/card0:      black      1234 ...m- Hyprland
                     black      7902 F.... sunshine
";
        let holders = parse_fuser_holders(out);
        assert_eq!(holders, vec!["sunshine(7902)", "Hyprland(1234)"]);
    }

    #[test]
    fn skips_header_device_only_and_garbage_lines() {
        let out = "\
                     USER        PID ACCESS COMMAND
/dev/nvidia-uvm:
No such file or directory
sh: 1: fuser: not found
";
        assert!(parse_fuser_holders(out).is_empty());
    }

    #[test]
    fn holders_report_is_well_formed_or_empty() {
        // machine-dependent: just never panics and stays on one line
        let report = gpu_holders_report();
        assert!(!report.contains('\n'));
        if !report.is_empty() {
            assert!(report.starts_with("held by ") || report.starts_with("kernel modules: "));
        }
    }
}

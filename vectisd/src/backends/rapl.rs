use vectis_common::error::{Error, Result};
use vectis_common::types::TdpLimit;
use std::fs;
use std::path::{Path, PathBuf};

/// RAPL powercap manager: writes PL1/PL2/tau to sysfs intel-rapl in microwatts
pub struct RaplManager {
    zone: PathBuf,
}

fn find_package_zone(root: &Path) -> Option<PathBuf> {
    let base = root.join("sys/class/powercap");
    let entries = fs::read_dir(&base).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let name_file = path.join("name");
        if let Ok(name) = fs::read_to_string(&name_file) {
            if name.trim().starts_with("package") {
                return Some(path);
            }
        }
    }
    None
}

fn write_sysfs(path: &Path, value: &str) -> Result<()> {
    fs::write(path, value).map_err(|source| Error::SysfsWrite {
        path: path.display().to_string(),
        source,
    })
}

fn read_sysfs_trim(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

impl RaplManager {
    pub fn detect(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        let zone = find_package_zone(root).ok_or(Error::NoBackend("RAPL powercap (intel-rapl package zone)"))?;
        Ok(Self { zone })
    }

    fn constraint_file(&self, constraint: u32, field: &str) -> PathBuf {
        self.zone.join(format!("constraint_{constraint}_{field}"))
    }

    pub fn apply(&self, limit: &TdpLimit) -> Result<()> {
        let (pl1, pl2, tau) = limit.resolve();
        if let Some(watts) = pl1 {
            write_sysfs(&self.constraint_file(0, "power_limit_uw"), &(watts as u64 * 1_000_000).to_string())?;
        }
        if let Some(watts) = pl2 {
            write_sysfs(&self.constraint_file(1, "power_limit_uw"), &(watts as u64 * 1_000_000).to_string())?;
        }
        if let Some(us) = tau {
            let path = self.constraint_file(1, "time_window_us");
            if path.exists() {
                write_sysfs(&path, &us.to_string())?;
            }
        }
        Ok(())
    }

    /// Reset to hardware defaults (reads constraint_N_max_power_uw)
    pub fn clear(&self) -> Result<()> {
        for constraint in [0u32, 1] {
            let max_path = self.constraint_file(constraint, "max_power_uw");
            if let Some(max_uw) = read_sysfs_trim(&max_path) {
                let limit_path = self.constraint_file(constraint, "power_limit_uw");
                write_sysfs(&limit_path, &max_uw)?;
            }
        }
        Ok(())
    }

    pub fn current_pl1_watts(&self) -> Option<u32> {
        read_sysfs_trim(&self.constraint_file(0, "power_limit_uw"))?
            .parse::<u64>()
            .ok()
            .map(|uw| (uw / 1_000_000) as u32)
    }

    pub fn current_pl2_watts(&self) -> Option<u32> {
        read_sysfs_trim(&self.constraint_file(1, "power_limit_uw"))?
            .parse::<u64>()
            .ok()
            .map(|uw| (uw / 1_000_000) as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fake_rapl_zone() -> TempDir {
        let tmp = TempDir::new().unwrap();
        let zone = tmp.path().join("sys/class/powercap/intel-rapl:0");
        fs::create_dir_all(&zone).unwrap();
        fs::write(zone.join("name"), "package-0\n").unwrap();
        fs::write(zone.join("constraint_0_power_limit_uw"), "15000000\n").unwrap();
        fs::write(zone.join("constraint_0_max_power_uw"), "35000000\n").unwrap();
        fs::write(zone.join("constraint_1_power_limit_uw"), "35000000\n").unwrap();
        fs::write(zone.join("constraint_1_max_power_uw"), "44000000\n").unwrap();
        fs::write(zone.join("constraint_1_time_window_us"), "28000000\n").unwrap();
        tmp
    }

    #[test]
    fn detects_package_zone_and_skips_core_uncore() {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path().join("sys/class/powercap");
        fs::create_dir_all(base.join("intel-rapl:0")).unwrap();
        fs::write(base.join("intel-rapl:0/name"), "package-0\n").unwrap();
        fs::create_dir_all(base.join("intel-rapl:0:0")).unwrap();
        fs::write(base.join("intel-rapl:0:0/name"), "core\n").unwrap();

        let mgr = RaplManager::detect(tmp.path()).unwrap();
        assert!(mgr.zone.ends_with("intel-rapl:0"));
    }

    #[test]
    fn plain_limit_sets_pl1_and_pl2_independently() {
        let tmp = fake_rapl_zone();
        let mgr = RaplManager::detect(tmp.path()).unwrap();
        mgr.apply(&TdpLimit { pl1_watts: Some(10), pl2_watts: Some(20), tau_us: None, hard_lock: false })
            .unwrap();
        assert_eq!(mgr.current_pl1_watts(), Some(10));
        assert_eq!(mgr.current_pl2_watts(), Some(20));
    }

    #[test]
    fn hard_lock_forces_pl2_equal_to_pl1() {
        let tmp = fake_rapl_zone();
        let mgr = RaplManager::detect(tmp.path()).unwrap();
        mgr.apply(&TdpLimit { pl1_watts: Some(12), pl2_watts: Some(45), tau_us: None, hard_lock: true })
            .unwrap();
        assert_eq!(mgr.current_pl1_watts(), Some(12));
        assert_eq!(mgr.current_pl2_watts(), Some(12), "hard_lock must mirror pl1");
    }

    #[test]
    fn clear_restores_hardware_max() {
        let tmp = fake_rapl_zone();
        let mgr = RaplManager::detect(tmp.path()).unwrap();
        mgr.apply(&TdpLimit { pl1_watts: Some(8), pl2_watts: Some(8), tau_us: None, hard_lock: false })
            .unwrap();
        mgr.clear().unwrap();
        assert_eq!(mgr.current_pl1_watts(), Some(35));
        assert_eq!(mgr.current_pl2_watts(), Some(44));
    }

    #[test]
    fn no_rapl_support_returns_no_backend_error() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("sys/class/powercap")).unwrap();
        let result = RaplManager::detect(tmp.path());
        assert!(matches!(result, Err(Error::NoBackend(_))));
    }
}

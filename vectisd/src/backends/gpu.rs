use vectis_common::error::{Error, Result};
use vectis_common::types::{GpuDriverPref, GpuMode, GpuPowerBackend, GpuVendor};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub trait ModuleLoader {
    fn is_loaded(&self, module: &str) -> bool;
    fn load(&self, module: &str, args: &[&str]) -> Result<()>;
    fn unload(&self, module: &str) -> Result<()>;
}

const MODPROBE_TIMEOUT: Duration = Duration::from_secs(15);

pub struct SystemModprobe {
    proc_modules: PathBuf,
}

impl SystemModprobe {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self { proc_modules: root.as_ref().join("proc/modules") }
    }
}

impl ModuleLoader for SystemModprobe {
    fn is_loaded(&self, module: &str) -> bool {
        let Ok(text) = fs::read_to_string(&self.proc_modules) else { return false };
        text.lines().any(|line| line.split_whitespace().next() == Some(module))
    }

    fn load(&self, module: &str, args: &[&str]) -> Result<()> {
        let mut full_args = vec![module];
        full_args.extend_from_slice(args);
        match crate::procutil::run_with_timeout("modprobe", &full_args, MODPROBE_TIMEOUT) {
            Ok(Some(status)) if status.success() => Ok(()),
            Ok(Some(status)) => Err(Error::Other(format!("modprobe {module} failed (exit {status})"))),
            Ok(None) => Err(Error::Other(format!("modprobe {module} timed out after {MODPROBE_TIMEOUT:?} - killed it"))),
            Err(e) => Err(Error::Other(format!("failed to spawn modprobe: {e}"))),
        }
    }

    fn unload(&self, module: &str) -> Result<()> {
        if !self.is_loaded(module) {
            return Ok(());
        }
        match crate::procutil::run_with_timeout("modprobe", &["-r", module], MODPROBE_TIMEOUT) {
            Ok(Some(status)) if status.success() => Ok(()),
            Ok(Some(status)) => Err(Error::Other(format!("modprobe -r {module} failed (exit {status}, still in use?)"))),
            Ok(None) => {
                Err(Error::Other(format!("modprobe -r {module} timed out after {MODPROBE_TIMEOUT:?} - killed it")))
            }
            Err(e) => Err(Error::Other(format!("failed to spawn modprobe -r: {e}"))),
        }
    }
}

pub trait BbswitchIo {
    fn exists(&self) -> bool;
    fn read_state(&self) -> Option<String>;
    fn write(&self, value: &str) -> Result<()>;
}

pub struct SystemBbswitch {
    path: PathBuf,
}

impl SystemBbswitch {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self { path: root.as_ref().join("proc/acpi/bbswitch") }
    }
}

impl BbswitchIo for SystemBbswitch {
    fn exists(&self) -> bool {
        self.path.exists()
    }

    fn read_state(&self) -> Option<String> {
        let text = fs::read_to_string(&self.path).ok()?;
        text.split_whitespace().nth(1).map(|s| s.to_string())
    }

    fn write(&self, value: &str) -> Result<()> {
        fs::write(&self.path, value).map_err(|source| Error::SysfsWrite {
            path: self.path.display().to_string(),
            source,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CardPower {
    On,
    Off,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct PciDevice {
    pub address: String,
    pub vendor: String,
    pub device: String,
    pub class: String,
    pub driver: Option<String>,
}

/// Bus 00 = integrated (CPU-wired), anything else = discrete
fn is_integrated_bus(pci_address: &str) -> bool {
    pci_address.split(':').nth(1) == Some("00")
}

fn read_trim(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

pub struct GpuManager<M: ModuleLoader, B: BbswitchIo> {
    root: PathBuf,
    backend: GpuPowerBackend,
    modules: M,
    bbswitch: B,
    acpi_call_off: Option<String>,
    acpi_call_on: Option<String>,
    poll_attempts: u32,
    poll_interval: Duration,
}

impl<M: ModuleLoader, B: BbswitchIo> GpuManager<M, B> {
    pub fn new(
        root: impl Into<PathBuf>,
        backend: GpuPowerBackend,
        modules: M,
        bbswitch: B,
        acpi_call_off: Option<String>,
        acpi_call_on: Option<String>,
    ) -> Self {
        Self {
            root: root.into(),
            backend,
            modules,
            bbswitch,
            acpi_call_off,
            acpi_call_on,
            poll_attempts: 60,
            poll_interval: Duration::from_millis(200),
        }
    }

    #[cfg(test)]
    pub fn with_fast_poll(mut self) -> Self {
        self.poll_attempts = 1;
        self.poll_interval = Duration::from_millis(0);
        self
    }

    #[cfg(test)]
    pub fn with_test_poll(mut self, attempts: u32, interval: Duration) -> Self {
        self.poll_attempts = attempts;
        self.poll_interval = interval;
        self
    }

    fn acpi_call_path(&self) -> PathBuf {
        self.root.join("proc/acpi/call")
    }

    pub fn list_devices(&self) -> Vec<PciDevice> {
        let base = self.root.join("sys/bus/pci/devices");
        let mut out = Vec::new();
        let Ok(entries) = fs::read_dir(&base) else { return out };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(class) = read_trim(&path.join("class")) else { continue };
            if !class.starts_with("0x03") {
                continue;
            }
            out.push(self.read_pci_device(&path));
        }
        out
    }

    fn read_pci_device(&self, path: &Path) -> PciDevice {
        let address = path.file_name().unwrap().to_string_lossy().to_string();
        let vendor = read_trim(&path.join("vendor")).unwrap_or_default();
        let device = read_trim(&path.join("device")).unwrap_or_default();
        let class = read_trim(&path.join("class")).unwrap_or_default();
        let driver = fs::read_link(path.join("driver"))
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()));
        PciDevice { address, vendor, device, class, driver }
    }

    pub fn has_integrated_gpu(&self) -> bool {
        self.list_devices().iter().any(|d| is_integrated_bus(&d.address))
    }

    fn discrete_pci_address(&self) -> Option<String> {
        self.list_devices().into_iter().find(|d| !is_integrated_bus(&d.address)).map(|d| d.address)
    }

    pub fn detect_vendor(&self) -> Option<GpuVendor> {
        self.list_devices()
            .into_iter()
            .find(|d| !is_integrated_bus(&d.address))
            .and_then(|d| match d.vendor.as_str() {
                "0x10de" => Some(GpuVendor::Nvidia),
                "0x1002" => Some(GpuVendor::Amd),
                _ => None,
            })
    }

    fn pci_runtime_status(&self, addr: &str) -> Option<String> {
        read_trim(&self.root.join(format!("sys/bus/pci/devices/{addr}/power/runtime_status")))
    }

    fn pci_power_state(&self) -> CardPower {
        let Some(addr) = self.discrete_pci_address() else { return CardPower::Unknown };
        match self.pci_runtime_status(&addr).as_deref() {
            Some("suspended") => CardPower::Off,
            Some(_) => CardPower::On,
            None => CardPower::Unknown,
        }
    }

    /// nvidia: prefers bbswitch state; falls back to PCI runtime_status. AMD: PCI runtime_status.
    pub fn card_power(&self) -> CardPower {
        match self.detect_vendor() {
            Some(GpuVendor::Nvidia) => match self.bbswitch.read_state().as_deref() {
                Some("ON") => CardPower::On,
                Some("OFF") => CardPower::Off,
                _ => self.pci_power_state(),
            },
            Some(GpuVendor::Amd) => self.pci_power_state(),
            None => CardPower::Unknown,
        }
    }

    /// Can't structurally distinguish Discrete from Hybrid — both mean "driver loaded, card on"
    pub fn detect_mode(&self) -> Option<GpuMode> {
        match self.detect_vendor()? {
            GpuVendor::Nvidia => {
                if self.modules.is_loaded("nvidia") || self.modules.is_loaded("nouveau") {
                    Some(GpuMode::Hybrid)
                } else if self.card_power() == CardPower::Off {
                    Some(GpuMode::Integrated)
                } else {
                    None
                }
            }
            GpuVendor::Amd => match self.pci_power_state() {
                CardPower::Off => Some(GpuMode::Integrated),
                CardPower::On => Some(GpuMode::Hybrid),
                CardPower::Unknown => None,
            },
        }
    }

    pub fn switch_to(&self, mode: GpuMode, driver_pref: GpuDriverPref) -> Result<()> {
        let addr = self.discrete_pci_address().ok_or(Error::NoBackend("no discrete GPU detected on this system"))?;

        if (mode == GpuMode::Integrated || mode == GpuMode::Hybrid) && !self.has_integrated_gpu() {
            return Err(Error::Other(format!(
                "{mode} mode requires an integrated GPU, but none was detected (discrete-only system?)"
            )));
        }

        if mode == GpuMode::Discrete && self.has_integrated_gpu() {
            tracing::info!(
                "gpu: 'discrete' requested but muxless hardware can't stop iGPU driving display - degrades to hybrid"
            );
        }

        match self.detect_vendor() {
            Some(GpuVendor::Nvidia) => self.switch_nvidia(mode, driver_pref, &addr),
            Some(GpuVendor::Amd) => self.switch_amd(mode, &addr),
            None => Err(Error::NoBackend("could not determine discrete GPU vendor (not nvidia 0x10de or AMD 0x1002)")),
        }
    }

    fn wait_for_suspend(&self, addr: &str) -> bool {
        for _ in 0..self.poll_attempts {
            if self.pci_runtime_status(addr).as_deref() == Some("suspended") {
                return true;
            }
            if self.poll_interval > Duration::ZERO {
                std::thread::sleep(self.poll_interval);
            }
        }
        self.pci_runtime_status(addr).as_deref() == Some("suspended")
    }

    fn wait_for_bbswitch_state(&self, want: &str) -> bool {
        for _ in 0..self.poll_attempts {
            if self.bbswitch.read_state().as_deref() == Some(want) {
                return true;
            }
            if self.poll_interval > Duration::ZERO {
                std::thread::sleep(self.poll_interval);
            }
        }
        self.bbswitch.read_state().as_deref() == Some(want)
    }

    // --- nvidia power tiers ---

    fn runtime_pm_off_verified(&self, addr: &str) -> Result<()> {
        let path = self.root.join(format!("sys/bus/pci/devices/{addr}/power/control"));
        fs::write(&path, "auto").map_err(|source| Error::SysfsWrite { path: path.display().to_string(), source })?;
        if self.wait_for_suspend(addr) {
            Ok(())
        } else {
            Err(Error::Other(format!("runtime PM did not reach D3cold on {addr} within the poll budget")))
        }
    }

    fn runtime_pm_on(&self, addr: &str) -> Result<()> {
        let path = self.root.join(format!("sys/bus/pci/devices/{addr}/power/control"));
        fs::write(&path, "on").map_err(|source| Error::SysfsWrite { path: path.display().to_string(), source })
    }

    fn bbswitch_off(&self) -> Result<()> {
        if !self.bbswitch.exists() {
            self.modules.load("bbswitch", &[])?;
        }
        self.bbswitch.write("OFF")?;
        if !self.wait_for_bbswitch_state("OFF") {
            return Err(Error::Other("bbswitch did not confirm OFF within the poll budget".into()));
        }
        Ok(())
    }

    fn bbswitch_on(&self) -> Result<()> {
        if !self.bbswitch.exists() {
            self.modules.load("bbswitch", &[])?;
        }
        self.bbswitch.write("ON")?;
        if !self.wait_for_bbswitch_state("ON") {
            return Err(Error::Other("bbswitch did not confirm ON within the poll budget".into()));
        }
        Ok(())
    }

    fn acpi_call_off(&self) -> Result<()> {
        let call = self.acpi_call_off.as_deref().ok_or(Error::NoBackend("acpi_call off string not configured"))?;
        fs::write(self.acpi_call_path(), call)
            .map_err(|source| Error::SysfsWrite { path: self.acpi_call_path().display().to_string(), source })
    }

    fn acpi_call_on(&self) -> Result<()> {
        let call = self.acpi_call_on.as_deref().ok_or(Error::NoBackend("acpi_call on string not configured"))?;
        fs::write(self.acpi_call_path(), call)
            .map_err(|source| Error::SysfsWrite { path: self.acpi_call_path().display().to_string(), source })
    }

    /// Auto priority: runtime PM (verify D3cold) → bbswitch → acpi_call
    fn nvidia_power_off(&self, addr: &str) -> Result<()> {
        match self.backend {
            GpuPowerBackend::RuntimePm => self.runtime_pm_off_verified(addr),
            GpuPowerBackend::Bbswitch => self.bbswitch_off(),
            GpuPowerBackend::AcpiCall => self.acpi_call_off(),
            GpuPowerBackend::Auto => {
                if self.runtime_pm_off_verified(addr).is_ok() {
                    tracing::info!("nvidia: powered off via runtime PM (RTD3 confirmed)");
                    return Ok(());
                }
                tracing::debug!("nvidia: runtime PM did not reach D3cold, falling back to bbswitch");
                if self.bbswitch.exists() || self.modules.load("bbswitch", &[]).is_ok() {
                    match self.bbswitch_off() {
                        Ok(()) => {
                            tracing::info!("nvidia: powered off via bbswitch");
                            return Ok(());
                        }
                        Err(e) => tracing::debug!("nvidia: bbswitch_off failed ({e}), falling back further"),
                    }
                }
                if self.acpi_call_off.is_some() {
                    tracing::debug!("nvidia: bbswitch unavailable/failed, falling back to acpi_call");
                    let result = self.acpi_call_off();
                    if result.is_ok() {
                        tracing::info!("nvidia: powered off via acpi_call");
                    }
                    return result;
                }
                Err(Error::NoBackend(
                    "no power-off mechanism succeeded (tried runtime PM and bbswitch; acpi_call not configured)",
                ))
            }
        }
    }

    /// Auto: prefer bbswitch if present (symmetry with off path), else runtime PM
    fn nvidia_power_on(&self, addr: &str) -> Result<()> {
        match self.backend {
            GpuPowerBackend::RuntimePm => self.runtime_pm_on(addr),
            GpuPowerBackend::Bbswitch => self.bbswitch_on(),
            GpuPowerBackend::AcpiCall => self.acpi_call_on(),
            GpuPowerBackend::Auto => {
                if self.bbswitch.exists() {
                    match self.bbswitch_on() {
                        Ok(()) => {
                            tracing::info!("nvidia: powered on via bbswitch");
                            Ok(())
                        }
                        Err(e) => {
                            tracing::debug!("nvidia: bbswitch_on failed ({e}), falling back to runtime PM");
                            self.runtime_pm_on(addr)
                        }
                    }
                } else {
                    self.runtime_pm_on(addr)
                }
            }
        }
    }

    fn unload_nvidia_stack(&self) -> Result<()> {
        for m in ["nvidia_drm", "nvidia_modeset", "nvidia_uvm", "nvidia"] {
            self.modules.unload(m)?;
        }
        Ok(())
    }

    fn switch_nvidia(&self, mode: GpuMode, driver_pref: GpuDriverPref, addr: &str) -> Result<()> {
        match mode {
            GpuMode::Integrated => {
                self.unload_nvidia_stack()?;
                self.modules.unload("nouveau")?;
                self.nvidia_power_off(addr)
            }
            GpuMode::Discrete | GpuMode::Hybrid => {
                if driver_pref == GpuDriverPref::OpenSource {
                    self.unload_nvidia_stack()?;
                    self.nvidia_power_on(addr)?;
                    self.modules.load("nouveau", &[])
                } else {
                    self.modules.unload("nouveau")?;
                    self.nvidia_power_on(addr)?;
                    self.modules.load("nvidia", &[])?;
                    self.modules.load("nvidia_modeset", &[])?;
                    self.modules.load("nvidia_uvm", &[])?;
                    self.modules.load("nvidia_drm", &["modeset=1"])
                }
            }
        }
    }

    // --- AMD ---

    /// amdgpu is shared between iGPU and dGPU — never unload, only toggle runtime PM
    fn switch_amd(&self, mode: GpuMode, addr: &str) -> Result<()> {
        self.modules.load("amdgpu", &[])?;

        match mode {
            GpuMode::Integrated => {
                let path = self.root.join(format!("sys/bus/pci/devices/{addr}/power/control"));
                fs::write(&path, "auto")
                    .map_err(|source| Error::SysfsWrite { path: path.display().to_string(), source })?;
                if self.wait_for_suspend(addr) {
                    Ok(())
                } else {
                    Err(Error::Other(format!(
                        "amdgpu runtime PM did not reach D3cold on {addr} — no bbswitch/acpi_call fallback for AMD"
                    )))
                }
            }
            GpuMode::Discrete | GpuMode::Hybrid => {
                let path = self.root.join(format!("sys/bus/pci/devices/{addr}/power/control"));
                fs::write(&path, "on").map_err(|source| Error::SysfsWrite { path: path.display().to_string(), source })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashSet;
    use tempfile::TempDir;

    struct FakeModules {
        loaded: RefCell<HashSet<String>>,
        calls: RefCell<Vec<String>>,
        fail_on: Option<String>,
    }

    impl FakeModules {
        fn new(initially_loaded: &[&str]) -> Self {
            Self {
                loaded: RefCell::new(initially_loaded.iter().map(|s| s.to_string()).collect()),
                calls: RefCell::new(Vec::new()),
                fail_on: None,
            }
        }
        fn failing_on(module: &str) -> Self {
            Self { loaded: RefCell::new(HashSet::new()), calls: RefCell::new(Vec::new()), fail_on: Some(module.to_string()) }
        }
    }

    impl ModuleLoader for FakeModules {
        fn is_loaded(&self, module: &str) -> bool {
            self.loaded.borrow().contains(module)
        }
        fn load(&self, module: &str, _args: &[&str]) -> Result<()> {
            self.calls.borrow_mut().push(format!("load {module}"));
            if self.fail_on.as_deref() == Some(module) {
                return Err(Error::Other(format!("simulated failure loading {module}")));
            }
            self.loaded.borrow_mut().insert(module.to_string());
            Ok(())
        }
        fn unload(&self, module: &str) -> Result<()> {
            if !self.is_loaded(module) {
                return Ok(());
            }
            self.calls.borrow_mut().push(format!("unload {module}"));
            if self.fail_on.as_deref() == Some(module) {
                return Err(Error::Other(format!("simulated failure unloading {module}")));
            }
            self.loaded.borrow_mut().remove(module);
            Ok(())
        }
    }

    struct FakeBbswitch {
        state: RefCell<Option<String>>,
        exists: bool,
        settle_after_reads: u32,
        pending: RefCell<Option<(String, u32)>>,
    }

    impl FakeBbswitch {
        fn new(initial: Option<&str>) -> Self {
            Self {
                state: RefCell::new(initial.map(|s| s.to_string())),
                exists: initial.is_some(),
                settle_after_reads: 0,
                pending: RefCell::new(None),
            }
        }
        fn absent() -> Self {
            Self { state: RefCell::new(None), exists: false, settle_after_reads: 0, pending: RefCell::new(None) }
        }
        fn with_settle_delay(initial: Option<&str>, settle_after_reads: u32) -> Self {
            Self {
                state: RefCell::new(initial.map(|s| s.to_string())),
                exists: initial.is_some(),
                settle_after_reads,
                pending: RefCell::new(None),
            }
        }
    }

    impl BbswitchIo for FakeBbswitch {
        fn exists(&self) -> bool {
            self.exists
        }
        fn read_state(&self) -> Option<String> {
            let mut pending = self.pending.borrow_mut();
            if let Some((value, remaining)) = pending.as_mut() {
                if *remaining == 0 {
                    let v = value.clone();
                    *self.state.borrow_mut() = Some(v.clone());
                    *pending = None;
                    return Some(v);
                }
                *remaining -= 1;
            }
            self.state.borrow().clone()
        }
        fn write(&self, value: &str) -> Result<()> {
            if self.settle_after_reads == 0 {
                *self.state.borrow_mut() = Some(value.to_string());
            } else {
                *self.pending.borrow_mut() = Some((value.to_string(), self.settle_after_reads));
            }
            Ok(())
        }
    }

    fn fake_intel_nvidia_root() -> TempDir {
        let tmp = TempDir::new().unwrap();
        let igpu = tmp.path().join("sys/bus/pci/devices/0000:00:02.0");
        fs::create_dir_all(&igpu).unwrap();
        fs::write(igpu.join("vendor"), "0x8086\n").unwrap();
        fs::write(igpu.join("device"), "0x0166\n").unwrap();
        fs::write(igpu.join("class"), "0x030000\n").unwrap();

        let dgpu = tmp.path().join("sys/bus/pci/devices/0000:01:00.0");
        fs::create_dir_all(&dgpu).unwrap();
        fs::write(dgpu.join("vendor"), "0x10de\n").unwrap();
        fs::write(dgpu.join("device"), "0x0fdf\n").unwrap();
        fs::write(dgpu.join("class"), "0x030200\n").unwrap();
        fs::create_dir_all(dgpu.join("power")).unwrap();
        fs::write(dgpu.join("power/control"), "on\n").unwrap();
        fs::write(dgpu.join("power/runtime_status"), "active\n").unwrap();

        tmp
    }

    fn fake_amd_hybrid_root() -> TempDir {
        let tmp = TempDir::new().unwrap();
        let apu = tmp.path().join("sys/bus/pci/devices/0000:00:02.0");
        fs::create_dir_all(&apu).unwrap();
        fs::write(apu.join("vendor"), "0x1002\n").unwrap();
        fs::write(apu.join("device"), "0x1636\n").unwrap();
        fs::write(apu.join("class"), "0x030000\n").unwrap();

        let dgpu = tmp.path().join("sys/bus/pci/devices/0000:03:00.0");
        fs::create_dir_all(dgpu.join("power")).unwrap();
        fs::write(dgpu.join("vendor"), "0x1002\n").unwrap();
        fs::write(dgpu.join("device"), "0x73ff\n").unwrap();
        fs::write(dgpu.join("class"), "0x030000\n").unwrap();
        fs::write(dgpu.join("power/control"), "on\n").unwrap();
        fs::write(dgpu.join("power/runtime_status"), "active\n").unwrap();

        tmp
    }

    fn fake_discrete_only_nvidia_root() -> TempDir {
        let tmp = TempDir::new().unwrap();
        let dgpu = tmp.path().join("sys/bus/pci/devices/0000:01:00.0");
        fs::create_dir_all(dgpu.join("power")).unwrap();
        fs::write(dgpu.join("vendor"), "0x10de\n").unwrap();
        fs::write(dgpu.join("device"), "0x2684\n").unwrap();
        fs::write(dgpu.join("class"), "0x030000\n").unwrap();
        fs::write(dgpu.join("power/control"), "on\n").unwrap();
        fs::write(dgpu.join("power/runtime_status"), "active\n").unwrap();
        tmp
    }

    fn mgr_with<'a>(
        tmp: &TempDir,
        backend: GpuPowerBackend,
        modules: FakeModules,
        bbswitch: FakeBbswitch,
    ) -> GpuManager<FakeModules, FakeBbswitch> {
        GpuManager::new(tmp.path(), backend, modules, bbswitch, None, None).with_fast_poll()
    }

    #[test]
    fn detects_nvidia_vendor_via_bus_heuristic() {
        let tmp = fake_intel_nvidia_root();
        let mgr = mgr_with(&tmp, GpuPowerBackend::Auto, FakeModules::new(&[]), FakeBbswitch::absent());
        assert_eq!(mgr.detect_vendor(), Some(GpuVendor::Nvidia));
        assert!(mgr.has_integrated_gpu());
    }

    #[test]
    fn detects_amd_vendor_even_though_igpu_is_also_amd() {
        let tmp = fake_amd_hybrid_root();
        let mgr = mgr_with(&tmp, GpuPowerBackend::Auto, FakeModules::new(&[]), FakeBbswitch::absent());
        assert_eq!(mgr.detect_vendor(), Some(GpuVendor::Amd));
        assert!(mgr.has_integrated_gpu());
    }

    #[test]
    fn discrete_only_system_has_no_integrated_gpu() {
        let tmp = fake_discrete_only_nvidia_root();
        let mgr = mgr_with(&tmp, GpuPowerBackend::Auto, FakeModules::new(&[]), FakeBbswitch::absent());
        assert!(!mgr.has_integrated_gpu());
        assert_eq!(mgr.detect_vendor(), Some(GpuVendor::Nvidia));
    }

    #[test]
    fn integrated_mode_errors_on_discrete_only_system() {
        let tmp = fake_discrete_only_nvidia_root();
        let mgr = mgr_with(&tmp, GpuPowerBackend::Auto, FakeModules::new(&["nvidia"]), FakeBbswitch::absent());
        let result = mgr.switch_to(GpuMode::Integrated, GpuDriverPref::Auto);
        assert!(result.is_err());
    }

    #[test]
    fn nvidia_auto_prefers_runtime_pm_when_it_confirms_suspend() {
        let tmp = fake_intel_nvidia_root();
        fs::write(tmp.path().join("sys/bus/pci/devices/0000:01:00.0/power/runtime_status"), "suspended\n").unwrap();
        let modules = FakeModules::new(&["nvidia", "nvidia_modeset", "nvidia_uvm", "nvidia_drm"]);
        let mgr = mgr_with(&tmp, GpuPowerBackend::Auto, modules, FakeBbswitch::absent());

        mgr.switch_to(GpuMode::Integrated, GpuDriverPref::Auto).unwrap();

        assert!(!mgr.bbswitch.exists());
        assert!(mgr.modules.calls.borrow().iter().all(|c| !c.contains("bbswitch")));
    }

    #[test]
    fn nvidia_auto_falls_back_to_bbswitch_when_runtime_pm_unconfirmed() {
        let tmp = fake_intel_nvidia_root();
        let modules = FakeModules::new(&["nvidia"]);
        let bbswitch = FakeBbswitch::new(Some("ON"));
        let mgr = mgr_with(&tmp, GpuPowerBackend::Auto, modules, bbswitch);

        mgr.switch_to(GpuMode::Integrated, GpuDriverPref::Auto).unwrap();

        assert_eq!(mgr.bbswitch.read_state().as_deref(), Some("OFF"));
    }

    #[test]
    fn bbswitch_off_polls_through_acpi_settling_lag() {
        let tmp = fake_intel_nvidia_root();
        let modules = FakeModules::new(&["nvidia"]);
        let bbswitch = FakeBbswitch::with_settle_delay(Some("ON"), 3);
        let mgr = GpuManager::new(tmp.path(), GpuPowerBackend::Auto, modules, bbswitch, None, None)
            .with_test_poll(10, Duration::ZERO);

        mgr.switch_to(GpuMode::Integrated, GpuDriverPref::Auto).unwrap();

        assert_eq!(mgr.bbswitch.read_state().as_deref(), Some("OFF"));
    }

    #[test]
    fn bbswitch_off_still_fails_if_never_settles() {
        let tmp = fake_intel_nvidia_root();
        let modules = FakeModules::new(&["nvidia"]);
        let bbswitch = FakeBbswitch::with_settle_delay(Some("ON"), 1000);
        let mgr = GpuManager::new(tmp.path(), GpuPowerBackend::Auto, modules, bbswitch, None, None)
            .with_test_poll(5, Duration::ZERO);

        let result = mgr.switch_to(GpuMode::Integrated, GpuDriverPref::Auto);
        assert!(result.is_err());
    }

    #[test]
    fn nvidia_power_on_bbswitch_failure_falls_back_to_runtime_pm() {
        let tmp = fake_intel_nvidia_root();
        fs::write(tmp.path().join("sys/bus/pci/devices/0000:01:00.0/power/control"), "auto\n").unwrap();
        let bbswitch = FakeBbswitch::with_settle_delay(Some("OFF"), 1000);
        let mgr = GpuManager::new(tmp.path(), GpuPowerBackend::Auto, FakeModules::new(&[]), bbswitch, None, None)
            .with_test_poll(3, Duration::ZERO);

        mgr.switch_to(GpuMode::Hybrid, GpuDriverPref::Auto).unwrap();

        let control =
            fs::read_to_string(tmp.path().join("sys/bus/pci/devices/0000:01:00.0/power/control")).unwrap();
        assert_eq!(control.trim(), "on");
    }

    #[test]
    fn nvidia_driver_pref_open_source_loads_nouveau() {
        let tmp = fake_intel_nvidia_root();
        fs::write(tmp.path().join("sys/bus/pci/devices/0000:01:00.0/power/runtime_status"), "suspended\n").unwrap();
        let modules = FakeModules::new(&[]);
        let mgr = mgr_with(&tmp, GpuPowerBackend::Auto, modules, FakeBbswitch::absent());

        mgr.switch_to(GpuMode::Hybrid, GpuDriverPref::OpenSource).unwrap();

        assert!(mgr.modules.is_loaded("nouveau"));
        assert!(!mgr.modules.is_loaded("nvidia"));
    }

    #[test]
    fn nvidia_driver_pref_auto_loads_proprietary_stack_in_order() {
        let tmp = fake_intel_nvidia_root();
        let modules = FakeModules::new(&[]);
        let mgr = mgr_with(&tmp, GpuPowerBackend::Auto, modules, FakeBbswitch::absent());

        mgr.switch_to(GpuMode::Hybrid, GpuDriverPref::Auto).unwrap();

        assert_eq!(
            *mgr.modules.calls.borrow(),
            vec!["load nvidia", "load nvidia_modeset", "load nvidia_uvm", "load nvidia_drm"]
        );
    }

    #[test]
    fn amd_integrated_uses_runtime_pm_never_unloads_shared_module() {
        let tmp = fake_amd_hybrid_root();
        fs::write(tmp.path().join("sys/bus/pci/devices/0000:03:00.0/power/runtime_status"), "suspended\n").unwrap();
        let modules = FakeModules::new(&["amdgpu"]);
        let mgr = mgr_with(&tmp, GpuPowerBackend::Auto, modules, FakeBbswitch::absent());

        mgr.switch_to(GpuMode::Integrated, GpuDriverPref::Auto).unwrap();

        assert!(mgr.modules.calls.borrow().iter().all(|c| c != "unload amdgpu"));
        let control =
            fs::read_to_string(tmp.path().join("sys/bus/pci/devices/0000:03:00.0/power/control")).unwrap();
        assert_eq!(control.trim(), "auto");
    }

    #[test]
    fn amd_integrated_errors_when_runtime_pm_never_confirms_suspend() {
        let tmp = fake_amd_hybrid_root();
        let modules = FakeModules::new(&["amdgpu"]);
        let mgr = mgr_with(&tmp, GpuPowerBackend::Auto, modules, FakeBbswitch::absent());

        let result = mgr.switch_to(GpuMode::Integrated, GpuDriverPref::Auto);
        assert!(result.is_err());
    }

    #[test]
    fn amd_hybrid_forces_device_power_control_on() {
        let tmp = fake_amd_hybrid_root();
        let modules = FakeModules::new(&["amdgpu"]);
        let mgr = mgr_with(&tmp, GpuPowerBackend::Auto, modules, FakeBbswitch::absent());

        mgr.switch_to(GpuMode::Hybrid, GpuDriverPref::Auto).unwrap();

        let control =
            fs::read_to_string(tmp.path().join("sys/bus/pci/devices/0000:03:00.0/power/control")).unwrap();
        assert_eq!(control.trim(), "on");
    }

    #[test]
    fn discrete_mode_on_muxless_hardware_succeeds_like_hybrid() {
        let tmp = fake_intel_nvidia_root();
        let modules = FakeModules::new(&[]);
        let mgr = mgr_with(&tmp, GpuPowerBackend::Auto, modules, FakeBbswitch::absent());

        mgr.switch_to(GpuMode::Discrete, GpuDriverPref::Auto).unwrap();
        assert!(mgr.modules.is_loaded("nvidia"));
    }

    #[test]
    fn failure_partway_through_propagates() {
        let tmp = fake_intel_nvidia_root();
        let modules = FakeModules::failing_on("nvidia_uvm");
        let mgr = mgr_with(&tmp, GpuPowerBackend::Auto, modules, FakeBbswitch::new(Some("OFF")));
        let result = mgr.switch_to(GpuMode::Hybrid, GpuDriverPref::Auto);
        assert!(result.is_err());
    }

    #[test]
    fn list_devices_finds_both_gpus() {
        let tmp = fake_intel_nvidia_root();
        let mgr = mgr_with(&tmp, GpuPowerBackend::Auto, FakeModules::new(&[]), FakeBbswitch::absent());
        let devices = mgr.list_devices();
        assert_eq!(devices.len(), 2);
        assert!(devices.iter().any(|d| d.vendor == "0x10de"));
        assert!(devices.iter().any(|d| d.vendor == "0x8086"));
    }

    #[test]
    fn acpi_call_backend_without_config_errors_cleanly() {
        let tmp = fake_intel_nvidia_root();
        let modules = FakeModules::new(&[]);
        let mgr = mgr_with(&tmp, GpuPowerBackend::AcpiCall, modules, FakeBbswitch::absent());
        let result = mgr.switch_to(GpuMode::Integrated, GpuDriverPref::Auto);
        assert!(result.is_err());
    }

    #[test]
    fn system_bbswitch_write_then_readback() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("proc/acpi")).unwrap();
        fs::write(tmp.path().join("proc/acpi/bbswitch"), "0000:01:00.0 ON\n").unwrap();
        let sys = SystemBbswitch::new(tmp.path());
        assert_eq!(sys.read_state().as_deref(), Some("ON"));

        sys.write("OFF").unwrap();
        assert_eq!(sys.read_state(), None, "plain file doesn't reformat like kernel — expected in fixture");
    }
}

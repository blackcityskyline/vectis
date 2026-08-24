use crate::backends::gpu::{GpuManager, SystemBbswitch, SystemModprobe};
use crate::backends::rapl::RaplManager;
use crate::backends::tlp::{SystemCommandRunner, TlpRunner};
use crate::state::StateStore;
use crate::{conflicts, dm, proc_kill, session};
use vectis_common::config::{Config, PowerBackendKind};
use vectis_common::types::{DaemonStatus, GpuDriverPref, GpuMode, Profile, TdpLimit};
use std::str::FromStr;
use std::sync::RwLock;
use tokio::sync::Mutex as AsyncMutex;
use zbus::dbus_interface;
use zbus::Connection;

pub struct DaemonService {
    config: RwLock<Config>,
    state: StateStore,
    connection: Connection,
    /// Serializes GPU code paths. Prevents background reconcile from racing
    /// foreground force-switch (SessionRemoved signal fires mid-force, background
    /// sees transient GPU state and "self-heals" back to old mode).
    gpu_lock: AsyncMutex<()>,
}

type SysGpuManager = GpuManager<SystemModprobe, SystemBbswitch>;

impl DaemonService {
    pub fn new(config: Config, state: StateStore, connection: Connection) -> Self {
        Self { config: RwLock::new(config), state, connection, gpu_lock: AsyncMutex::new(()) }
    }

    fn gpu_manager(&self) -> SysGpuManager {
        let cfg = self.config.read().unwrap();
        GpuManager::new(
            "/",
            cfg.gpu.backend,
            SystemModprobe::new("/"),
            SystemBbswitch::new("/"),
            cfg.gpu.acpi_call_off.clone(),
            cfg.gpu.acpi_call_on.clone(),
        )
    }

    fn rapl_manager(&self) -> Option<RaplManager> {
        RaplManager::detect("/").ok()
    }

    /// Auto probes tlp → ppd → asusd. Defaults to Tlp if neither found.
    async fn resolve_power_backend(&self) -> PowerBackendKind {
        let configured = self.config.read().unwrap().power_backend;
        match configured {
            PowerBackendKind::Auto => {
                let manager = match zbus::Proxy::new(
                    &self.connection,
                    "org.freedesktop.systemd1",
                    "/org/freedesktop/systemd1",
                    "org.freedesktop.systemd1.Manager",
                )
                .await
                {
                    Ok(p) => p,
                    Err(_) => return PowerBackendKind::Tlp,
                };

                let unit_installed = |name: &'static str| {
                    let manager = &manager;
                    async move {
                        manager.call::<_, _, zbus::zvariant::OwnedObjectPath>("LoadUnit", &(name,)).await.is_ok()
                    }
                };

                if unit_installed("tlp.service").await {
                    return PowerBackendKind::Tlp;
                }
                if unit_installed("power-profiles-daemon.service").await || unit_installed("asusd.service").await {
                    return PowerBackendKind::Ppd;
                }
                PowerBackendKind::Tlp
            }
            other => other,
        }
    }

    async fn apply_profile_inner(&self, profile_name: &str) -> vectis_common::Result<()> {
        let profile = Profile::from_str(profile_name).map_err(vectis_common::Error::Other)?;

        match self.resolve_power_backend().await {
            PowerBackendKind::Tlp | PowerBackendKind::Auto => {
                TlpRunner::new(SystemCommandRunner).apply_profile(profile)?;
            }
            PowerBackendKind::Ppd => {
                crate::backends::ppd::set_profile(&self.connection, profile).await?;
            }
        }

        let mut state = self.state.load();
        state.last_profile = Some(profile);
        self.state.save(&state)?;
        Ok(())
    }

    async fn apply_tdp_inner(&self, limit: TdpLimit) -> vectis_common::Result<()> {
        let rapl = self.rapl_manager().ok_or(vectis_common::Error::NoBackend("RAPL powercap"))?;
        rapl.apply(&limit)
    }

    /// Immediate GPU switch: stop DM → kill holders → switch → restart DM.
    /// Holds gpu_lock for entire operation to prevent background revert.
    async fn apply_gpu_force(&self, mode: GpuMode, driver: GpuDriverPref) -> vectis_common::Result<()> {
        let _guard = self.gpu_lock.lock().await;

        let active_dm = dm::detect(&self.connection).await;
        if let Some(unit) = &active_dm {
            let _ = dm::stop(&self.connection, unit).await;
        }
        proc_kill::kill_gpu_holders();

        let gpu = self.gpu_manager();
        let result = gpu.switch_to(mode, driver);

        if let Some(unit) = &active_dm {
            let _ = dm::start(&self.connection, unit).await;
        }

        result?;
        let mut state = self.state.load();
        state.last_gpu_mode = Some(mode);
        state.last_gpu_driver = driver;
        state.pending_gpu_mode = None;
        state.last_auto_apply_error = None;
        self.state.save(&state)?;
        Ok(())
    }

    /// Background path: try_lock, skip if foreground is busy.
    pub async fn apply_pending_gpu_switch_if_ready(&self) {
        let Ok(_guard) = self.gpu_lock.try_lock() else {
            tracing::debug!("apply_pending_gpu_switch_if_ready: gpu_lock busy, skipping");
            return;
        };
        self.apply_pending_gpu_switch_if_ready_locked().await;
    }

    /// Apply pending switch or enforce last known mode.
    /// Caller must hold gpu_lock.
    async fn apply_pending_gpu_switch_if_ready_locked(&self) {
        let state = self.state.load();
        let Some(target) = state.pending_gpu_mode else {
            tracing::debug!("apply_pending_gpu_switch_if_ready: nothing pending");
            self.enforce_last_gpu_mode(&state).await;
            return;
        };
        let driver = state.pending_gpu_driver;

        let _ = session::no_active_user_sessions(&self.connection).await;

        if proc_kill::nvidia_device_in_use() {
            tracing::debug!("apply_pending_gpu_switch_if_ready: pending={target}, but /dev/nvidia* held — not applying yet");
            return;
        }

        tracing::info!("apply_pending_gpu_switch_if_ready: applying pending switch to {target}");
        proc_kill::kill_nvidia_holders_only();
        let gpu = self.gpu_manager();
        match gpu.switch_to(target, driver) {
            Ok(()) => {
                tracing::info!("apply_pending_gpu_switch_if_ready: switch to {target} succeeded");
                let mut s = self.state.load();
                s.last_gpu_mode = Some(target);
                s.last_gpu_driver = driver;
                s.pending_gpu_mode = None;
                s.last_auto_apply_error = None;
                let _ = self.state.save(&s);
            }
            Err(e) => {
                tracing::warn!("apply_pending_gpu_switch_if_ready: switch to {target} FAILED: {e}");
                let mut s = self.state.load();
                s.last_auto_apply_error = Some(format!("{e}"));
                let _ = self.state.save(&s);
            }
        }
    }

    /// Verify live GPU matches expected, self-heal if drifted.
    /// Caller must hold gpu_lock.
    async fn enforce_last_gpu_mode(&self, state: &crate::state::State) {
        let Some(expected) = state.last_gpu_mode else {
            tracing::debug!("enforce_last_gpu_mode: no last_gpu_mode recorded");
            return;
        };

        let gpu = self.gpu_manager();
        let live = gpu.detect_mode();
        if live == Some(expected) {
            tracing::trace!("enforce_last_gpu_mode: live ({live:?}) matches expected ({expected})");
            return;
        }

        // Kill nvidia holders BEFORE checking device liveness — the nvidia driver
        // itself holds devices after logout (not userspace processes). Killing first
        // gives the driver a chance to release.
        tracing::warn!("enforce_last_gpu_mode: live={live:?}, expected {expected} — re-asserting");
        proc_kill::kill_nvidia_holders_only();

        if proc_kill::nvidia_device_in_use() {
            tracing::debug!("enforce_last_gpu_mode: live={live:?} expected={expected}, but /dev/nvidia* still in use after kill");
            return;
        }
        match gpu.switch_to(expected, state.last_gpu_driver) {
            Ok(()) => {
                tracing::info!("enforce_last_gpu_mode: re-asserted {expected} successfully");
                let mut s = self.state.load();
                s.last_auto_apply_error = None;
                let _ = self.state.save(&s);
            }
            Err(e) => {
                tracing::warn!("enforce_last_gpu_mode: failed to re-assert {expected}: {e}");
                let mut s = self.state.load();
                s.last_auto_apply_error = Some(format!("drift-correction failed: {e}"));
                let _ = self.state.save(&s);
            }
        }
    }

    /// Startup: apply pending or enforce last mode. Runs before DM starts.
    pub async fn reconcile_at_startup(&self) {
        let _guard = self.gpu_lock.lock().await;
        let state = self.state.load();

        if state.pending_gpu_mode.is_some() {
            tracing::info!("reconcile_at_startup: pending switch found, applying");
            self.apply_pending_gpu_switch_if_ready_locked().await;
            return;
        }

        self.enforce_last_gpu_mode(&state).await;
    }

    async fn status_snapshot(&self, conflicts: Vec<String>) -> DaemonStatus {
        let rapl = self.rapl_manager();
        let gpu = self.gpu_manager();
        let state = self.state.load();
        let resolved_backend = self.resolve_power_backend().await;

        let active_profile = match resolved_backend {
            PowerBackendKind::Ppd => {
                crate::backends::ppd::get_active_profile(&self.connection).await.or(state.last_profile)
            }
            _ => state.last_profile,
        };

        DaemonStatus {
            active_profile,
            power_backend: format!("{resolved_backend:?}").to_lowercase(),
            tlp_service_active: conflicts.iter().any(|s| s == "tlp.service")
                || matches!(resolved_backend, PowerBackendKind::Tlp),
            tdp_pl1_watts: rapl.as_ref().and_then(|r| r.current_pl1_watts()),
            tdp_pl2_watts: rapl.as_ref().and_then(|r| r.current_pl2_watts()),
            gpu_mode: gpu.detect_mode(),
            gpu_power_state: format!("{:?}", gpu.card_power()),
            gpu_pending: state.pending_gpu_mode,
            gpu_last_auto_apply_error: state.last_auto_apply_error.clone(),
            conflicting_services: conflicts,
        }
    }
}

// D-Bus public methods
impl DaemonService {
    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    pub async fn get_status(&self) -> zbus::fdo::Result<String> {
        let watch_list = self.config.read().unwrap().conflicts.watch_services.clone();
        let active_conflicts = conflicts::active_conflicts(&self.connection, &watch_list).await;
        let snapshot = self.status_snapshot(active_conflicts).await;
        serde_json::to_string(&snapshot).map_err(|e| zbus::fdo::Error::Failed(format!("serialize status: {e}")))
    }

    pub async fn apply_profile(&self, profile: &str) -> zbus::fdo::Result<()> {
        self.apply_profile_inner(profile).await.map_err(Into::into)
    }

    pub async fn set_tdp(&self, pl1_watts: u32, pl2_watts: u32, tau_us: u32, hard_lock: bool) -> zbus::fdo::Result<()> {
        let limit = TdpLimit {
            pl1_watts: (pl1_watts > 0).then_some(pl1_watts),
            pl2_watts: (pl2_watts > 0).then_some(pl2_watts),
            tau_us: (tau_us > 0).then_some(tau_us),
            hard_lock,
        };
        self.apply_tdp_inner(limit).await.map_err(Into::into)
    }

    pub async fn clear_tdp(&self) -> zbus::fdo::Result<()> {
        let rapl = self.rapl_manager().ok_or(zbus::fdo::Error::Failed("RAPL not supported on this system".into()))?;
        rapl.clear().map_err(Into::into)
    }

    pub async fn set_gpu_mode(&self, mode: &str, driver: &str, force: bool) -> zbus::fdo::Result<()> {
        let target = GpuMode::from_str(mode).map_err(zbus::fdo::Error::Failed)?;
        let driver_pref = GpuDriverPref::from_str(driver).map_err(zbus::fdo::Error::Failed)?;

        // Short-circuit: skip if already in requested mode+driver
        if let Ok(_guard) = self.gpu_lock.try_lock() {
            let current = self.gpu_manager().detect_mode();
            let state_now = self.state.load();
            if current == Some(target) && state_now.last_gpu_driver == driver_pref {
                let mut state = state_now;
                state.last_gpu_mode = Some(target);
                state.pending_gpu_mode = None;
                let _ = self.state.save(&state);
                return Ok(());
            }
        }

        if force {
            self.apply_gpu_force(target, driver_pref).await.map_err(Into::into)
        } else {
            let mut state = self.state.load();
            state.pending_gpu_mode = Some(target);
            state.pending_gpu_driver = driver_pref;
            self.state.save(&state).map_err(Into::into)
        }
    }

    pub async fn list_gpu_devices(&self) -> zbus::fdo::Result<String> {
        let gpu = self.gpu_manager();
        let devices = gpu.list_devices();
        serde_json::to_string(
            &devices
                .into_iter()
                .map(|d| {
                    serde_json::json!({
                        "address": d.address,
                        "vendor": d.vendor,
                        "device": d.device,
                        "class": d.class,
                        "driver": d.driver,
                    })
                })
                .collect::<Vec<_>>(),
        )
        .map_err(|e| zbus::fdo::Error::Failed(format!("serialize devices: {e}")))
    }

    pub async fn get_conflicts(&self) -> zbus::fdo::Result<Vec<String>> {
        let watch_list = self.config.read().unwrap().conflicts.watch_services.clone();
        Ok(conflicts::active_conflicts(&self.connection, &watch_list).await)
    }
}

/// D-Bus-facing wrapper with polkit authorization
pub struct DaemonServiceIface(pub std::sync::Arc<DaemonService>);

#[dbus_interface(name = "org.vectis.Daemon1")]
impl DaemonServiceIface {
    async fn get_status(&self) -> zbus::fdo::Result<String> {
        self.0.get_status().await
    }

    async fn apply_profile(&self, profile: &str, #[zbus(header)] hdr: zbus::MessageHeader<'_>) -> zbus::fdo::Result<()> {
        self.authorize(&hdr, "org.vectis.apply-profile").await?;
        self.0.apply_profile(profile).await
    }

    async fn set_tdp(
        &self,
        pl1_watts: u32,
        pl2_watts: u32,
        tau_us: u32,
        hard_lock: bool,
        #[zbus(header)] hdr: zbus::MessageHeader<'_>,
    ) -> zbus::fdo::Result<()> {
        self.authorize(&hdr, "org.vectis.set-tdp").await?;
        self.0.set_tdp(pl1_watts, pl2_watts, tau_us, hard_lock).await
    }

    async fn clear_tdp(&self, #[zbus(header)] hdr: zbus::MessageHeader<'_>) -> zbus::fdo::Result<()> {
        self.authorize(&hdr, "org.vectis.set-tdp").await?;
        self.0.clear_tdp().await
    }

    async fn set_gpu_mode(
        &self,
        mode: &str,
        driver: &str,
        force: bool,
        #[zbus(header)] hdr: zbus::MessageHeader<'_>,
    ) -> zbus::fdo::Result<()> {
        self.authorize(&hdr, "org.vectis.set-gpu-mode").await?;
        self.0.set_gpu_mode(mode, driver, force).await
    }

    async fn list_gpu_devices(&self) -> zbus::fdo::Result<String> {
        self.0.list_gpu_devices().await
    }
    async fn get_conflicts(&self) -> zbus::fdo::Result<Vec<String>> {
        self.0.get_conflicts().await
    }
}

impl DaemonServiceIface {
    async fn authorize(&self, hdr: &zbus::MessageHeader<'_>, action_id: &str) -> zbus::fdo::Result<()> {
        let sender = hdr
            .sender()
            .ok()
            .flatten()
            .map(|s| s.to_string())
            .ok_or_else(|| zbus::fdo::Error::AccessDenied("no caller identity on message".into()))?;

        if crate::polkit::check_authorization(self.0.connection(), &sender, action_id).await {
            Ok(())
        } else {
            Err(zbus::fdo::Error::AccessDenied(format!("not authorized for {action_id}")))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::Mutex as AsyncMutex;

    #[tokio::test]
    async fn gpu_lock_prevents_background_revert_during_foreground_switch() {
        let gpu_lock = Arc::new(AsyncMutex::new(()));

        let fg_lock = gpu_lock.clone();
        let fg_handle = tokio::spawn(async move {
            let _guard = fg_lock.lock().await;
            tokio::time::sleep(Duration::from_millis(200)).await;
        });

        tokio::time::sleep(Duration::from_millis(10)).await;

        let mut skipped_while_locked = 0u32;
        let mut succeeded_after_release = false;

        for _ in 0..100 {
            if let Ok(_guard) = gpu_lock.try_lock() {
                succeeded_after_release = true;
                break;
            }
            skipped_while_locked += 1;
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        fg_handle.await.unwrap();

        assert!(
            skipped_while_locked > 0,
            "background try_lock should fail while foreground holds lock"
        );
        assert!(
            succeeded_after_release,
            "background try_lock should succeed after foreground releases"
        );
    }

    #[tokio::test]
    async fn gpu_lock_releases_cleanly_after_foreground_finishes() {
        let gpu_lock = Arc::new(AsyncMutex::new(()));

        {
            let _guard = gpu_lock.lock().await;
        }

        let guard = gpu_lock.try_lock();
        assert!(guard.is_ok(), "gpu_lock must be available after foreground releases it");
    }
}

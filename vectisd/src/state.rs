use vectis_common::error::Result;
use vectis_common::types::{GpuDriverPref, GpuMode, Profile};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const DEFAULT_STATE_DIR: &str = "/var/lib/vectis";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct State {
    pub last_gpu_mode: Option<GpuMode>,
    pub last_gpu_driver: GpuDriverPref,
    pub pending_gpu_mode: Option<GpuMode>,
    pub pending_gpu_driver: GpuDriverPref,
    pub last_profile: Option<Profile>,
    pub last_auto_apply_error: Option<String>,
}

impl State {
    /// Drops a queued GPU switch. Returns whether one was queued.
    ///
    /// `last_gpu_mode` is deliberately left alone: the background path falls
    /// through to `enforce_last_gpu_mode` once nothing is pending, so the
    /// current mode keeps being re-asserted instead of going ungoverned.
    pub fn clear_gpu_pending(&mut self) -> bool {
        let was_queued = self.pending_gpu_mode.take().is_some();
        // The driver preference has no meaning without a pending mode, so a
        // stale one would be inherited by whatever switch is queued next.
        self.pending_gpu_driver = GpuDriverPref::Auto;
        was_queued
    }
}

pub struct StateStore {
    path: PathBuf,
}

impl StateStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { path: dir.into().join("state.json") }
    }

    pub fn load(&self) -> State {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, state: &State) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(state)
            .map_err(|e| vectis_common::error::Error::Other(format!("state serialize error: {e}")))?;
        std::fs::write(&self.path, text)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn clearing_a_pending_switch_reports_that_there_was_one() {
        let mut state = State::default();
        state.pending_gpu_mode = Some(GpuMode::Discrete);
        state.pending_gpu_driver = GpuDriverPref::OpenSource;

        assert!(state.clear_gpu_pending(), "a queued switch existed");
        assert!(state.pending_gpu_mode.is_none());
        // The driver preference only exists to be applied with the pending mode,
        // so leaving it behind would let a later switch inherit it silently.
        assert_eq!(state.pending_gpu_driver, GpuDriverPref::Auto);
    }

    #[test]
    fn clearing_nothing_is_not_an_error() {
        let mut state = State::default();
        assert!(!state.clear_gpu_pending(), "nothing was queued");
        // last_gpu_mode must survive: the daemon enforces it instead, and losing
        // it would drop the laptop back to no enforced mode at all.
        state.last_gpu_mode = Some(GpuMode::Integrated);
        assert!(!state.clear_gpu_pending());
        assert_eq!(state.last_gpu_mode, Some(GpuMode::Integrated));
    }

    #[test]
    fn clearing_does_not_touch_the_enforced_mode() {
        let mut state = State::default();
        state.last_gpu_mode = Some(GpuMode::Hybrid);
        state.pending_gpu_mode = Some(GpuMode::Discrete);

        state.clear_gpu_pending();

        assert_eq!(state.last_gpu_mode, Some(GpuMode::Hybrid));
    }

    #[test]
    fn missing_file_gives_default_state() {
        let tmp = TempDir::new().unwrap();
        let store = StateStore::new(tmp.path());
        let state = store.load();
        assert!(state.pending_gpu_mode.is_none());
    }

    #[test]
    fn save_then_load_roundtrips() {
        let tmp = TempDir::new().unwrap();
        let store = StateStore::new(tmp.path());
        let mut state = State::default();
        state.pending_gpu_mode = Some(GpuMode::Hybrid);
        state.pending_gpu_driver = GpuDriverPref::OpenSource;
        state.last_profile = Some(Profile::Balanced);
        state.last_auto_apply_error = Some("boom".to_string());
        store.save(&state).unwrap();

        let loaded = store.load();
        assert_eq!(loaded.pending_gpu_mode, Some(GpuMode::Hybrid));
        assert_eq!(loaded.pending_gpu_driver, GpuDriverPref::OpenSource);
        assert_eq!(loaded.last_profile, Some(Profile::Balanced));
        assert_eq!(loaded.last_auto_apply_error.as_deref(), Some("boom"));
    }

    #[test]
    fn corrupt_file_falls_back_to_default() {
        let tmp = TempDir::new().unwrap();
        let store = StateStore::new(tmp.path());
        std::fs::write(tmp.path().join("state.json"), "not json at all").unwrap();
        let loaded = store.load();
        assert!(loaded.last_profile.is_none());
    }
}

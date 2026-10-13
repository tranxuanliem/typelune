//! Fallback for non-macOS targets: native expansion is unavailable.

use super::{NativeState, NativeStats, Permissions};
use crate::model::Library;

#[derive(Default)]
pub struct NativeController;

impl NativeController {
    pub fn new() -> Self {
        Self
    }
    pub fn permissions(&self) -> Permissions {
        Permissions::default()
    }
    pub fn start(&mut self, _library: &Library) -> Result<(), String> {
        Err("System-wide expansion is only implemented for macOS".into())
    }
    pub fn stop(&mut self) {}
    pub fn begin_stop(&mut self) -> Option<PendingStop> {
        None
    }
    pub fn finish_stop(&mut self, _outcome: StopOutcome) {}
    pub fn is_stopping(&self) -> bool {
        false
    }
    pub fn set_paused(&mut self, _paused: bool) {}
    pub fn update_library(&self, _library: &Library) {}
    pub fn refresh_keyboard_layout(&self) {}
    pub fn state(&self) -> NativeState {
        NativeState::Off
    }
    pub fn stats(&self) -> NativeStats {
        NativeStats::default()
    }
}

pub fn open_accessibility_settings() -> Result<(), String> {
    Err("Not supported on this platform".into())
}

pub struct PendingStop;
pub struct StopOutcome;

impl PendingStop {
    pub fn wait(self) -> StopOutcome {
        StopOutcome
    }
}

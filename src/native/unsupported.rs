//! Fallback for non-macOS targets: native expansion is unavailable.

use super::{CapturedTarget, NativeState, NativeStats, Permissions};
use crate::model::{Library, Snippet};
use std::collections::HashSet;

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
    pub fn set_exclusions(&self, _excluded: HashSet<String>) {}
    pub fn set_frontmost(&self, _pid: i32) {}
    pub fn capture_target(&self) -> Result<CapturedTarget, String> {
        Err("Quick Search insertion is only implemented for macOS".into())
    }
    pub fn release_target(&self) {}
    pub fn set_inhibited(&self, _on: bool) {}
    pub fn is_inhibited(&self) -> bool {
        false
    }
    pub fn quick_insert(&mut self, _token: u64, _snippet: &Snippet) -> Result<(), String> {
        Err("Quick Search insertion is only implemented for macOS".into())
    }
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

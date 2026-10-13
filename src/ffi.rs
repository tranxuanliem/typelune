//! Small, explicit C ABI used by the native SwiftUI shell (`include/typelune_core.h`).
//!
//! Surface (all `extern "C"`, all panic-guarded):
//! * `tpl_abi_version()`                  – integer ABI version.
//! * `tpl_core_open(data_dir)`            – load (or create) the library; never starts input
//!   monitoring. `data_dir` may be NULL (default Application Support location).
//! * `tpl_core_call(core, request_json)`  – one JSON command in, one JSON reply out.
//! * `tpl_core_close(core)`               – stop system-wide expansion, flush saves, free.
//! * `tpl_string_free(s)`                 – free a string returned by `tpl_core_call`.
//!
//! Ownership: the core pointer is owned by the caller between open and close (close exactly
//! once). Returned strings are owned by the caller and must be freed with `tpl_string_free`.
//! Replies are always `{"ok":true,"result":…}` or `{"ok":false,"error":"…"}`; a Rust panic is
//! caught and reported as an error instead of unwinding into Swift.
//!
//! Threading: the core is internally locked, so calls may come from any thread, except
//! `native_start` and `native_refresh_layout`, which must run on the main thread (Carbon
//! TextInputSources contract). That is checked and reported, never assumed.

use crate::backup::{self, BackupKind};
use crate::csv_import;
use crate::engine::{self, Engine, KeyInput, Lint};
use crate::migrate::{self, MigrationOutcome};
use crate::model::Library;
use crate::native::{self, NativeController, NativeState};
use crate::prefs::{Preferences, PrefsLoad, PrefsStore};
use crate::saver::{SaveStatus, SaveWorker};
use crate::store::{self, DataLocation, LoadOutcome, Store};
use crate::template::{self, RenderContext};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::ffi::{c_char, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

pub const ABI_VERSION: u32 = 2;

/// Opaque handle given to Swift.
pub struct TplCore {
    state: Mutex<CoreState>,
    store: Store,
    prefs_store: PrefsStore,
    load: Value,
    /// Opened with an explicit data folder (`TYPELUNE_DATA_DIR` or a test folder).
    isolated: bool,
}

struct CoreState {
    library: Library,
    playground: Engine,
    native: NativeController,
    /// `Err` holds the reason the save thread could not start; edits are then refused.
    saver: Result<SaveWorker, String>,
    prefs: Preferences,
    prefs_load: PrefsLoad,
}

#[derive(Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum Request {
    State,
    UpdateLibrary { library: Library },
    ReplaceLibrary { library: Library },
    MergeImport { library: Library },
    ParseImport { path: String },
    Export { path: String },
    Render { body: String },
    PlaygroundReset,
    PlaygroundEdit { backspaces: usize, inserted: String },
    SaveStatus,
    Flush { timeout_ms: u64 },
    NativeStatus,
    NativeStart,
    NativePause,
    NativeResume,
    NativeStop,
    NativeRefreshLayout,
    OpenAccessibilitySettings,
    PreferencesGet,
    PreferencesSet { preferences: Preferences },
    BackupList,
    BackupNow,
    BackupRead { name: String },
    BackupRestore { name: String },
    ParseCsvImport { path: String },
    NativeSetFrontmost { pid: i32 },
    NativeCaptureTarget,
    NativeReleaseTarget,
    NativeInhibit { on: bool },
    NativeQuickInsert { token: u64, snippet_id: String },
}

#[derive(Serialize)]
struct LintDto<'a> {
    snippet_id: String,
    message: &'a str,
}

fn lints_json(lib: &Library) -> Value {
    let lints: Vec<Lint> = engine::lint(lib);
    let dto: Vec<LintDto> = lints
        .iter()
        .map(|l| LintDto {
            snippet_id: l.snippet_id.to_string(),
            message: &l.message,
        })
        .collect();
    json!(dto)
}

impl TplCore {
    fn open(data_dir: Option<PathBuf>) -> Self {
        let location = match data_dir {
            Some(d) => DataLocation::Env(d),
            None => store::resolve_data_location(),
        };
        Self::open_at(location)
    }

    /// Open the library at `location`. For the default location, the pre-0.3 library is
    /// first copied once (see `crate::migrate`). If that copy is refused (invalid or
    /// unreadable legacy file, I/O error), the core opens with an empty in-memory library
    /// and saving disabled: no demo data is created and nothing is written.
    pub(crate) fn open_at(location: DataLocation) -> Self {
        let store = Store::new(location.dir());
        let mut migration = Value::Null;
        let mut blocked: Option<(Library, Value)> = None;
        if let DataLocation::Default { dir, legacy_dir } = &location {
            match migrate::copy_legacy_once(legacy_dir, dir) {
                Ok(MigrationOutcome::Copied { bytes, sha256 }) => {
                    migration = json!({"kind": "copied", "bytes": bytes, "sha256": sha256});
                }
                Ok(MigrationOutcome::TargetExists) => migration = json!({"kind": "not_needed"}),
                Ok(MigrationOutcome::NoLegacy) => migration = json!({"kind": "no_legacy"}),
                Err(e) => {
                    blocked = Some((
                        Library::empty(),
                        json!({"kind": "migration_failed", "error": e.to_string()}),
                    ));
                }
            }
        }
        let (library, mut load) = match blocked {
            Some(b) => b,
            None => match store.load_or_init() {
                Ok(LoadOutcome::Loaded(l)) => (l, json!({"kind": "loaded"})),
                Ok(LoadOutcome::CreatedDemo(l)) => (l, json!({"kind": "created_demo"})),
                Ok(LoadOutcome::Recovered {
                    library,
                    backup,
                    error,
                }) => (
                    library,
                    json!({"kind": "recovered", "backup": backup.display().to_string(), "error": error}),
                ),
                // Shown empty (not demo data) and never saved: see `saver` below.
                Err(e) => (
                    Library::empty(),
                    json!({"kind": "memory_only", "error": e.to_string()}),
                ),
            },
        };
        load["migration"] = migration;
        load["deprecated_env"] = json!(matches!(location, DataLocation::LegacyEnv(_)));
        let isolated = !matches!(location, DataLocation::Default { .. });
        load["isolated"] = json!(isolated);
        let playground = Engine::new(&library);
        let native = NativeController::new(); // OFF: nothing is monitored until native_start.
        let prefs_store = PrefsStore::new(location.dir());
        let (prefs, mut prefs_load) = prefs_store.load();
        if matches!(
            load["kind"].as_str(),
            Some("memory_only") | Some("migration_failed")
        ) {
            prefs_load = PrefsLoad::Locked("the library could not be opened".into());
        }
        native.set_exclusions(prefs.excluded_set());
        // If the library could not be read or migrated, never save the in-memory fallback: it
        // would overwrite (or pre-empt) the user's real library later. Edits fail closed.
        let saver = match load["kind"].as_str() {
            Some("memory_only") | Some("migration_failed") => Err(format!(
                "saving is disabled to protect your library ({})",
                load["error"].as_str().unwrap_or("unknown error")
            )),
            _ => SaveWorker::new(store.clone()),
        };
        TplCore {
            state: Mutex::new(CoreState {
                library,
                playground,
                native,
                saver,
                prefs,
                prefs_load,
            }),
            store,
            prefs_store,
            load,
            isolated,
        }
    }

    fn preferences_json(&self, st: &CoreState) -> Value {
        let error = match &st.prefs_load {
            PrefsLoad::Missing | PrefsLoad::Loaded => None,
            PrefsLoad::Corrupt(e) => Some(format!(
                "The preferences file could not be read ({e}); defaults are used. It will be set aside on the next change."
            )),
            PrefsLoad::Locked(e) => Some(format!("Preferences are read-only this session: {e}")),
        };
        json!({
            "preferences": st.prefs,
            "error": error,
            "writable": !matches!(st.prefs_load, PrefsLoad::Locked(_)),
            "isolated": self.isolated,
            "path": self.prefs_store.path().display().to_string(),
        })
    }

    fn backups_json(&self) -> Result<Value, String> {
        let list = backup::list(self.store.dir()).map_err(|e| e.to_string())?;
        Ok(json!({
            "backups": list,
            "status": self.store.backup_status(),
            "folder": backup::backup_dir(self.store.dir()).display().to_string(),
            "retention": backup::RETENTION,
        }))
    }

    fn handle(&self, request: Request) -> Result<Value, String> {
        // Stateless requests: read files without holding the core lock.
        match &request {
            Request::ParseImport { path } => {
                let lib =
                    store::import_from(std::path::Path::new(path)).map_err(|e| e.to_string())?;
                let summary = json!({"groups": lib.groups.len(), "snippets": lib.snippet_count()});
                return Ok(json!({"library": lib, "summary": summary}));
            }
            Request::BackupRead { name } => {
                let lib = backup::read(self.store.dir(), name).map_err(|e| e.to_string())?;
                let summary = json!({"groups": lib.groups.len(), "snippets": lib.snippet_count()});
                return Ok(json!({"library": lib, "summary": summary}));
            }
            Request::BackupList => return self.backups_json(),
            Request::ParseCsvImport { path } => {
                let mut import = csv_import::import_file(std::path::Path::new(path))
                    .map_err(|e| e.to_string())?;
                // Only reads the current library to point out abbreviations already in use.
                csv_import::note_existing_conflicts(&mut import, &self.lock_state()?.library);
                let lib = &import.library;
                let summary = json!({"groups": lib.groups.len(), "snippets": lib.snippet_count()});
                return Ok(json!({"library": lib, "summary": summary, "report": import.report}));
            }
            _ => {}
        }
        // A restore reads (and validates) the snapshot before taking the lock.
        let restore = match &request {
            Request::BackupRestore { name } => {
                Some(backup::read(self.store.dir(), name).map_err(|e| e.to_string())?)
            }
            _ => None,
        };
        let mut st = self.lock_state()?;
        match request {
            Request::ParseImport { .. }
            | Request::BackupRead { .. }
            | Request::BackupList
            | Request::ParseCsvImport { .. } => unreachable!(),
            Request::PreferencesGet => Ok(self.preferences_json(&st)),
            Request::PreferencesSet { preferences } => {
                self.prefs_store
                    .save(&preferences, &st.prefs_load)
                    .map_err(|e| e.to_string())?;
                st.native.set_exclusions(preferences.excluded_set());
                st.prefs = preferences;
                st.prefs_load = PrefsLoad::Loaded;
                Ok(self.preferences_json(&st))
            }
            Request::BackupNow => {
                if let Err(e) = &st.saver {
                    return Err(format!("backups are unavailable ({e})"));
                }
                let bytes = store::to_json(&st.library).map_err(|e| e.to_string())?;
                let name = backup::write_snapshot(
                    self.store.dir(),
                    BackupKind::Manual,
                    bytes.as_bytes(),
                    chrono::Local::now(),
                )
                .map_err(|e| e.to_string())?;
                drop(st);
                let mut reply = self.backups_json()?;
                reply["created"] = json!(name);
                Ok(reply)
            }
            Request::BackupRestore { .. } => {
                let library = restore.expect("read above");
                if let Err(e) = &st.saver {
                    return Err(format!("restore is unavailable ({e})"));
                }
                // Keep the current library first; if that fails, nothing is restored.
                let bytes = store::to_json(&st.library).map_err(|e| e.to_string())?;
                let pre = backup::write_snapshot(
                    self.store.dir(),
                    BackupKind::PreRestore,
                    bytes.as_bytes(),
                    chrono::Local::now(),
                )
                .map_err(|e| format!("the current library could not be backed up first, so nothing was restored: {e}"))?;
                let mut reply = st.adopt(library)?;
                reply["library"] = json!(st.library);
                reply["pre_restore"] = json!(pre);
                Ok(reply)
            }
            Request::NativeSetFrontmost { pid } => {
                st.native.set_frontmost(pid);
                Ok(json!({}))
            }
            Request::NativeCaptureTarget => Ok(json!(st.native.capture_target()?)),
            Request::NativeReleaseTarget => {
                st.native.release_target();
                Ok(json!({}))
            }
            Request::NativeInhibit { on } => {
                st.native.set_inhibited(on);
                Ok(native_status(&st.native))
            }
            Request::NativeQuickInsert { token, snippet_id } => {
                let id =
                    uuid::Uuid::parse_str(&snippet_id).map_err(|_| "unknown snippet".to_owned())?;
                let (gi, si) = st
                    .library
                    .find_snippet(id)
                    .ok_or("the snippet no longer exists")?;
                let snippet = st.library.groups[gi].snippets[si].clone();
                st.native.quick_insert(token, &snippet)?;
                Ok(native_status(&st.native))
            }
            Request::State => Ok(json!({
                "library": st.library,
                "library_path": self.store.library_path().display().to_string(),
                "load": self.load,
                "lints": lints_json(&st.library),
                "active_rules": st.playground.active_rule_count(),
                "save": st.save_status(),
                "abi_version": ABI_VERSION,
                "core_version": env!("CARGO_PKG_VERSION"),
            })),
            Request::UpdateLibrary { library } | Request::ReplaceLibrary { library } => {
                st.adopt(library)
            }
            Request::MergeImport { library } => {
                let mut merged = st.library.clone();
                let summary = store::merge_into(&mut merged, library);
                let mut reply = st.adopt(merged)?;
                reply["merged"] = json!({"groups": summary.groups, "snippets": summary.snippets});
                reply["library"] = json!(st.library);
                Ok(reply)
            }
            Request::Export { path } => {
                store::export_to(std::path::Path::new(&path), &st.library)
                    .map_err(|e| e.to_string())?;
                Ok(json!({"snippets": st.library.snippet_count()}))
            }
            Request::Render { body } => {
                let r = template::render(&body, &render_ctx(&body));
                let before: String = match r.cursor {
                    Some(c) => r.text.chars().take(c).collect(),
                    None => r.text.clone(),
                };
                Ok(json!({
                    "text": r.text,
                    "has_cursor": r.cursor.is_some(),
                    "before_cursor": before,
                    "after_cursor": r.text_after_cursor(),
                    "warnings": r.warnings,
                }))
            }
            Request::PlaygroundReset => {
                st.playground.feed(KeyInput::Reset);
                Ok(json!({}))
            }
            Request::PlaygroundEdit {
                backspaces,
                inserted,
            } => {
                for _ in 0..backspaces.min(engine::MAX_BUFFER_CHARS * 2) {
                    st.playground.feed(KeyInput::Backspace);
                }
                let mut expansion = Value::Null;
                for (i, c) in inserted.chars().enumerate() {
                    if let Some(exp) = st.playground.feed(KeyInput::Char(c)) {
                        let rep = exp.render(&render_ctx(&exp.body));
                        expansion = json!({
                            "title": exp.title,
                            "consumed_chars": i + 1,
                            "delete_text": rep.delete_text,
                            "insert": rep.insert,
                            "after_cursor": rep.after_cursor,
                            "warnings": rep.warnings,
                        });
                        break;
                    }
                }
                Ok(json!({"expansion": expansion, "buffer": st.playground.buffer()}))
            }
            Request::SaveStatus => Ok(json!(st.save_status())),
            Request::Flush { timeout_ms } => {
                let done = match &st.saver {
                    Ok(w) => w.flush(Duration::from_millis(timeout_ms.min(10_000))),
                    Err(_) => false,
                };
                Ok(json!({"flushed": done, "save": st.save_status()}))
            }
            Request::NativeStatus => Ok(native_status(&st.native)),
            Request::NativeStart => {
                let lib = st.library.clone();
                st.native.start(&lib)?;
                Ok(native_status(&st.native))
            }
            Request::NativePause => {
                st.native.set_paused(true);
                Ok(native_status(&st.native))
            }
            Request::NativeResume => {
                st.native.set_paused(false);
                Ok(native_status(&st.native))
            }
            Request::NativeStop => {
                // Signal shutdown under the lock (immediate), join the threads without it, so
                // other calls (rendering, edits) are never stuck behind the bounded join.
                let pending = st.native.begin_stop();
                drop(st);
                let outcome = pending.map(|p| p.wait());
                let mut st = self.lock_state()?;
                if let Some(outcome) = outcome {
                    st.native.finish_stop(outcome);
                }
                Ok(native_status(&st.native))
            }
            Request::NativeRefreshLayout => {
                if !main_thread() {
                    return Err("native_refresh_layout must be called on the main thread".into());
                }
                st.native.refresh_keyboard_layout();
                Ok(json!({}))
            }
            Request::OpenAccessibilitySettings => {
                native::open_accessibility_settings()?;
                Ok(json!({}))
            }
        }
    }

    fn lock_state(&self) -> Result<std::sync::MutexGuard<'_, CoreState>, String> {
        self.state
            .lock()
            .map_err(|_| "core state is unavailable after an earlier internal error".to_owned())
    }

    fn close(&self) {
        if let Ok(mut st) = self.state.lock() {
            st.native.stop();
            if let Ok(w) = &mut st.saver {
                w.shutdown();
            }
        }
    }
}

impl CoreState {
    /// Validate and adopt a library: engines and lints update now, the write happens on the
    /// save worker. Invalid libraries are rejected and the previous one is kept.
    fn adopt(&mut self, library: Library) -> Result<Value, String> {
        let errors = store::validate(&library);
        if !errors.is_empty() {
            return Err(store::StoreError::Invalid(errors).to_string());
        }
        // Fail closed: if the change cannot be queued for saving, do not adopt it.
        match &self.saver {
            Ok(w) => w.submit(library.clone())?,
            Err(e) => return Err(format!("saving unavailable ({e}); change not applied")),
        }
        self.playground.load(&library);
        self.native.update_library(&library);
        self.library = library;
        Ok(json!({
            "lints": lints_json(&self.library),
            "active_rules": self.playground.active_rule_count(),
            "save": self.save_status(),
        }))
    }

    fn save_status(&self) -> SaveStatus {
        match &self.saver {
            Ok(w) => w.status(),
            Err(e) => SaveStatus {
                last_error: Some(format!("saving unavailable: {e}")),
                ..SaveStatus::default()
            },
        }
    }
}

fn render_ctx(body: &str) -> RenderContext {
    let clipboard = if template::uses_clipboard(body) {
        arboard::Clipboard::new()
            .ok()
            .and_then(|mut c| c.get_text().ok())
    } else {
        None
    };
    RenderContext::new(chrono::Local::now(), clipboard)
}

fn main_thread() -> bool {
    #[cfg(target_os = "macos")]
    {
        native::macos::is_main_thread()
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

fn native_status(n: &NativeController) -> Value {
    let (state, error) = match n.state() {
        NativeState::Off => ("off", None),
        NativeState::Running => ("running", None),
        NativeState::Paused => ("paused", None),
        NativeState::Disabled => ("disabled", None),
        NativeState::Stopping => ("stopping", None),
        NativeState::Error(e) => ("error", Some(e)),
    };
    let s = n.stats();
    json!({
        "state": state,
        "error": error,
        "inhibited": n.is_inhibited(),
        "permissions": n.permissions(),
        "process_path": std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default(),
        "stats": {
            "expansions": s.expansions,
            "last_snippet": s.last_snippet,
            "last_at": s.last_at.map(|t| t.format("%H:%M:%S").to_string()),
            "secure_input_skips": s.secure_input_skips,
            "tap_reenabled": s.tap_reenabled,
            "last_warning": s.last_warning,
            "aborted": s.aborted,
            "last_abort": s.last_abort,
            "last_quick_insert": s.last_quick_insert,
            "diag": s.diag,
        }
    })
}

// ---- extern "C" surface ---------------------------------------------------------------

fn reply_ok(result: Value) -> String {
    json!({"ok": true, "result": result}).to_string()
}

fn reply_err(msg: impl Into<String>) -> String {
    json!({"ok": false, "error": msg.into()}).to_string()
}

fn into_c(s: String) -> *mut c_char {
    // JSON never contains interior NULs (serde escapes U+0000), but stay total.
    CString::new(s)
        .unwrap_or_else(|_| CString::new(reply_err("reply contained NUL")).unwrap())
        .into_raw()
}

#[no_mangle]
pub extern "C" fn tpl_abi_version() -> u32 {
    ABI_VERSION
}

/// # Safety
/// `data_dir` must be NULL or a valid NUL-terminated string. NULL selects the default
/// location; an empty or non-UTF-8 string is an open failure (returns NULL) rather than a
/// silent fallback to the user's real library folder.
#[no_mangle]
pub unsafe extern "C" fn tpl_core_open(data_dir: *const c_char) -> *mut TplCore {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let dir = if data_dir.is_null() {
            None
        } else {
            // SAFETY: caller contract.
            match unsafe { CStr::from_ptr(data_dir) }.to_str() {
                Ok(s) if !s.is_empty() => Some(PathBuf::from(s)),
                _ => return std::ptr::null_mut(),
            }
        };
        Box::into_raw(Box::new(TplCore::open(dir)))
    }));
    result.unwrap_or(std::ptr::null_mut())
}

/// # Safety
/// `core` must be NULL or a pointer from `tpl_core_open` that has not been closed;
/// `request` must be NULL or a valid NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn tpl_core_call(core: *mut TplCore, request: *const c_char) -> *mut c_char {
    let out = catch_unwind(AssertUnwindSafe(|| {
        if core.is_null() {
            return reply_err("core handle is NULL");
        }
        if request.is_null() {
            return reply_err("request is NULL");
        }
        // SAFETY: caller contract.
        let text = match unsafe { CStr::from_ptr(request) }.to_str() {
            Ok(t) => t,
            Err(_) => return reply_err("request is not valid UTF-8"),
        };
        let req: Request = match serde_json::from_str(text) {
            Ok(r) => r,
            Err(e) => return reply_err(format!("bad request: {e}")),
        };
        // SAFETY: caller contract; TplCore is internally synchronised.
        let core = unsafe { &*core };
        match core.handle(req) {
            Ok(v) => reply_ok(v),
            Err(e) => reply_err(e),
        }
    }))
    .unwrap_or_else(|_| reply_err("internal error (panic caught at the ABI boundary)"));
    into_c(out)
}

/// # Safety
/// `core` must be NULL or a pointer from `tpl_core_open`, closed at most once.
#[no_mangle]
pub unsafe extern "C" fn tpl_core_close(core: *mut TplCore) {
    if core.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: caller contract — we take back ownership exactly once.
        let boxed = unsafe { Box::from_raw(core) };
        boxed.close();
        drop(boxed);
    }));
}

/// # Safety
/// `s` must be NULL or a string returned by `tpl_core_call`, freed at most once.
#[no_mangle]
pub unsafe extern "C" fn tpl_string_free(s: *mut c_char) {
    if !s.is_null() {
        // SAFETY: caller contract — the string came from CString::into_raw.
        drop(unsafe { CString::from_raw(s) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Library;
    use std::fs;

    fn locations() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let legacy = root.path().join("legacy");
        let dir = root.path().join("new");
        (root, legacy, dir)
    }

    fn state(core: &TplCore) -> Value {
        core.handle(Request::State).unwrap()
    }

    #[test]
    fn default_location_copies_legacy_once_and_loads_it_unchanged() {
        let (_root, legacy, dir) = locations();
        fs::create_dir_all(&legacy).unwrap();
        let bytes = format!("{}\n", store::to_json(&Library::demo()).unwrap()).into_bytes();
        fs::write(legacy.join("library.json"), &bytes).unwrap();
        let core = TplCore::open_at(DataLocation::Default {
            dir: dir.clone(),
            legacy_dir: legacy.clone(),
        });
        let st = state(&core);
        assert_eq!(st["load"]["kind"], "loaded");
        assert_eq!(st["load"]["migration"]["kind"], "copied");
        assert_eq!(
            st["load"]["migration"]["sha256"],
            migrate::sha256_hex(&bytes)
        );
        core.close();
        assert_eq!(
            fs::read(dir.join("library.json")).unwrap(),
            bytes,
            "byte-for-byte, no rewrite"
        );
        assert_eq!(
            fs::read(legacy.join("library.json")).unwrap(),
            bytes,
            "legacy untouched"
        );
        // Re-open: no second copy.
        let again = TplCore::open_at(DataLocation::Default {
            dir,
            legacy_dir: legacy,
        });
        assert_eq!(state(&again)["load"]["migration"]["kind"], "not_needed");
        again.close();
    }

    #[test]
    fn invalid_legacy_fails_closed_without_demo_or_writes() {
        let (_root, legacy, dir) = locations();
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("library.json"), b"{ broken").unwrap();
        let core = TplCore::open_at(DataLocation::Default {
            dir: dir.clone(),
            legacy_dir: legacy.clone(),
        });
        let st = state(&core);
        assert_eq!(st["load"]["kind"], "migration_failed");
        assert_eq!(
            st["library"]["groups"].as_array().unwrap().len(),
            0,
            "no demo data"
        );
        assert!(st["save"]["last_error"]
            .as_str()
            .unwrap()
            .contains("saving is disabled"));
        let lib: Library = serde_json::from_value(st["library"].clone()).unwrap();
        assert!(
            core.handle(Request::UpdateLibrary { library: lib })
                .is_err(),
            "edits refused"
        );
        core.close();
        assert!(
            !dir.join("library.json").exists(),
            "nothing written to the new location"
        );
        assert_eq!(fs::read(legacy.join("library.json")).unwrap(), b"{ broken");
    }

    #[test]
    fn dangling_symlink_target_fails_closed_at_open() {
        // Target library.json is a dangling symlink: migration must not replace it, and the
        // core must not create demo data or save over it.
        let (_root, legacy, dir) = locations();
        fs::create_dir_all(&legacy).unwrap();
        fs::write(
            legacy.join("library.json"),
            store::to_json(&Library::demo()).unwrap(),
        )
        .unwrap();
        fs::create_dir_all(&dir).unwrap();
        let link = dir.join("library.json");
        std::os::unix::fs::symlink("/nonexistent/elsewhere.json", &link).unwrap();
        let core = TplCore::open_at(DataLocation::Default {
            dir: dir.clone(),
            legacy_dir: legacy,
        });
        let st = state(&core);
        assert_eq!(st["load"]["migration"]["kind"], "not_needed");
        assert_eq!(st["load"]["kind"], "memory_only");
        assert_eq!(
            st["library"]["groups"].as_array().unwrap().len(),
            0,
            "no demo data"
        );
        let lib: Library = serde_json::from_value(st["library"].clone()).unwrap();
        assert!(
            core.handle(Request::UpdateLibrary { library: lib })
                .is_err(),
            "edits refused"
        );
        core.close();
        assert_eq!(
            fs::read_link(&link).unwrap(),
            PathBuf::from("/nonexistent/elsewhere.json")
        );
        assert_eq!(
            fs::read_dir(&dir).unwrap().count(),
            1,
            "nothing else written"
        );
    }

    #[test]
    fn explicit_and_legacy_env_locations_never_migrate() {
        let (_root, legacy, dir) = locations();
        fs::create_dir_all(&legacy).unwrap();
        fs::write(
            legacy.join("library.json"),
            store::to_json(&Library::demo()).unwrap(),
        )
        .unwrap();
        let core = TplCore::open_at(DataLocation::LegacyEnv(dir.clone()));
        let st = state(&core);
        assert_eq!(st["load"]["migration"], Value::Null);
        assert_eq!(st["load"]["deprecated_env"], true);
        assert_eq!(st["load"]["kind"], "created_demo");
        core.close();
    }
}

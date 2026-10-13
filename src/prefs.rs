//! App preferences kept next to the library, in `<data folder>/preferences.json`.
//!
//! Keeping them in the data folder (instead of the per-bundle macOS defaults domain) means a
//! QA run with an isolated `TYPELUNE_DATA_DIR` reads and writes its own preferences and never
//! changes the real ones. The library schema is unchanged: exclusions and shortcuts are not
//! library data, so older versions read the library exactly as before.
//!
//! * Missing file: defaults, nothing written until a preference is changed.
//! * Corrupt file: defaults; on the next change it is first moved aside (never deleted).
//! * Symbolic link, or a file from a newer format version: defaults and writes are refused.
//! * Writes are atomic (exclusive temp file + rename).

use crate::store::{self, StoreError};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

pub const PREFERENCES_FILE: &str = "preferences.json";
pub const PREFERENCES_FORMAT: &str = "typelune.preferences";
pub const PREFERENCES_VERSION: u32 = 1;
pub const MAX_PREFERENCES_BYTES: u64 = 1024 * 1024;
pub const MAX_EXCLUDED_APPS: usize = 500;
pub const MAX_BUNDLE_ID_CHARS: usize = 255;
pub const MAX_APP_NAME_CHARS: usize = 200;

/// An app in which Typelune never records typing, matches or inserts text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExcludedApp {
    /// Stable bundle identifier, e.g. `com.example.Editor`. Compared case-insensitively.
    pub bundle_id: String,
    /// Display name at the time it was added (informational only).
    #[serde(default)]
    pub name: String,
}

/// A global keyboard shortcut. `key_code` is a macOS virtual key code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hotkey {
    pub key_code: u16,
    #[serde(default)]
    pub command: bool,
    #[serde(default)]
    pub option: bool,
    #[serde(default)]
    pub control: bool,
    #[serde(default)]
    pub shift: bool,
    /// What the key produced when recorded, for display (e.g. "T").
    #[serde(default)]
    pub key_label: String,
}

impl Hotkey {
    /// Default Quick Search shortcut: Control-Option-Command-T.
    pub fn default_quick_search() -> Self {
        Self {
            key_code: 17,
            command: true,
            option: true,
            control: true,
            shift: false,
            key_label: "T".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preferences {
    #[serde(default = "default_format")]
    pub format: String,
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub excluded_apps: Vec<ExcludedApp>,
    /// `None` = no global shortcut.
    #[serde(default = "default_hotkey")]
    pub quick_search_hotkey: Option<Hotkey>,
    /// Remembered opt-in: start system-wide expansion when Typelune opens. Off by default.
    #[serde(default)]
    pub start_expansion_at_launch: bool,
}

fn default_format() -> String {
    PREFERENCES_FORMAT.into()
}
fn default_version() -> u32 {
    PREFERENCES_VERSION
}
fn default_hotkey() -> Option<Hotkey> {
    Some(Hotkey::default_quick_search())
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            format: default_format(),
            version: default_version(),
            excluded_apps: Vec::new(),
            quick_search_hotkey: default_hotkey(),
            start_expansion_at_launch: false,
        }
    }
}

impl Preferences {
    /// Lower-cased excluded bundle identifiers.
    pub fn excluded_set(&self) -> HashSet<String> {
        self.excluded_apps
            .iter()
            .map(|a| a.bundle_id.to_ascii_lowercase())
            .collect()
    }
}

fn valid_bundle_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_BUNDLE_ID_CHARS
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

pub fn validate(p: &Preferences) -> Vec<String> {
    let mut errors = Vec::new();
    if p.format != PREFERENCES_FORMAT {
        errors.push(format!("format must be \"{PREFERENCES_FORMAT}\""));
    }
    if p.version != PREFERENCES_VERSION {
        errors.push(format!("version must be {PREFERENCES_VERSION}"));
    }
    if p.excluded_apps.len() > MAX_EXCLUDED_APPS {
        errors.push(format!("more than {MAX_EXCLUDED_APPS} excluded apps"));
    }
    let mut seen = HashSet::new();
    for app in &p.excluded_apps {
        if !valid_bundle_id(&app.bundle_id) {
            errors.push(format!(
                "“{}” is not a valid bundle identifier",
                store_safe(&app.bundle_id)
            ));
        } else if !seen.insert(app.bundle_id.to_ascii_lowercase()) {
            errors.push(format!("{} is listed twice", app.bundle_id));
        }
        if app.name.chars().count() > MAX_APP_NAME_CHARS {
            errors.push(format!("name of {} is too long", app.bundle_id));
        }
    }
    if let Some(h) = &p.quick_search_hotkey {
        if h.key_code > 0x7F {
            errors.push("shortcut key code is out of range".into());
        }
        if !(h.command || h.option || h.control) {
            errors.push("a shortcut needs Command, Option or Control".into());
        }
        if h.key_label.chars().count() > 16 {
            errors.push("shortcut label is too long".into());
        }
    }
    errors
}

/// Shorten untrusted text for error messages.
fn store_safe(s: &str) -> String {
    crate::model::truncate_chars(s, 60)
}

/// How the preferences were found at open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrefsLoad {
    Missing,
    Loaded,
    /// Unreadable or invalid; defaults used. Moved aside before the next write.
    Corrupt(String),
    /// Symlink, newer version or I/O error; defaults used and writes refused.
    Locked(String),
}

#[derive(Debug, Clone)]
pub struct PrefsStore {
    dir: PathBuf,
}

impl PrefsStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn path(&self) -> PathBuf {
        self.dir.join(PREFERENCES_FILE)
    }

    pub fn load(&self) -> (Preferences, PrefsLoad) {
        let path = self.path();
        let meta = match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == ErrorKind::NotFound => {
                return (Preferences::default(), PrefsLoad::Missing)
            }
            Err(e) => return (Preferences::default(), PrefsLoad::Locked(e.to_string())),
            Ok(m) => m,
        };
        if !meta.file_type().is_file() {
            return (
                Preferences::default(),
                PrefsLoad::Locked(format!(
                    "{} is not a regular file; preferences are read-only this session",
                    path.display()
                )),
            );
        }
        if meta.len() > MAX_PREFERENCES_BYTES {
            return (
                Preferences::default(),
                PrefsLoad::Corrupt("preferences file is too large".into()),
            );
        }
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => return (Preferences::default(), PrefsLoad::Locked(e.to_string())),
        };
        let value: serde_json::Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => return (Preferences::default(), PrefsLoad::Corrupt(e.to_string())),
        };
        if let Some(v) = value.get("version").and_then(|v| v.as_u64()) {
            if v > PREFERENCES_VERSION as u64 {
                return (
                    Preferences::default(),
                    PrefsLoad::Locked(format!(
                        "preferences were written by a newer Typelune (version {v}); they are left unchanged"
                    )),
                );
            }
        }
        match serde_json::from_value::<Preferences>(value) {
            Ok(p) => {
                let errors = validate(&p);
                if errors.is_empty() {
                    (p, PrefsLoad::Loaded)
                } else {
                    (
                        Preferences::default(),
                        PrefsLoad::Corrupt(errors.join("; ")),
                    )
                }
            }
            Err(e) => (Preferences::default(), PrefsLoad::Corrupt(e.to_string())),
        }
    }

    /// Validate and write atomically. A corrupt file is moved aside first.
    pub fn save(&self, p: &Preferences, load: &PrefsLoad) -> Result<(), StoreError> {
        if let PrefsLoad::Locked(reason) = load {
            return Err(StoreError::Invalid(vec![format!(
                "preferences cannot be saved: {reason}"
            )]));
        }
        let errors = validate(p);
        if !errors.is_empty() {
            return Err(StoreError::Invalid(errors));
        }
        fs::create_dir_all(&self.dir).map_err(|source| StoreError::Io {
            path: self.dir.clone(),
            source,
        })?;
        let path = self.path();
        match fs::symlink_metadata(&path) {
            Ok(m) if !m.file_type().is_file() => {
                return Err(StoreError::Invalid(vec![format!(
                    "{} is not a regular file; Typelune will not replace it",
                    path.display()
                )]))
            }
            Ok(_) if matches!(load, PrefsLoad::Corrupt(_)) => self.move_aside(&path)?,
            _ => {}
        }
        let json = serde_json::to_string_pretty(p).map_err(StoreError::Json)?;
        store::write_atomic(&path, json.as_bytes())
    }

    fn move_aside(&self, path: &Path) -> Result<(), StoreError> {
        let aside = self
            .dir
            .join(format!("preferences.corrupt-{}.json", uuid::Uuid::new_v4()));
        fs::hard_link(path, &aside)
            .and_then(|_| fs::remove_file(path))
            .map_err(|source| StoreError::Io {
                path: path.to_owned(),
                source,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_safe() {
        let p = Preferences::default();
        assert!(!p.start_expansion_at_launch, "never auto-start by default");
        assert!(p.excluded_apps.is_empty());
        assert!(validate(&p).is_empty());
    }

    #[test]
    fn missing_file_is_not_written_on_load() {
        let tmp = tempfile::tempdir().unwrap();
        let s = PrefsStore::new(tmp.path());
        let (p, load) = s.load();
        assert_eq!(load, PrefsLoad::Missing);
        assert_eq!(p, Preferences::default());
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 0);
    }

    #[test]
    fn round_trip_and_older_files_get_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let s = PrefsStore::new(tmp.path());
        let mut p = Preferences::default();
        p.excluded_apps.push(ExcludedApp {
            bundle_id: "com.example.Editor".into(),
            name: "Editor".into(),
        });
        p.start_expansion_at_launch = true;
        p.quick_search_hotkey = None;
        s.save(&p, &PrefsLoad::Missing).unwrap();
        assert_eq!(s.load(), (p, PrefsLoad::Loaded));
        // A minimal file (e.g. written by hand) loads with defaults for the rest.
        fs::write(
            s.path(),
            r#"{"excluded_apps":[{"bundle_id":"com.example.A"}]}"#,
        )
        .unwrap();
        let (p, load) = s.load();
        assert_eq!(load, PrefsLoad::Loaded);
        assert_eq!(p.excluded_apps[0].bundle_id, "com.example.A");
        assert!(!p.start_expansion_at_launch);
        assert_eq!(p.quick_search_hotkey, Some(Hotkey::default_quick_search()));
    }

    #[test]
    fn validation_rejects_bad_entries() {
        let mut p = Preferences::default();
        for id in ["", "has space", "../x", "com.example.A", "COM.EXAMPLE.a"] {
            p.excluded_apps.push(ExcludedApp {
                bundle_id: id.into(),
                name: String::new(),
            });
        }
        p.quick_search_hotkey = Some(Hotkey {
            command: false,
            option: false,
            control: false,
            ..Hotkey::default_quick_search()
        });
        let errors = validate(&p);
        assert_eq!(
            errors
                .iter()
                .filter(|e| e.contains("not a valid bundle"))
                .count(),
            3
        );
        assert!(errors.iter().any(|e| e.contains("listed twice")));
        assert!(errors.iter().any(|e| e.contains("needs Command")));
    }

    #[test]
    fn corrupt_file_is_moved_aside_on_next_write() {
        let tmp = tempfile::tempdir().unwrap();
        let s = PrefsStore::new(tmp.path());
        fs::write(s.path(), "{ nope").unwrap();
        let (p, load) = s.load();
        assert!(matches!(load, PrefsLoad::Corrupt(_)));
        s.save(&p, &load).unwrap();
        let aside: Vec<_> = fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("preferences.corrupt-")
            })
            .collect();
        assert_eq!(aside.len(), 1);
        assert_eq!(fs::read_to_string(aside[0].path()).unwrap(), "{ nope");
        assert_eq!(s.load().1, PrefsLoad::Loaded);
    }

    #[test]
    fn symlink_and_newer_versions_are_never_overwritten() {
        let tmp = tempfile::tempdir().unwrap();
        let s = PrefsStore::new(tmp.path());
        std::os::unix::fs::symlink("/nonexistent/prefs.json", s.path()).unwrap();
        let (p, load) = s.load();
        assert!(matches!(load, PrefsLoad::Locked(_)));
        assert!(s.save(&p, &load).is_err());
        assert!(fs::symlink_metadata(s.path())
            .unwrap()
            .file_type()
            .is_symlink());

        let tmp = tempfile::tempdir().unwrap();
        let s = PrefsStore::new(tmp.path());
        fs::write(s.path(), r#"{"format":"typelune.preferences","version":9}"#).unwrap();
        let (p, load) = s.load();
        assert!(matches!(load, PrefsLoad::Locked(_)));
        assert!(s.save(&p, &load).is_err());
        assert!(fs::read_to_string(s.path())
            .unwrap()
            .contains("\"version\":9"));
    }
}

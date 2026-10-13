//! JSON persistence, import and export.
//!
//! Library lives in `~/Library/Application Support/io.github.tranxuanliem.typelune/library.json`
//! (override with `TYPELUNE_DATA_DIR`; the pre-0.3 `ATEXT_RS_DATA_DIR` is still honoured but
//! deprecated). Writes are atomic (temp file + rename) and the previous version is kept as
//! `library.json.bak`. A library from the pre-0.3 location is copied once by
//! [`crate::migrate`], never moved or rewritten.

use crate::engine::MAX_ABBREVIATION_CHARS;
use crate::model::{Library, ACCEPTED_FORMATS, LIBRARY_FORMAT, LIBRARY_VERSION};
use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Application Support folder name (the bundle identifier).
pub const APP_DIR_NAME: &str = "io.github.tranxuanliem.typelune";
/// Folder used before v0.3.0 (bundle identifier `dev.atext-rs.poc`); read-only source of
/// the one-time copy.
pub const LEGACY_APP_DIR_NAME: &str = "dev.atext-rs.poc";
pub const DATA_DIR_ENV: &str = "TYPELUNE_DATA_DIR";
/// Deprecated alias of [`DATA_DIR_ENV`], kept for pre-0.3 test setups.
pub const LEGACY_DATA_DIR_ENV: &str = "ATEXT_RS_DATA_DIR";

/// Where the library lives and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataLocation {
    /// Standard Application Support folder; `legacy_dir` is the pre-0.3 folder to copy from.
    Default { dir: PathBuf, legacy_dir: PathBuf },
    /// `TYPELUNE_DATA_DIR`.
    Env(PathBuf),
    /// Deprecated `ATEXT_RS_DATA_DIR`.
    LegacyEnv(PathBuf),
}

/// Data folder for the optional legacy egui developer UI. It must be an explicit,
/// isolated `TYPELUNE_DATA_DIR`: the production folder, the pre-0.3 folder (anything
/// inside or containing them) and other apps' data paths are refused, so the developer UI
/// can never create a library that pre-empts the one-time migration or touches real data.
pub fn developer_fallback_dir(
    env_value: Option<std::ffi::OsString>,
    production: &Path,
    legacy: &Path,
) -> Result<PathBuf, String> {
    let dir = env_value
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| {
            format!("the legacy egui UI needs an explicit {DATA_DIR_ENV}, e.g. {DATA_DIR_ENV}=/tmp/typelune-dev")
        })?;
    if !dir.is_absolute() {
        return Err(format!("{DATA_DIR_ENV} must be an absolute path"));
    }
    let norm = |p: &Path| -> PathBuf { p.components().collect() };
    let (d, prod, leg) = (norm(&dir), norm(production), norm(legacy));
    for protected in [&prod, &leg] {
        if d.starts_with(protected) || protected.starts_with(&d) {
            return Err(format!(
                "refusing {}: it is or contains the app's real library folder",
                dir.display()
            ));
        }
    }
    if is_protected_path(&dir) || is_protected_path(&dir.join(LIBRARY_FILE)) {
        return Err(format!(
            "refusing {}: it looks like another app's data",
            dir.display()
        ));
    }
    Ok(dir)
}

impl DataLocation {
    pub fn dir(&self) -> &Path {
        match self {
            DataLocation::Default { dir, .. }
            | DataLocation::Env(dir)
            | DataLocation::LegacyEnv(dir) => dir,
        }
    }
}

/// Resolve the data folder from the environment. Empty variables are ignored.
pub fn resolve_data_location() -> DataLocation {
    let non_empty = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
    if let Some(dir) = non_empty(DATA_DIR_ENV) {
        return DataLocation::Env(PathBuf::from(dir));
    }
    if let Some(dir) = non_empty(LEGACY_DATA_DIR_ENV) {
        return DataLocation::LegacyEnv(PathBuf::from(dir));
    }
    let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    DataLocation::Default {
        dir: base.join(APP_DIR_NAME),
        legacy_dir: base.join(LEGACY_APP_DIR_NAME),
    }
}
pub const LIBRARY_FILE: &str = "library.json";
pub const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;
pub const MAX_SNIPPETS: usize = 10_000;
pub const MAX_BODY_CHARS: usize = 100_000;
pub const MAX_NAME_CHARS: usize = 200;
pub const MAX_TAGS: usize = 32;
pub const MAX_TAG_CHARS: usize = 64;

#[derive(Debug)]
pub enum StoreError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    TooLarge {
        path: PathBuf,
        bytes: u64,
    },
    Json(serde_json::Error),
    Invalid(Vec<String>),
    ProtectedPath(PathBuf),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Io { path, source } => write!(f, "{}: {source}", path.display()),
            StoreError::TooLarge { path, bytes } => write!(
                f,
                "{} is {bytes} bytes; the limit is {MAX_FILE_BYTES} bytes",
                path.display()
            ),
            StoreError::Json(e) => write!(f, "Not a valid Typelune library: {e}"),
            StoreError::Invalid(errors) => {
                write!(f, "Library failed validation:")?;
                for e in errors.iter().take(10) {
                    write!(f, "\n  • {e}")?;
                }
                if errors.len() > 10 {
                    write!(f, "\n  • … and {} more", errors.len() - 10)?;
                }
                Ok(())
            }
            StoreError::ProtectedPath(p) => write!(
                f,
                "Refusing to write to {}: it looks like another text expander's data. Choose a different location.",
                p.display()
            ),
        }
    }
}

impl std::error::Error for StoreError {}

/// Result of opening the library at startup.
#[derive(Debug)]
pub enum LoadOutcome {
    Loaded(Library),
    /// No library existed; demo content was created and saved.
    CreatedDemo(Library),
    /// Existing file could not be read; it was moved aside and demo content created.
    Recovered {
        library: Library,
        backup: PathBuf,
        error: String,
    },
}

#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// Default location (see [`resolve_data_location`]). Does not migrate; callers that
    /// open the user's library use [`crate::migrate::copy_legacy_once`] first.
    pub fn default_location() -> Self {
        Self::new(resolve_data_location().dir())
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn library_path(&self) -> PathBuf {
        self.dir.join(LIBRARY_FILE)
    }

    /// Load the library, creating demo content only when no `library.json` entry exists
    /// at all. A symbolic link (dangling or not) is never replaced: if it cannot be read as
    /// a library this fails, and the caller keeps saving disabled. A corrupt regular file
    /// is moved aside (never deleted, never overwriting an earlier backup).
    pub fn load_or_init(&self) -> Result<LoadOutcome, StoreError> {
        let path = self.library_path();
        let io = |source| StoreError::Io {
            path: path.clone(),
            source,
        };
        match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let lib = Library::demo();
                match self.create_new(&lib) {
                    Ok(()) => Ok(LoadOutcome::CreatedDemo(lib)),
                    // Someone created it meanwhile: load theirs, never overwrite.
                    Err(StoreError::Io { source, .. })
                        if source.kind() == std::io::ErrorKind::AlreadyExists =>
                    {
                        read_library_file(&path).map(LoadOutcome::Loaded)
                    }
                    Err(e) => Err(e),
                }
            }
            Err(source) => Err(io(source)),
            Ok(meta) if meta.file_type().is_symlink() => match read_library_file(&path) {
                Ok(lib) => Ok(LoadOutcome::Loaded(lib)),
                Err(e) => Err(StoreError::Invalid(vec![format!(
                    "{} is a symbolic link that could not be read as a library ({e}); it was left in place",
                    path.display()
                )])),
            },
            Ok(_) => match read_library_file(&path) {
                Ok(lib) => Ok(LoadOutcome::Loaded(lib)),
                Err(e) => {
                    let backup = move_aside_no_clobber(&path, &self.dir)?;
                    let lib = Library::demo();
                    self.create_new(&lib)?;
                    Ok(LoadOutcome::Recovered {
                        library: lib,
                        backup,
                        error: e.to_string(),
                    })
                }
            },
        }
    }

    /// Write `lib` as a new library file; fails with `AlreadyExists` instead of replacing.
    fn create_new(&self, lib: &Library) -> Result<(), StoreError> {
        let errors = validate(lib);
        if !errors.is_empty() {
            return Err(StoreError::Invalid(errors));
        }
        fs::create_dir_all(&self.dir).map_err(|source| StoreError::Io {
            path: self.dir.clone(),
            source,
        })?;
        write_new_atomic(&self.library_path(), to_json(lib)?.as_bytes())
    }

    pub fn save(&self, lib: &Library) -> Result<(), StoreError> {
        let errors = validate(lib);
        if !errors.is_empty() {
            return Err(StoreError::Invalid(errors));
        }
        fs::create_dir_all(&self.dir).map_err(|source| StoreError::Io {
            path: self.dir.clone(),
            source,
        })?;
        let path = self.library_path();
        if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(StoreError::Invalid(vec![format!(
                "{} is a symbolic link; Typelune will not replace it. Point TYPELUNE_DATA_DIR at the real folder instead.",
                path.display()
            )]));
        }
        if let Ok(previous) = fs::read(&path) {
            // Best effort, but atomic: the backup is always a complete earlier version.
            let _ = write_atomic(&self.dir.join(format!("{LIBRARY_FILE}.bak")), &previous);
        }
        write_atomic(&path, to_json(lib)?.as_bytes())
    }
}

pub fn to_json(lib: &Library) -> Result<String, StoreError> {
    serde_json::to_string_pretty(lib).map_err(StoreError::Json)
}

pub fn read_library_file(path: &Path) -> Result<Library, StoreError> {
    let meta = fs::metadata(path).map_err(|source| StoreError::Io {
        path: path.to_owned(),
        source,
    })?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(StoreError::TooLarge {
            path: path.to_owned(),
            bytes: meta.len(),
        });
    }
    let text = fs::read_to_string(path).map_err(|source| StoreError::Io {
        path: path.to_owned(),
        source,
    })?;
    parse_library(&text)
}

/// Parse and validate library JSON (used for both loading and importing).
pub fn parse_library(json: &str) -> Result<Library, StoreError> {
    // Check the header first so foreign files get a clear message instead of a serde error.
    let value: serde_json::Value = serde_json::from_str(json).map_err(StoreError::Json)?;
    let format = value.get("format").and_then(|v| v.as_str());
    if !format.is_some_and(|f| ACCEPTED_FORMATS.contains(&f)) {
        return Err(StoreError::Invalid(vec![format!(
            "Missing or wrong \"format\" (expected \"{LIBRARY_FORMAT}\"). Only Typelune library exports can be imported; other text expanders' formats are not supported."
        )]));
    }
    match value.get("version").and_then(|v| v.as_u64()) {
        Some(v) if v == LIBRARY_VERSION as u64 => {}
        Some(v) => {
            return Err(StoreError::Invalid(vec![format!(
                "Unsupported library version {v} (this build reads version {LIBRARY_VERSION})"
            )]))
        }
        None => {
            return Err(StoreError::Invalid(vec![
                "Missing \"version\" field".to_owned()
            ]))
        }
    }
    let lib: Library = serde_json::from_value(value).map_err(StoreError::Json)?;
    let errors = validate(&lib);
    if errors.is_empty() {
        Ok(lib)
    } else {
        Err(StoreError::Invalid(errors))
    }
}

pub fn validate(lib: &Library) -> Vec<String> {
    let mut errors = Vec::new();
    if !ACCEPTED_FORMATS.contains(&lib.format.as_str()) {
        errors.push(format!("format must be \"{LIBRARY_FORMAT}\""));
    }
    if lib.version != LIBRARY_VERSION {
        errors.push(format!("version must be {LIBRARY_VERSION}"));
    }
    if lib.settings.delimiters.is_empty() {
        errors.push("settings.delimiters must contain at least one character".into());
    }
    if lib.settings.delimiters.chars().any(|c| c.is_alphanumeric()) {
        errors.push("settings.delimiters must not contain letters or digits".into());
    }
    if lib.snippet_count() > MAX_SNIPPETS {
        errors.push(format!("more than {MAX_SNIPPETS} snippets"));
    }
    let mut ids: HashSet<Uuid> = HashSet::new();
    for (gi, g) in lib.groups.iter().enumerate() {
        let gp = format!("groups[{gi}]");
        if !ids.insert(g.id) {
            errors.push(format!("{gp}.id is duplicated"));
        }
        if g.name.trim().is_empty() {
            errors.push(format!("{gp}.name is empty"));
        }
        if g.name.chars().count() > MAX_NAME_CHARS {
            errors.push(format!("{gp}.name longer than {MAX_NAME_CHARS} characters"));
        }
        for (si, s) in g.snippets.iter().enumerate() {
            let sp = format!("{gp}.snippets[{si}]");
            if !ids.insert(s.id) {
                errors.push(format!("{sp}.id is duplicated"));
            }
            if s.abbreviation.chars().count() > MAX_ABBREVIATION_CHARS {
                errors.push(format!(
                    "{sp}.abbreviation longer than {MAX_ABBREVIATION_CHARS} characters"
                ));
            }
            if s.abbreviation.chars().any(|c| c.is_control()) {
                errors.push(format!("{sp}.abbreviation contains control characters"));
            }
            if s.title.chars().count() > MAX_NAME_CHARS {
                errors.push(format!(
                    "{sp}.title longer than {MAX_NAME_CHARS} characters"
                ));
            }
            if s.body.chars().count() > MAX_BODY_CHARS {
                errors.push(format!("{sp}.body longer than {MAX_BODY_CHARS} characters"));
            }
            if s.tags.len() > MAX_TAGS {
                errors.push(format!("{sp}.tags has more than {MAX_TAGS} entries"));
            }
            if s.tags.iter().any(|t| t.chars().count() > MAX_TAG_CHARS) {
                errors.push(format!(
                    "{sp}.tags entry longer than {MAX_TAG_CHARS} characters"
                ));
            }
        }
    }
    errors
}

/// Paths that belong to other text-expansion apps. Never written to.
pub fn is_protected_path(path: &Path) -> bool {
    let lower = path.to_string_lossy().to_lowercase();
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if matches!(ext.as_str(), "atext" | "textexpander" | "atextbackup") {
        return true;
    }
    let markers = [
        "/application support/atext",
        "/containers/com.trankynam.atext",
        "trankynam",
        "/applications/atext.app",
        "/preferences/com.trankynam",
    ];
    markers.iter().any(|m| lower.contains(m))
}

pub fn export_to(path: &Path, lib: &Library) -> Result<(), StoreError> {
    if is_protected_path(path) {
        return Err(StoreError::ProtectedPath(path.to_owned()));
    }
    let errors = validate(lib);
    if !errors.is_empty() {
        return Err(StoreError::Invalid(errors));
    }
    write_atomic(path, to_json(lib)?.as_bytes())
}

pub fn import_from(path: &Path) -> Result<Library, StoreError> {
    read_library_file(path)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportSummary {
    pub groups: usize,
    pub snippets: usize,
}

/// Append imported groups to `target` with fresh IDs. Settings of `target` are kept.
pub fn merge_into(target: &mut Library, mut incoming: Library) -> ImportSummary {
    let mut summary = ImportSummary {
        groups: 0,
        snippets: 0,
    };
    let existing: HashSet<String> = target.groups.iter().map(|g| g.name.clone()).collect();
    for mut g in incoming.groups.drain(..) {
        g.id = Uuid::new_v4();
        if existing.contains(&g.name) {
            g.name = format!("{} (imported)", g.name);
        }
        for s in &mut g.snippets {
            s.id = Uuid::new_v4();
        }
        summary.groups += 1;
        summary.snippets += g.snippets.len();
        target.groups.push(g);
    }
    summary
}

/// Temp file next to `path`, unique per operation (UUID), created exclusively and removed
/// on drop. Never shares or truncates another writer's temp.
struct TempSibling {
    path: PathBuf,
}

impl TempSibling {
    fn write(target: &Path, contents: &[u8]) -> std::io::Result<Self> {
        let dir = target.parent().unwrap_or_else(|| Path::new("."));
        let name = target
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| LIBRARY_FILE.into());
        let path = dir.join(format!(".{name}.tmp-{}", Uuid::new_v4()));
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let guard = TempSibling { path };
        f.write_all(contents)?;
        f.sync_all()?;
        Ok(guard)
    }
}

impl Drop for TempSibling {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Replace `path` atomically with `contents` (temp file + rename).
fn write_atomic(path: &Path, contents: &[u8]) -> Result<(), StoreError> {
    let io = |source| StoreError::Io {
        path: path.to_owned(),
        source,
    };
    let tmp = TempSibling::write(path, contents).map_err(io)?;
    fs::rename(&tmp.path, path).map_err(io)
}

/// Create `path` atomically with `contents`; fails with `AlreadyExists` instead of replacing
/// anything (temp file + hard link).
fn write_new_atomic(path: &Path, contents: &[u8]) -> Result<(), StoreError> {
    let io = |source| StoreError::Io {
        path: path.to_owned(),
        source,
    };
    let tmp = TempSibling::write(path, contents).map_err(io)?;
    fs::hard_link(&tmp.path, path).map_err(io)
}

/// Move an unreadable library aside as `library.corrupt-<stamp>[-n].json` without ever
/// overwriting an earlier backup.
fn move_aside_no_clobber(path: &Path, dir: &Path) -> Result<PathBuf, StoreError> {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    for n in 0..1000 {
        let name = if n == 0 {
            format!("library.corrupt-{stamp}.json")
        } else {
            format!("library.corrupt-{stamp}-{n}.json")
        };
        let backup = dir.join(name);
        match fs::hard_link(path, &backup) {
            Ok(()) => {
                fs::remove_file(path).map_err(|source| StoreError::Io {
                    path: path.to_owned(),
                    source,
                })?;
                return Ok(backup);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(source) => {
                return Err(StoreError::Io {
                    path: path.to_owned(),
                    source,
                })
            }
        }
    }
    Err(StoreError::Invalid(vec![
        "too many corrupt-library backups with the same timestamp".into(),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Group, Snippet};

    fn temp_leftovers(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp-"))
            .collect()
    }

    #[test]
    fn concurrent_saves_and_exports_never_corrupt_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        store.save(&Library::demo()).unwrap();
        let path = store.library_path();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let (store, path, barrier) = (store.clone(), path.clone(), barrier.clone());
                std::thread::spawn(move || {
                    let mut lib = Library::demo();
                    lib.groups[0].name = format!("writer {i}");
                    barrier.wait();
                    for _ in 0..25 {
                        if i % 2 == 0 {
                            store.save(&lib).unwrap();
                        } else {
                            export_to(&path, &lib).unwrap();
                        }
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let final_lib = read_library_file(&path).expect("complete, parseable library");
        assert!(final_lib.groups[0].name.starts_with("writer "));
        read_library_file(&dir.path().join("library.json.bak")).expect("backup is complete too");
        assert!(temp_leftovers(dir.path()).is_empty(), "no temp files left");
    }

    #[test]
    fn dangling_symlink_library_fails_closed_and_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        let link = store.library_path();
        std::os::unix::fs::symlink("/nonexistent/typelune-library.json", &link).unwrap();
        assert!(
            store.load_or_init().is_err(),
            "no demo over a dangling symlink"
        );
        assert!(
            store.save(&Library::demo()).is_err(),
            "save refuses to replace a symlink"
        );
        assert_eq!(
            fs::read_link(&link).unwrap(),
            PathBuf::from("/nonexistent/typelune-library.json"),
            "symlink untouched"
        );
        assert_eq!(
            fs::read_dir(dir.path()).unwrap().count(),
            1,
            "nothing else written"
        );
    }

    #[test]
    fn corrupt_backups_never_overwrite_each_other() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        for round in 0..3 {
            fs::write(store.library_path(), format!("corrupt {round}")).unwrap();
            assert!(matches!(
                store.load_or_init().unwrap(),
                LoadOutcome::Recovered { .. }
            ));
        }
        let mut backups: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("library.corrupt-"))
            .map(|n| fs::read_to_string(dir.path().join(n)).unwrap())
            .collect();
        backups.sort();
        assert_eq!(backups, ["corrupt 0", "corrupt 1", "corrupt 2"]);
    }

    #[test]
    fn developer_fallback_requires_isolated_explicit_dir() {
        let base = PathBuf::from("/Users/example/Library/Application Support");
        let prod = base.join(APP_DIR_NAME);
        let legacy = base.join(LEGACY_APP_DIR_NAME);
        let check = |v: Option<&str>| developer_fallback_dir(v.map(Into::into), &prod, &legacy);
        assert!(check(None).is_err(), "must be explicit");
        assert!(check(Some("")).is_err());
        assert!(check(Some("relative/dir")).is_err());
        assert!(
            check(Some(prod.to_str().unwrap())).is_err(),
            "production refused"
        );
        assert!(
            check(Some(&format!("{}/", prod.display()))).is_err(),
            "trailing slash"
        );
        assert!(
            check(Some(prod.join("sub").to_str().unwrap())).is_err(),
            "inside production"
        );
        assert!(
            check(Some(legacy.to_str().unwrap())).is_err(),
            "legacy refused"
        );
        assert!(
            check(Some(base.to_str().unwrap())).is_err(),
            "parent of real folders refused"
        );
        assert!(
            check(Some("/Users/example/Library/Application Support/aText")).is_err(),
            "foreign app"
        );
        assert_eq!(
            check(Some("/tmp/typelune-dev")).unwrap(),
            PathBuf::from("/tmp/typelune-dev")
        );
    }

    #[test]
    fn creates_demo_then_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        let first = store.load_or_init().unwrap();
        let lib = match first {
            LoadOutcome::CreatedDemo(l) => l,
            other => panic!("expected demo, got {other:?}"),
        };
        assert!(lib.snippet_count() > 5);
        let mut edited = lib.clone();
        edited.groups[0].snippets[0].body = "Xin chào 👋\nline 2".into();
        store.save(&edited).unwrap();
        match store.load_or_init().unwrap() {
            LoadOutcome::Loaded(l) => assert_eq!(l, edited),
            other => panic!("expected load, got {other:?}"),
        }
        assert!(dir.path().join("library.json.bak").exists());
    }

    #[test]
    fn corrupt_file_is_moved_aside_not_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        fs::write(store.library_path(), "{ not json").unwrap();
        match store.load_or_init().unwrap() {
            LoadOutcome::Recovered { backup, error, .. } => {
                assert!(backup.exists());
                assert_eq!(fs::read_to_string(&backup).unwrap(), "{ not json");
                assert!(error.contains("not a valid") || error.contains("Not a valid"));
            }
            other => panic!("expected recovery, got {other:?}"),
        }
    }

    #[test]
    fn rejects_foreign_format() {
        let err = parse_library(r#"{"snippets": []}"#).unwrap_err();
        assert!(err.to_string().contains("format"));
        let err =
            parse_library(r#"{"format":"atext-rs.library","version":99,"groups":[]}"#).unwrap_err();
        assert!(err.to_string().contains("version 99"));
    }

    #[test]
    fn rejects_invalid_json_with_location() {
        let err = parse_library("{\n \"format\": ").unwrap_err();
        assert!(matches!(err, StoreError::Json(_)));
        assert!(err.to_string().contains("line"));
    }

    #[test]
    fn rejects_wrong_types() {
        let json = r#"{"format":"atext-rs.library","version":1,"groups":[{"id":"not-a-uuid","name":"x"}]}"#;
        assert!(matches!(parse_library(json), Err(StoreError::Json(_))));
    }

    #[test]
    fn validation_errors_are_listed() {
        let mut lib = Library::empty();
        let mut g = Group::new("  ");
        let s = Snippet::new("", &"a".repeat(60), "x");
        let mut dup = Snippet::new("", "ok", "y");
        dup.id = s.id;
        g.snippets = vec![s, dup];
        lib.groups.push(g);
        let errs = validate(&lib);
        assert!(errs.iter().any(|e| e.contains("name is empty")));
        assert!(errs.iter().any(|e| e.contains("abbreviation longer")));
        assert!(errs.iter().any(|e| e.contains("id is duplicated")));
        let json = to_json(&lib).unwrap();
        assert!(matches!(parse_library(&json), Err(StoreError::Invalid(_))));
    }

    #[test]
    fn missing_optional_fields_get_defaults() {
        let id = Uuid::new_v4();
        let sid = Uuid::new_v4();
        let json = format!(
            r#"{{"format":"atext-rs.library","version":1,"groups":[{{"id":"{id}","name":"G","snippets":[{{"id":"{sid}","abbreviation":"x"}}]}}]}}"#
        );
        let lib = parse_library(&json).unwrap();
        let s = &lib.groups[0].snippets[0];
        assert!(s.enabled && !s.case_sensitive && s.body.is_empty());
        assert!(lib.settings.keep_delimiter);
    }

    #[test]
    fn export_import_merge() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("export.json");
        let demo = Library::demo();
        export_to(&path, &demo).unwrap();
        let imported = import_from(&path).unwrap();
        assert_eq!(imported, demo);

        let mut target = Library::demo();
        let before = target.snippet_count();
        let summary = merge_into(&mut target, imported);
        assert_eq!(summary.snippets, demo.snippet_count());
        assert_eq!(target.snippet_count(), before + demo.snippet_count());
        assert!(target.groups.iter().any(|g| g.name == "General (imported)"));
        assert!(validate(&target).is_empty(), "merged ids must be unique");
    }

    #[test]
    fn refuses_protected_paths() {
        let home = dirs::home_dir().unwrap();
        for p in [
            home.join("Library/Application Support/aText/data.json"),
            home.join("Desktop/backup.atext"),
            PathBuf::from("/Applications/aText.app/Contents/x.json"),
        ] {
            assert!(is_protected_path(&p), "{}", p.display());
            assert!(matches!(
                export_to(&p, &Library::demo()),
                Err(StoreError::ProtectedPath(_))
            ));
        }
        assert!(!is_protected_path(&home.join("Desktop/snippets.json")));
    }

    #[test]
    fn import_rejects_oversized_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.json");
        let f = fs::File::create(&path).unwrap();
        f.set_len(MAX_FILE_BYTES + 1).unwrap();
        assert!(matches!(
            import_from(&path),
            Err(StoreError::TooLarge { .. })
        ));
    }
}

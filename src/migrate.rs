//! One-time, copy-only migration of the pre-0.3 library.
//!
//! Before v0.3.0 the app used the bundle identifier `dev.atext-rs.poc`, so the library lived
//! in `Application Support/dev.atext-rs.poc/library.json`. On first launch of Typelune, if
//! the new folder has no library yet, that file is copied **byte for byte** into the new
//! folder. Rules:
//!
//! * The legacy file is only read: never moved, rewritten or deleted.
//! * An existing library in the new folder is never overwritten. This is checked up front,
//!   and the final step is an atomic no-clobber hard link from a private temp file, so a
//!   concurrent creator (another thread, process or app) always wins.
//! * Each operation writes its own uniquely named temp file (created exclusively) and only
//!   ever removes that file, on every exit path. Other operations' temps are never touched.
//! * The legacy file must parse and validate first. If it does not, nothing is written and
//!   the caller must fail closed (no demo data, saving disabled).
//! * After copying, the new file is re-read and compared with the source bytes.
//! * A `migrated-from.json` note records where the copy came from (path, size, SHA-256,
//!   time). It contains no library content.

use crate::store::{self, LIBRARY_FILE, MAX_FILE_BYTES};
use sha2::{Digest, Sha256};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

pub const MIGRATION_NOTE_FILE: &str = "migrated-from.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationOutcome {
    /// The new folder already has a library; nothing was copied.
    TargetExists,
    /// There is no legacy library to copy.
    NoLegacy,
    Copied {
        bytes: u64,
        sha256: String,
    },
}

#[derive(Debug)]
pub enum MigrationError {
    LegacyUnreadable {
        path: PathBuf,
        source: std::io::Error,
    },
    LegacyInvalid {
        path: PathBuf,
        reason: String,
    },
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    VerifyFailed {
        path: PathBuf,
    },
}

impl fmt::Display for MigrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MigrationError::LegacyUnreadable { path, source } => write!(
                f,
                "The library from the previous version at {} could not be read ({source}). It was left untouched.",
                path.display()
            ),
            MigrationError::LegacyInvalid { path, reason } => write!(
                f,
                "The library from the previous version at {} is not valid, so it was not copied and was left untouched: {reason}",
                path.display()
            ),
            MigrationError::Io { path, source } => {
                write!(f, "Copying the previous library to {} failed: {source}", path.display())
            }
            MigrationError::VerifyFailed { path } => write!(
                f,
                "The copied library at {} does not match the original byte for byte.",
                path.display()
            ),
        }
    }
}

impl std::error::Error for MigrationError {}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Exclusively created temp file with a name unique to one operation; removed on drop.
struct OwnedTemp {
    path: PathBuf,
    file: Option<fs::File>,
}

impl OwnedTemp {
    fn create(dir: &Path) -> std::io::Result<Self> {
        let path = dir.join(format!(
            ".{LIBRARY_FILE}.migrating-{}",
            uuid::Uuid::new_v4()
        ));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        Ok(Self {
            path,
            file: Some(file),
        })
    }

    fn write_all_and_sync(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        let f = self.file.as_mut().expect("temp file is open");
        f.write_all(bytes)?;
        f.sync_all()
    }
}

impl Drop for OwnedTemp {
    fn drop(&mut self) {
        self.file.take();
        let _ = fs::remove_file(&self.path);
    }
}

/// Copy `legacy_dir/library.json` to `target_dir/library.json` if, and only if, the target
/// does not exist and the legacy file is a valid library. See the module docs.
pub fn copy_legacy_once(
    legacy_dir: &Path,
    target_dir: &Path,
) -> Result<MigrationOutcome, MigrationError> {
    let target = target_dir.join(LIBRARY_FILE);
    if target.symlink_metadata().is_ok() {
        return Ok(MigrationOutcome::TargetExists);
    }
    if legacy_dir == target_dir {
        return Ok(MigrationOutcome::NoLegacy);
    }
    let legacy = legacy_dir.join(LIBRARY_FILE);
    let meta = match legacy.metadata() {
        Ok(m) => m,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(MigrationOutcome::NoLegacy),
        Err(source) => {
            return Err(MigrationError::LegacyUnreadable {
                path: legacy,
                source,
            })
        }
    };
    let too_large = || MigrationError::LegacyInvalid {
        path: legacy.clone(),
        reason: format!("larger than {MAX_FILE_BYTES} bytes"),
    };
    if meta.len() > MAX_FILE_BYTES {
        return Err(too_large());
    }
    let bytes = fs::read(&legacy).map_err(|source| MigrationError::LegacyUnreadable {
        path: legacy.clone(),
        source,
    })?;
    // The file may have grown between metadata() and read(); enforce the limit on what we read.
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(too_large());
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| MigrationError::LegacyInvalid {
        path: legacy.clone(),
        reason: "not UTF-8".into(),
    })?;
    store::parse_library(text).map_err(|e| MigrationError::LegacyInvalid {
        path: legacy.clone(),
        reason: e.to_string(),
    })?;

    let io = |path: &Path| {
        let path = path.to_owned();
        move |source| MigrationError::Io { path, source }
    };
    fs::create_dir_all(target_dir).map_err(io(target_dir))?;
    let mut tmp = OwnedTemp::create(target_dir).map_err(io(target_dir))?;
    tmp.write_all_and_sync(&bytes).map_err(io(&tmp.path))?;
    // Atomic and no-clobber: fails with AlreadyExists if anything created the target meanwhile.
    let linked = fs::hard_link(&tmp.path, &target);
    drop(tmp); // removes only our own temp file
    match linked {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {
            return Ok(MigrationOutcome::TargetExists)
        }
        Err(source) => {
            return Err(MigrationError::Io {
                path: target,
                source,
            })
        }
    }

    let copied = fs::read(&target).map_err(io(&target))?;
    if copied != bytes {
        return Err(MigrationError::VerifyFailed { path: target });
    }
    let sha256 = sha256_hex(&bytes);
    let note = serde_json::json!({
        "migrated_from": legacy.display().to_string(),
        "bytes": bytes.len(),
        "sha256": sha256,
        "migrated_at": chrono::Local::now().to_rfc3339(),
        "note": "Copied once from the pre-0.3 location. The original file was not modified.",
    });
    if let Ok(mut f) = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target_dir.join(MIGRATION_NOTE_FILE))
    {
        let _ = f.write_all(
            serde_json::to_string_pretty(&note)
                .unwrap_or_default()
                .as_bytes(),
        );
    }
    Ok(MigrationOutcome::Copied {
        bytes: bytes.len() as u64,
        sha256,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Library;
    use crate::store::{to_json, Store};
    use std::os::unix::fs::PermissionsExt;

    /// A valid legacy library with unusual-but-valid trailing whitespace, so a byte-for-byte
    /// copy is distinguishable from a re-serialisation. Built once: `Library::demo()` makes
    /// fresh UUIDs on every call.
    fn legacy_bytes() -> Vec<u8> {
        static BYTES: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
        BYTES
            .get_or_init(|| {
                let mut json = to_json(&Library::demo()).unwrap();
                json.push_str("\n\n");
                json.into_bytes()
            })
            .clone()
    }

    fn running_as_root() -> bool {
        extern "C" {
            fn geteuid() -> u32;
        }
        // SAFETY: pure query.
        unsafe { geteuid() == 0 }
    }

    fn leftover_temps(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| n.contains("migrating"))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn dirs() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let legacy = root.path().join("dev.atext-rs.poc");
        let target = root.path().join("io.github.tranxuanliem.typelune");
        (root, legacy, target)
    }

    fn with_legacy(legacy: &Path, bytes: &[u8]) {
        fs::create_dir_all(legacy).unwrap();
        fs::write(legacy.join(LIBRARY_FILE), bytes).unwrap();
    }

    #[test]
    fn copies_byte_for_byte_and_leaves_legacy_untouched() {
        let (_root, legacy, target) = dirs();
        with_legacy(&legacy, &legacy_bytes());
        let src = legacy.join(LIBRARY_FILE);
        let mtime = fs::metadata(&src).unwrap().modified().unwrap();

        let out = copy_legacy_once(&legacy, &target).unwrap();
        let MigrationOutcome::Copied { bytes, sha256 } = out else {
            panic!("expected copy, got {out:?}")
        };
        assert_eq!(fs::read(target.join(LIBRARY_FILE)).unwrap(), legacy_bytes());
        assert_eq!(bytes as usize, legacy_bytes().len());
        assert_eq!(sha256, sha256_hex(&legacy_bytes()));
        assert_eq!(
            fs::read(&src).unwrap(),
            legacy_bytes(),
            "legacy bytes unchanged"
        );
        assert_eq!(
            fs::metadata(&src).unwrap().modified().unwrap(),
            mtime,
            "legacy mtime unchanged"
        );
        let note = fs::read_to_string(target.join(MIGRATION_NOTE_FILE)).unwrap();
        assert!(
            note.contains(&sha256) && !note.contains("Hello there"),
            "note has no content"
        );
        assert_eq!(
            copy_legacy_once(&legacy, &target).unwrap(),
            MigrationOutcome::TargetExists
        );
        assert!(
            leftover_temps(&target).is_empty(),
            "no temp files left behind"
        );
    }

    #[test]
    fn never_overwrites_an_existing_target() {
        let (_root, legacy, target) = dirs();
        with_legacy(&legacy, &legacy_bytes());
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join(LIBRARY_FILE), b"existing new-location library").unwrap();
        assert_eq!(
            copy_legacy_once(&legacy, &target).unwrap(),
            MigrationOutcome::TargetExists
        );
        assert_eq!(
            fs::read(target.join(LIBRARY_FILE)).unwrap(),
            b"existing new-location library"
        );
        assert!(!target.join(MIGRATION_NOTE_FILE).exists());
    }

    #[test]
    fn dangling_symlink_target_counts_as_existing() {
        let (_root, legacy, target) = dirs();
        with_legacy(&legacy, &legacy_bytes());
        fs::create_dir_all(&target).unwrap();
        std::os::unix::fs::symlink("/nonexistent/elsewhere.json", target.join(LIBRARY_FILE))
            .unwrap();
        assert_eq!(
            copy_legacy_once(&legacy, &target).unwrap(),
            MigrationOutcome::TargetExists
        );
    }

    #[test]
    fn concurrent_migrations_in_one_process_copy_exactly_once() {
        // Threads race the same migration: exactly one copies, the rest see the target, the
        // target is the complete source, and no temp file survives.
        for _round in 0..20 {
            let (_root, legacy, target) = dirs();
            with_legacy(&legacy, &legacy_bytes());
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    let (legacy, target, barrier) =
                        (legacy.clone(), target.clone(), barrier.clone());
                    std::thread::spawn(move || {
                        barrier.wait();
                        copy_legacy_once(&legacy, &target).unwrap()
                    })
                })
                .collect();
            let outcomes: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
            let copied = outcomes
                .iter()
                .filter(|o| matches!(o, MigrationOutcome::Copied { .. }))
                .count();
            assert_eq!(copied, 1, "exactly one copy: {outcomes:?}");
            assert!(outcomes.iter().all(|o| matches!(
                o,
                MigrationOutcome::Copied { .. } | MigrationOutcome::TargetExists
            )));
            assert_eq!(
                fs::read(target.join(LIBRARY_FILE)).unwrap(),
                legacy_bytes(),
                "never partial"
            );
            assert!(leftover_temps(&target).is_empty(), "no leftovers");
            assert_eq!(fs::read(legacy.join(LIBRARY_FILE)).unwrap(), legacy_bytes());
        }
    }

    #[test]
    fn foreign_temp_files_are_never_removed() {
        let (_root, legacy, target) = dirs();
        with_legacy(&legacy, &legacy_bytes());
        fs::create_dir_all(&target).unwrap();
        // Same naming scheme an older build (PID-based) or another operation might use.
        let foreign = target.join(format!(".{LIBRARY_FILE}.migrating-{}", std::process::id()));
        fs::write(&foreign, b"another operation").unwrap();
        assert!(matches!(
            copy_legacy_once(&legacy, &target).unwrap(),
            MigrationOutcome::Copied { .. }
        ));
        assert_eq!(fs::read(&foreign).unwrap(), b"another operation");
    }

    #[test]
    fn no_legacy_means_nothing_written() {
        let (_root, legacy, target) = dirs();
        assert_eq!(
            copy_legacy_once(&legacy, &target).unwrap(),
            MigrationOutcome::NoLegacy
        );
        assert!(!target.exists());
    }

    #[test]
    fn invalid_legacy_fails_closed_without_writing() {
        let (_root, legacy, target) = dirs();
        for bad in [
            &b"{ not json"[..],
            br#"{"format":"other","version":1}"#,
            &[0xff, 0xfe][..],
        ] {
            with_legacy(&legacy, bad);
            let err = copy_legacy_once(&legacy, &target).unwrap_err();
            assert!(matches!(err, MigrationError::LegacyInvalid { .. }), "{err}");
            assert!(!target.join(LIBRARY_FILE).exists());
            assert_eq!(
                fs::read(legacy.join(LIBRARY_FILE)).unwrap(),
                bad,
                "legacy untouched"
            );
        }
    }

    #[test]
    fn oversized_legacy_fails_closed() {
        let (_root, legacy, target) = dirs();
        fs::create_dir_all(&legacy).unwrap();
        let f = fs::File::create(legacy.join(LIBRARY_FILE)).unwrap();
        f.set_len(MAX_FILE_BYTES + 1).unwrap();
        assert!(matches!(
            copy_legacy_once(&legacy, &target),
            Err(MigrationError::LegacyInvalid { .. })
        ));
        assert!(!target.exists());
    }

    #[test]
    fn unreadable_legacy_fails_closed() {
        if running_as_root() {
            eprintln!("skipped: permission bits do not apply to root");
            return;
        }
        let (_root, legacy, target) = dirs();
        with_legacy(&legacy, &legacy_bytes());
        let src = legacy.join(LIBRARY_FILE);
        fs::set_permissions(&src, fs::Permissions::from_mode(0o000)).unwrap();
        let result = copy_legacy_once(&legacy, &target);
        fs::set_permissions(&src, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(
            matches!(result, Err(MigrationError::LegacyUnreadable { .. })),
            "{result:?}"
        );
        assert!(!target.join(LIBRARY_FILE).exists());
    }

    #[test]
    fn unwritable_target_reports_io_error_and_leaves_legacy() {
        if running_as_root() {
            eprintln!("skipped: permission bits do not apply to root");
            return;
        }
        let (root, legacy, _target) = dirs();
        with_legacy(&legacy, &legacy_bytes());
        let locked = root.path().join("locked");
        fs::create_dir_all(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o500)).unwrap();
        let target = locked.join("io.github.tranxuanliem.typelune");
        let result = copy_legacy_once(&legacy, &target);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            matches!(result, Err(MigrationError::Io { .. })),
            "{result:?}"
        );
        assert!(!target.exists());
        assert_eq!(fs::read(legacy.join(LIBRARY_FILE)).unwrap(), legacy_bytes());
    }

    #[test]
    fn copied_library_loads_without_rewrite() {
        let (_root, legacy, target) = dirs();
        with_legacy(&legacy, &legacy_bytes());
        copy_legacy_once(&legacy, &target).unwrap();
        let store = Store::new(&target);
        assert!(matches!(
            store.load_or_init().unwrap(),
            store::LoadOutcome::Loaded(_)
        ));
        assert_eq!(fs::read(target.join(LIBRARY_FILE)).unwrap(), legacy_bytes());
    }
}

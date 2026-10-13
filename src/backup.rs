//! Local, timestamped library snapshots in `<data folder>/backups/`.
//!
//! * **Daily.** At most one per calendar day, `library-YYYYMMDD-daily.json`, taken by the
//!   save worker just before the day's first overwrite of `library.json`. It holds the
//!   previous on-disk version. The name is fixed per day and created with an atomic
//!   no-clobber link, so racing writers produce exactly one.
//! * **Manual** (*Backup Now*) and **pre-restore** snapshots:
//!   `library-YYYYMMDD-HHMMSS[-n]-manual.json` / `-pre-restore.json`.
//! * **Retention.** At most [`RETENTION`] snapshots are kept; older ones are deleted. Today's
//!   daily snapshot is never pruned, so a burst of manual snapshots cannot cause a second
//!   daily snapshot (with different content) later the same day. Only regular files whose
//!   names match the patterns above are ever listed, read or deleted.
//! * **Writes.** Every snapshot is a complete, validated library. It is written to an
//!   exclusive, per-operation temp file and then hard-linked into place, so an existing
//!   snapshot is never replaced and a half-written file is never visible.
//! * **Links.** The `backups` folder must be a real folder, and snapshots are opened with
//!   `O_NOFOLLOW`: symbolic links are refused, never followed. If today's daily name is
//!   taken by something that is not a valid snapshot (a link, a folder, a damaged file), it
//!   is left in place and an error is reported instead of silently skipping. These are
//!   static checks; a hostile process swapping folders while Typelune runs is out of scope.
//!
//! The existing `library.json.bak` (previous version, replaced on every save) is unchanged.

use crate::model::Library;
use crate::store::{self, StoreError, MAX_FILE_BYTES};
use chrono::{DateTime, Local, NaiveDate};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

pub const BACKUP_DIR: &str = "backups";
/// Snapshots kept after pruning (all kinds together).
pub const RETENTION: usize = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupKind {
    Daily,
    Manual,
    PreRestore,
}

impl BackupKind {
    fn suffix(self) -> &'static str {
        match self {
            BackupKind::Daily => "daily",
            BackupKind::Manual => "manual",
            BackupKind::PreRestore => "pre-restore",
        }
    }
}

/// One snapshot as listed to the UI. Contains no library content.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct BackupInfo {
    pub name: String,
    pub kind: BackupKind,
    /// `YYYY-MM-DD`.
    pub date: String,
    /// `HH:MM:SS`; `None` for daily snapshots (taken before the day's first save).
    pub time: Option<String>,
    pub bytes: u64,
}

/// Outcome of the most recent automatic snapshot attempt (session only).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct BackupStatus {
    pub last_daily: Option<String>,
    pub last_error: Option<String>,
    /// Day (`YYYYMMDD`) whose daily snapshot is known to exist and be valid.
    #[serde(skip)]
    pub verified_day: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedName {
    date: String,
    time: Option<String>,
    seq: u32,
    kind: BackupKind,
}

impl ParsedName {
    fn sort_key(&self) -> (String, String, u32) {
        (
            self.date.clone(),
            self.time.clone().unwrap_or_else(|| "000000".into()),
            self.seq,
        )
    }
}

fn all_digits(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| b.is_ascii_digit())
}

/// Strict parser for snapshot names. Anything else (including paths) is rejected.
fn parse_name(name: &str) -> Option<ParsedName> {
    let rest = name.strip_prefix("library-")?.strip_suffix(".json")?;
    let (stamp, kind) = if let Some(s) = rest.strip_suffix("-daily") {
        (s, BackupKind::Daily)
    } else if let Some(s) = rest.strip_suffix("-manual") {
        (s, BackupKind::Manual)
    } else {
        (rest.strip_suffix("-pre-restore")?, BackupKind::PreRestore)
    };
    let parts: Vec<&str> = stamp.split('-').collect();
    let parsed = match (kind, parts.as_slice()) {
        (BackupKind::Daily, [date]) if all_digits(date, 8) => ParsedName {
            date: (*date).into(),
            time: None,
            seq: 0,
            kind,
        },
        (BackupKind::Manual | BackupKind::PreRestore, [date, time, rest @ ..])
            if all_digits(date, 8) && all_digits(time, 6) && rest.len() <= 1 =>
        {
            let seq = match rest {
                [] => 0,
                [n] if !n.is_empty() && n.len() <= 3 && n.bytes().all(|b| b.is_ascii_digit()) => {
                    n.parse().ok()?
                }
                _ => return None,
            };
            ParsedName {
                date: (*date).into(),
                time: Some((*time).into()),
                seq,
                kind,
            }
        }
        _ => return None,
    };
    NaiveDate::parse_from_str(&parsed.date, "%Y%m%d").ok()?;
    Some(parsed)
}

pub fn backup_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(BACKUP_DIR)
}

fn io_err(path: &Path) -> impl Fn(std::io::Error) -> StoreError + '_ {
    move |source| StoreError::Io {
        path: path.to_owned(),
        source,
    }
}

fn refuse_link(path: &Path) -> StoreError {
    StoreError::Invalid(vec![format!(
        "{} is a symbolic link or not a regular folder/file; Typelune will not follow it",
        path.display()
    )])
}

/// The backups folder, which must be a real folder (created owner-only if missing).
fn ensure_dir(data_dir: &Path) -> Result<PathBuf, StoreError> {
    let dir = backup_dir(data_dir);
    match fs::symlink_metadata(&dir) {
        Ok(m) if m.file_type().is_dir() => Ok(dir),
        Ok(_) => Err(refuse_link(&dir)),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            match fs::create_dir(&dir) {
                Ok(()) => {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
                }
                Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
                Err(e) => return Err(io_err(&dir)(e)),
            }
            // Re-check: someone may have raced a symlink into place.
            match fs::symlink_metadata(&dir) {
                Ok(m) if m.file_type().is_dir() => Ok(dir),
                Ok(_) => Err(refuse_link(&dir)),
                Err(e) => Err(io_err(&dir)(e)),
            }
        }
        Err(e) => Err(io_err(&dir)(e)),
    }
}

/// Validate that `bytes` is a complete library before it becomes a snapshot.
fn validated(bytes: &[u8]) -> Result<(), StoreError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| StoreError::Invalid(vec!["library is not valid UTF-8".into()]))?;
    store::parse_library(text).map(|_| ())
}

/// Write a new snapshot of `bytes` (a library JSON document). Returns its name. Never
/// replaces an existing snapshot; prunes to [`RETENTION`] afterwards.
pub fn write_snapshot(
    data_dir: &Path,
    kind: BackupKind,
    bytes: &[u8],
    now: DateTime<Local>,
) -> Result<String, StoreError> {
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(StoreError::TooLarge {
            path: backup_dir(data_dir),
            bytes: bytes.len() as u64,
        });
    }
    validated(bytes)?;
    let dir = ensure_dir(data_dir)?;
    let date = now.format("%Y%m%d").to_string();
    let today_daily = daily_name(now);
    let name = if kind == BackupKind::Daily {
        let name = format!("library-{date}-daily.json");
        store::write_new_atomic(&dir.join(&name), bytes)?;
        name
    } else {
        let time = now.format("%H%M%S").to_string();
        let mut created = None;
        for n in 0..1000 {
            let name = if n == 0 {
                format!("library-{date}-{time}-{}.json", kind.suffix())
            } else {
                format!("library-{date}-{time}-{n}-{}.json", kind.suffix())
            };
            match store::write_new_atomic(&dir.join(&name), bytes) {
                Ok(()) => {
                    created = Some(name);
                    break;
                }
                Err(StoreError::Io { source, .. }) if source.kind() == ErrorKind::AlreadyExists => {
                    continue
                }
                Err(e) => return Err(e),
            }
        }
        created.ok_or_else(|| {
            StoreError::Invalid(vec!["too many snapshots with the same timestamp".into()])
        })?
    };
    prune(data_dir, RETENTION, &[name.as_str(), today_daily.as_str()])?;
    Ok(name)
}

fn daily_name(now: DateTime<Local>) -> String {
    format!("library-{}-daily.json", now.format("%Y%m%d"))
}

/// Daily snapshot of the version about to be overwritten. `Ok(None)` when today's valid
/// daily snapshot already exists (only the first overwrite of a day is captured). An
/// existing entry with today's name that is not a valid snapshot is an error and is left
/// untouched.
pub fn daily_before_overwrite(
    data_dir: &Path,
    previous: &[u8],
    now: DateTime<Local>,
) -> Result<Option<String>, StoreError> {
    ensure_dir(data_dir)?;
    let name = daily_name(now);
    match fs::symlink_metadata(backup_dir(data_dir).join(&name)) {
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(io_err(&backup_dir(data_dir))(e)),
        Ok(_) => return existing_daily(data_dir, &name),
    }
    match write_snapshot(data_dir, BackupKind::Daily, previous, now) {
        Ok(n) => Ok(Some(n)),
        // Lost a race against another writer: theirs must be a valid snapshot too.
        Err(StoreError::Io { source, .. }) if source.kind() == ErrorKind::AlreadyExists => {
            existing_daily(data_dir, &name)
        }
        Err(e) => Err(e),
    }
}

fn existing_daily(data_dir: &Path, name: &str) -> Result<Option<String>, StoreError> {
    match read(data_dir, name) {
        Ok(_) => Ok(None),
        Err(e) => Err(StoreError::Invalid(vec![format!(
            "today's daily snapshot {name} exists but is not a valid snapshot ({e}); it was left in place and no daily snapshot was taken"
        )])),
    }
}

/// Snapshots, newest first. A missing folder is an empty list.
pub fn list(data_dir: &Path) -> Result<Vec<BackupInfo>, StoreError> {
    let dir = backup_dir(data_dir);
    match fs::symlink_metadata(&dir) {
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io_err(&dir)(e)),
        Ok(m) if !m.file_type().is_dir() => return Err(refuse_link(&dir)),
        Ok(_) => {}
    }
    let mut found: Vec<(ParsedName, BackupInfo)> = Vec::new();
    for entry in fs::read_dir(&dir).map_err(io_err(&dir))? {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(parsed) = parse_name(&name) else {
            continue;
        };
        // DirEntry::metadata does not follow symlinks.
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.file_type().is_file() {
            continue;
        }
        let d = &parsed.date;
        let info = BackupInfo {
            name,
            kind: parsed.kind,
            date: format!("{}-{}-{}", &d[..4], &d[4..6], &d[6..]),
            time: parsed
                .time
                .as_ref()
                .map(|t| format!("{}:{}:{}", &t[..2], &t[2..4], &t[4..])),
            bytes: meta.len(),
        };
        found.push((parsed, info));
    }
    found.sort_by(|a, b| {
        b.0.sort_key()
            .cmp(&a.0.sort_key())
            .then_with(|| b.1.name.cmp(&a.1.name))
    });
    Ok(found.into_iter().map(|(_, i)| i).collect())
}

/// Keep at most `keep` snapshots: every listed name in `protect`, then the newest others.
/// Returns the number removed. Files already removed by a concurrent prune are ignored.
pub fn prune(data_dir: &Path, keep: usize, protect: &[&str]) -> Result<usize, StoreError> {
    let dir = backup_dir(data_dir);
    let all = list(data_dir)?;
    let protected = all
        .iter()
        .filter(|i| protect.contains(&i.name.as_str()))
        .count();
    let others = keep.saturating_sub(protected);
    let mut removed = 0;
    let doomed = all
        .into_iter()
        .filter(|i| !protect.contains(&i.name.as_str()))
        .skip(others);
    for info in doomed {
        let path = dir.join(&info.name);
        match fs::symlink_metadata(&path) {
            Ok(m) if m.file_type().is_file() => {}
            _ => continue,
        }
        match fs::remove_file(&path) {
            Ok(()) => removed += 1,
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(io_err(&path)(e)),
        }
    }
    Ok(removed)
}

/// Read and validate one snapshot. The name must be a snapshot name (no paths); the file
/// is opened without following links, must be a regular file and at most
/// [`MAX_FILE_BYTES`]. Corrupt or oversized snapshots fail closed.
pub fn read(data_dir: &Path, name: &str) -> Result<Library, StoreError> {
    if parse_name(name).is_none() {
        return Err(StoreError::Invalid(vec![format!(
            "“{name}” is not a Typelune backup name"
        )]));
    }
    let dir = backup_dir(data_dir);
    match fs::symlink_metadata(&dir) {
        Ok(m) if m.file_type().is_dir() => {}
        Ok(_) => return Err(refuse_link(&dir)),
        Err(e) => return Err(io_err(&dir)(e)),
    }
    let buf = store::read_regular_file(&dir.join(name), MAX_FILE_BYTES, true)?;
    let text = std::str::from_utf8(&buf)
        .map_err(|_| StoreError::Invalid(vec![format!("{name} is not valid UTF-8")]))?;
    store::parse_library(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, mo, d, h, mi, s).single().unwrap()
    }

    fn lib_bytes(tag: &str) -> Vec<u8> {
        let mut lib = Library::demo();
        lib.groups[0].name = tag.into();
        store::to_json(&lib).unwrap().into_bytes()
    }

    fn names(dir: &Path) -> Vec<String> {
        list(dir).unwrap().into_iter().map(|i| i.name).collect()
    }

    #[test]
    fn names_are_parsed_strictly() {
        assert!(parse_name("library-20261013-daily.json").is_some());
        assert!(parse_name("library-20261013-101500-manual.json").is_some());
        assert!(parse_name("library-20261013-101500-7-pre-restore.json").is_some());
        for bad in [
            "library-20261013-101500-daily.json",
            "library-2026101-daily.json",
            "library-20261332-daily.json",
            "library-20261013-manual.json",
            "library-20261013-101500-1234-manual.json",
            "../library-20261013-daily.json",
            "library-20261013-daily.json/x",
            "library.json",
            "library-20261013-daily.json.tmp",
        ] {
            assert!(parse_name(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn one_daily_snapshot_per_day_even_when_racing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let now = at(2026, 10, 13, 9, 0, 0);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let (dir, barrier) = (dir.clone(), barrier.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    daily_before_overwrite(&dir, &lib_bytes(&format!("v{i}")), now).unwrap()
                })
            })
            .collect();
        let created: Vec<_> = handles
            .into_iter()
            .filter_map(|h| h.join().unwrap())
            .collect();
        assert_eq!(created.len(), 1, "exactly one writer created the daily");
        assert_eq!(names(&dir), ["library-20261013-daily.json"]);
        read(&dir, "library-20261013-daily.json").expect("complete snapshot");
        // Later the same day: nothing new. Next day: a new one.
        assert_eq!(
            daily_before_overwrite(&dir, &lib_bytes("x"), at(2026, 10, 13, 23, 0, 0)).unwrap(),
            None
        );
        assert!(
            daily_before_overwrite(&dir, &lib_bytes("y"), at(2026, 10, 14, 8, 0, 0))
                .unwrap()
                .is_some()
        );
        let leftovers: Vec<_> = fs::read_dir(backup_dir(&dir))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "no temp files left");
    }

    #[test]
    fn invalid_or_oversized_content_is_never_snapshotted() {
        let tmp = tempfile::tempdir().unwrap();
        let now = at(2026, 10, 13, 9, 0, 0);
        assert!(write_snapshot(tmp.path(), BackupKind::Manual, b"{ broken", now).is_err());
        assert!(daily_before_overwrite(tmp.path(), b"\xff\xfe", now).is_err());
        let big = vec![b' '; MAX_FILE_BYTES as usize + 1];
        assert!(matches!(
            write_snapshot(tmp.path(), BackupKind::Manual, &big, now),
            Err(StoreError::TooLarge { .. })
        ));
        assert!(names(tmp.path()).is_empty());
    }

    #[test]
    fn retention_keeps_newest_thirty_and_ignores_foreign_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        ensure_dir(dir).unwrap();
        // A foreign file and a lookalike directory must never be deleted or listed.
        fs::write(backup_dir(dir).join("notes.txt"), "keep me").unwrap();
        fs::create_dir(backup_dir(dir).join("library-20200101-daily.json")).unwrap();
        for day in 1..=31u32 {
            daily_before_overwrite(dir, &lib_bytes("d"), at(2026, 8, day, 9, 0, 0)).unwrap();
        }
        for s in 0..5u32 {
            write_snapshot(
                dir,
                BackupKind::Manual,
                &lib_bytes("m"),
                at(2026, 9, 1, 10, 0, s),
            )
            .unwrap();
        }
        let all = names(dir);
        assert_eq!(all.len(), RETENTION);
        assert_eq!(
            all[0], "library-20260901-100004-manual.json",
            "newest first"
        );
        assert!(!all.contains(&"library-20260801-daily.json".to_owned()));
        assert!(backup_dir(dir).join("notes.txt").exists());
        assert!(backup_dir(dir).join("library-20200101-daily.json").is_dir());
    }

    #[test]
    fn existing_folder_or_damaged_daily_is_an_error_not_a_skip() {
        let now = at(2026, 10, 13, 9, 0, 0);
        let tmp = tempfile::tempdir().unwrap();
        ensure_dir(tmp.path()).unwrap();
        fs::create_dir(backup_dir(tmp.path()).join("library-20261013-daily.json")).unwrap();
        assert!(daily_before_overwrite(tmp.path(), &lib_bytes("x"), now).is_err());
        let tmp = tempfile::tempdir().unwrap();
        ensure_dir(tmp.path()).unwrap();
        let damaged = backup_dir(tmp.path()).join("library-20261013-daily.json");
        fs::write(&damaged, "{ half").unwrap();
        assert!(daily_before_overwrite(tmp.path(), &lib_bytes("x"), now).is_err());
        assert_eq!(
            fs::read_to_string(&damaged).unwrap(),
            "{ half",
            "left in place"
        );
    }

    #[test]
    fn todays_daily_survives_a_burst_of_manual_snapshots() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        // Older history so retention is under pressure from both sides.
        for day in 1..=10u32 {
            daily_before_overwrite(dir, &lib_bytes("old"), at(2026, 9, day, 9, 0, 0)).unwrap();
        }
        let first = daily_before_overwrite(dir, &lib_bytes("A"), at(2026, 10, 13, 8, 0, 0))
            .unwrap()
            .unwrap();
        for s in 0..(RETENTION as u32 + 5) {
            write_snapshot(
                dir,
                BackupKind::Manual,
                &lib_bytes("m"),
                at(2026, 10, 13, 9, s / 60, s % 60),
            )
            .unwrap();
            assert!(list(dir).unwrap().len() <= RETENTION, "never above the cap");
        }
        // The day's next overwrite must not take a second daily snapshot.
        assert_eq!(
            daily_before_overwrite(dir, &lib_bytes("B"), at(2026, 10, 13, 18, 0, 0)).unwrap(),
            None
        );
        assert_eq!(
            read(dir, &first).unwrap().groups[0].name,
            "A",
            "original kept"
        );
        let all = names(dir);
        assert_eq!(all.len(), RETENTION);
        assert_eq!(all.iter().filter(|n| n.ends_with("-daily.json")).count(), 1);
        // Prune with protected names still respects the cap.
        prune(dir, 3, &[first.as_str()]).unwrap();
        let left = names(dir);
        assert_eq!(left.len(), 3);
        assert!(left.contains(&first));
    }

    #[test]
    fn same_second_snapshots_never_overwrite_each_other() {
        let tmp = tempfile::tempdir().unwrap();
        let now = at(2026, 10, 13, 9, 0, 0);
        let a = write_snapshot(tmp.path(), BackupKind::Manual, &lib_bytes("a"), now).unwrap();
        let b = write_snapshot(tmp.path(), BackupKind::Manual, &lib_bytes("b"), now).unwrap();
        assert_ne!(a, b);
        assert_eq!(read(tmp.path(), &a).unwrap().groups[0].name, "a");
        assert_eq!(read(tmp.path(), &b).unwrap().groups[0].name, "b");
    }

    #[test]
    fn symlinks_are_refused_not_followed() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let now = at(2026, 10, 13, 9, 0, 0);
        // Snapshot name pointing elsewhere.
        ensure_dir(tmp.path()).unwrap();
        let target = outside.path().join("victim.json");
        fs::write(&target, lib_bytes("outside")).unwrap();
        let link = backup_dir(tmp.path()).join("library-20261012-daily.json");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(read(tmp.path(), "library-20261012-daily.json").is_err());
        assert!(names(tmp.path()).is_empty(), "symlinks are not listed");
        // Today's daily name taken by a dangling symlink: reported, never replaced or followed.
        let dangling = backup_dir(tmp.path()).join("library-20261013-daily.json");
        std::os::unix::fs::symlink(outside.path().join("nothing.json"), &dangling).unwrap();
        let err = daily_before_overwrite(tmp.path(), &lib_bytes("x"), now).unwrap_err();
        assert!(err.to_string().contains("not a valid snapshot"), "{err}");
        assert!(fs::symlink_metadata(&dangling)
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(!outside.path().join("nothing.json").exists());
        // The backups folder itself as a symlink: everything refused.
        let tmp2 = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), backup_dir(tmp2.path())).unwrap();
        assert!(write_snapshot(tmp2.path(), BackupKind::Manual, &lib_bytes("x"), now).is_err());
        assert!(list(tmp2.path()).is_err());
        // Even when the outside folder already has today's daily name, the linked parent is
        // refused rather than treated as "done".
        fs::copy(&target, outside.path().join("library-20261013-daily.json")).unwrap();
        assert!(daily_before_overwrite(tmp2.path(), &lib_bytes("x"), now).is_err());
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 2);
        // Pruning never follows or deletes links.
        prune(tmp.path(), 0, &[]).unwrap();
        assert!(fs::symlink_metadata(&link).is_ok());
        assert!(target.exists());
    }

    #[test]
    fn named_pipe_snapshot_is_refused_without_blocking() {
        let tmp = tempfile::tempdir().unwrap();
        ensure_dir(tmp.path()).unwrap();
        let name = "library-20261013-090000-manual.json";
        let fifo = backup_dir(tmp.path()).join(name);
        assert!(std::process::Command::new("/usr/bin/mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success());
        let (tx, rx) = std::sync::mpsc::channel();
        let dir = tmp.path().to_path_buf();
        std::thread::spawn(move || {
            let _ = tx.send(read(&dir, name).is_err());
        });
        assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(5)), Ok(true));
        assert!(names(tmp.path()).is_empty(), "pipes are not listed");
    }

    #[test]
    fn corrupt_or_oversized_snapshots_fail_closed() {
        let tmp = tempfile::tempdir().unwrap();
        ensure_dir(tmp.path()).unwrap();
        let bad = backup_dir(tmp.path()).join("library-20261013-090000-manual.json");
        fs::write(&bad, "{ not json").unwrap();
        assert!(read(tmp.path(), "library-20261013-090000-manual.json").is_err());
        let big = backup_dir(tmp.path()).join("library-20261013-090001-manual.json");
        fs::File::create(&big)
            .unwrap()
            .set_len(MAX_FILE_BYTES + 1)
            .unwrap();
        assert!(matches!(
            read(tmp.path(), "library-20261013-090001-manual.json"),
            Err(StoreError::TooLarge { .. })
        ));
        assert!(read(tmp.path(), "../library.json").is_err());
    }
}

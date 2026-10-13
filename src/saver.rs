//! Background library writer.
//!
//! Callers on the UI thread hand over a library snapshot and return without doing disk I/O;
//! a single dedicated thread performs the atomic write (`Store::save`, which fsyncs). Work is
//! bounded: there is exactly one pending slot, so a burst of edits while a write is in
//! progress coalesces into one follow-up write of the newest snapshot ("latest value wins",
//! like a watch channel). The worker is owned by [`SaveWorker`]: dropping it (or calling
//! [`SaveWorker::shutdown`]) writes any pending snapshot and joins the thread. After shutdown,
//! `submit` is rejected rather than silently queued.
//!
//! This uses a plain OS thread + Condvar on purpose: the work is one blocking file write,
//! so an async runtime would add a dependency without removing any blocking.

use crate::model::Library;
use crate::store::Store;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Observable outcome of background saving (no library content).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct SaveStatus {
    /// Successful writes.
    pub saves: u64,
    /// Snapshots replaced by a newer one before they were written.
    pub coalesced: u64,
    pub last_error: Option<String>,
    /// Local time of the last successful write, `HH:MM:SS`.
    pub last_saved_at: Option<String>,
    /// A snapshot is queued or being written right now.
    pub pending: bool,
}

type Writer = Box<dyn Fn(&Library) -> Result<(), String> + Send>;

#[derive(Default)]
struct Slot {
    pending: Option<Library>,
    busy: bool,
    shutdown: bool,
    status: SaveStatus,
}

struct Inner {
    slot: Mutex<Slot>,
    changed: Condvar,
}

pub struct SaveWorker {
    inner: Arc<Inner>,
    handle: Option<JoinHandle<()>>,
}

impl SaveWorker {
    /// Start a worker that writes with `Store::save`. Fails if the thread cannot be spawned;
    /// callers must then refuse edits (fail closed) instead of pretending they are saved.
    pub fn new(store: Store) -> Result<Self, String> {
        Self::with_writer(Box::new(move |lib| {
            store.save(lib).map_err(|e| e.to_string())
        }))
    }

    fn with_writer(writer: Writer) -> Result<Self, String> {
        let inner = Arc::new(Inner {
            slot: Mutex::new(Slot::default()),
            changed: Condvar::new(),
        });
        let worker = inner.clone();
        let handle = std::thread::Builder::new()
            .name("typelune-saver".into())
            .spawn(move || run(worker, writer))
            .map_err(|e| format!("could not start the save thread: {e}"))?;
        Ok(Self {
            inner,
            handle: Some(handle),
        })
    }

    /// Queue a snapshot for writing without doing disk I/O. Errors after shutdown.
    pub fn submit(&self, library: Library) -> Result<(), String> {
        let mut slot = self
            .inner
            .slot
            .lock()
            .map_err(|_| "save worker state is unavailable".to_owned())?;
        if slot.shutdown {
            return Err("save worker has shut down; the change was not queued".into());
        }
        if slot.pending.replace(library).is_some() {
            slot.status.coalesced += 1;
        }
        slot.status.pending = true;
        self.inner.changed.notify_all();
        Ok(())
    }

    pub fn status(&self) -> SaveStatus {
        self.inner
            .slot
            .lock()
            .map(|s| s.status.clone())
            .unwrap_or_default()
    }

    /// Wait until nothing is pending or being written, up to `timeout`.
    /// Returns false on timeout.
    pub fn flush(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let Ok(mut slot) = self.inner.slot.lock() else {
            return false;
        };
        while slot.pending.is_some() || slot.busy {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            match self.inner.changed.wait_timeout(slot, deadline - now) {
                Ok((s, _)) => slot = s,
                Err(_) => return false,
            }
        }
        true
    }

    /// Write anything pending, stop the thread and join it. Idempotent.
    pub fn shutdown(&mut self) {
        if let Ok(mut slot) = self.inner.slot.lock() {
            slot.shutdown = true;
            self.inner.changed.notify_all();
        }
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for SaveWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn run(inner: Arc<Inner>, writer: Writer) {
    loop {
        let library = {
            let Ok(mut slot) = inner.slot.lock() else {
                return;
            };
            while slot.pending.is_none() && !slot.shutdown {
                slot = match inner.changed.wait(slot) {
                    Ok(s) => s,
                    Err(_) => return,
                };
            }
            match slot.pending.take() {
                Some(lib) => {
                    slot.busy = true;
                    lib
                }
                None => return, // shutdown with nothing pending
            }
        };
        // The lock is not held during the write, so `submit` never waits for disk I/O.
        let result = writer(&library);
        let Ok(mut slot) = inner.slot.lock() else {
            return;
        };
        slot.busy = false;
        match result {
            Ok(()) => {
                slot.status.saves += 1;
                slot.status.last_error = None;
                slot.status.last_saved_at =
                    Some(chrono::Local::now().format("%H:%M:%S").to_string());
            }
            Err(e) => slot.status.last_error = Some(e),
        }
        slot.status.pending = slot.pending.is_some();
        inner.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Group;
    use crate::store;
    use std::sync::mpsc;

    fn lib_named(name: &str) -> Library {
        let mut l = Library::demo();
        l.groups.push(Group::new(name));
        l
    }

    fn last_group(l: &Library) -> String {
        l.groups.last().unwrap().name.clone()
    }

    #[test]
    fn bounded_single_slot_coalesces_while_writer_is_busy() {
        // Deterministic: the writer blocks until released, so we control exactly what is
        // in flight. A, then B/C/D while A is being written -> only A and D are written.
        let (started_tx, started_rx) = mpsc::channel::<String>();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let release_rx = Mutex::new(release_rx);
        let w = SaveWorker::with_writer(Box::new(move |lib| {
            started_tx.send(last_group(lib)).unwrap();
            release_rx.lock().unwrap().recv().unwrap();
            Ok(())
        }))
        .unwrap();

        w.submit(lib_named("A")).unwrap();
        assert_eq!(started_rx.recv().unwrap(), "A"); // A is now being written
        for n in ["B", "C", "D"] {
            w.submit(lib_named(n)).unwrap();
        }
        let st = w.status();
        assert_eq!(st.coalesced, 2, "B and C were replaced in the single slot");
        assert!(st.pending);
        release_tx.send(()).unwrap(); // finish A
        assert_eq!(
            started_rx.recv().unwrap(),
            "D",
            "latest snapshot written next"
        );
        release_tx.send(()).unwrap(); // finish D
        assert!(w.flush(Duration::from_secs(5)));
        let st = w.status();
        assert_eq!((st.saves, st.coalesced, st.pending), (2, 2, false));
        assert!(started_rx.try_recv().is_err(), "no further writes");
    }

    #[test]
    fn writes_latest_snapshot_to_disk_and_flushes() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::new(dir.path());
        let w = SaveWorker::new(s.clone()).unwrap();
        for i in 0..50 {
            w.submit(lib_named(&format!("burst {i}"))).unwrap();
        }
        assert!(w.flush(Duration::from_secs(10)));
        let on_disk = store::read_library_file(&s.library_path()).unwrap();
        assert_eq!(last_group(&on_disk), "burst 49", "latest wins");
        let st = w.status();
        assert_eq!(
            st.saves + st.coalesced,
            50,
            "every submit is written or coalesced"
        );
        assert!(!st.pending && st.last_error.is_none());
    }

    #[test]
    fn shutdown_writes_pending_then_rejects_submits() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::new(dir.path());
        let mut w = SaveWorker::new(s.clone()).unwrap();
        w.submit(lib_named("final")).unwrap();
        w.shutdown();
        let on_disk = store::read_library_file(&s.library_path()).unwrap();
        assert_eq!(last_group(&on_disk), "final");
        let err = w.submit(lib_named("late")).unwrap_err();
        assert!(err.contains("shut down"));
        w.shutdown(); // idempotent
    }

    #[test]
    fn drop_writes_pending_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::new(dir.path());
        {
            let w = SaveWorker::new(s.clone()).unwrap();
            w.submit(lib_named("dropped")).unwrap();
        }
        let on_disk = store::read_library_file(&s.library_path()).unwrap();
        assert_eq!(last_group(&on_disk), "dropped");
    }

    #[test]
    fn invalid_snapshot_reports_error_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::new(dir.path());
        let w = SaveWorker::new(s.clone()).unwrap();
        let mut bad = Library::demo();
        bad.groups[0].name = "   ".into();
        w.submit(bad).unwrap();
        assert!(w.flush(Duration::from_secs(5)));
        assert!(w.status().last_error.unwrap().contains("name is empty"));
        assert!(!s.library_path().exists());
    }

    #[test]
    fn latency_measurement_informational() {
        // Informational only: prints this machine's numbers; asserts nothing about timing.
        let dir = tempfile::tempdir().unwrap();
        let s = Store::new(dir.path());
        let lib = Library::demo();
        let t = Instant::now();
        for _ in 0..20 {
            s.save(&lib).unwrap();
        }
        let direct = t.elapsed() / 20;
        let w = SaveWorker::new(s.clone()).unwrap();
        let t = Instant::now();
        for _ in 0..20 {
            w.submit(lib.clone()).unwrap();
        }
        let submit = t.elapsed() / 20;
        assert!(w.flush(Duration::from_secs(10)));
        println!(
            "save-latency (informational, this run): Store::save avg {direct:?}; SaveWorker::submit avg {submit:?}"
        );
    }
}

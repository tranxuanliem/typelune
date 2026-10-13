//! Exercises the C ABI exactly as the Swift shell uses it: pointers, JSON in/out, freeing.
//! No tap is started: native_start is refused off the main thread (libtest threads).

use serde_json::{json, Value};
use std::ffi::{CStr, CString};
use std::ptr;
use typelune_core::ffi::{
    tpl_abi_version, tpl_core_call, tpl_core_close, tpl_core_open, tpl_string_free,
};

struct Core(*mut typelune_core::ffi::TplCore);

impl Core {
    fn open(dir: &std::path::Path) -> Self {
        let c = CString::new(dir.to_str().unwrap()).unwrap();
        // SAFETY: valid C string.
        let p = unsafe { tpl_core_open(c.as_ptr()) };
        assert!(!p.is_null());
        Core(p)
    }

    fn raw(&self, request: &str) -> Value {
        let c = CString::new(request).unwrap();
        // SAFETY: valid handle and string; reply freed below.
        unsafe {
            let out = tpl_core_call(self.0, c.as_ptr());
            assert!(!out.is_null());
            let v: Value = serde_json::from_str(CStr::from_ptr(out).to_str().unwrap()).unwrap();
            tpl_string_free(out);
            v
        }
    }

    fn ok(&self, request: Value) -> Value {
        let v = self.raw(&request.to_string());
        assert_eq!(v["ok"], true, "request {request} failed: {v}");
        v["result"].clone()
    }

    fn err(&self, request: Value) -> String {
        let v = self.raw(&request.to_string());
        assert_eq!(v["ok"], false, "request {request} unexpectedly succeeded");
        v["error"].as_str().unwrap().to_owned()
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        // SAFETY: closed exactly once.
        unsafe { tpl_core_close(self.0) };
    }
}

#[test]
fn abi_version_and_null_safety() {
    assert_eq!(tpl_abi_version(), 2);
    // SAFETY: NULL handles are documented as allowed and reported as errors.
    unsafe {
        let out = tpl_core_call(ptr::null_mut(), ptr::null());
        let v: Value = serde_json::from_str(CStr::from_ptr(out).to_str().unwrap()).unwrap();
        assert_eq!(v["ok"], false);
        tpl_string_free(out);
        tpl_string_free(ptr::null_mut());
        tpl_core_close(ptr::null_mut());
    }
}

#[test]
fn invalid_or_empty_data_dir_fails_to_open() {
    // SAFETY: valid NUL-terminated byte strings.
    unsafe {
        let empty = CString::new("").unwrap();
        assert!(tpl_core_open(empty.as_ptr()).is_null());
        let bad = CString::new(vec![0x2f, 0x74, 0xff, 0xfe]).unwrap(); // "/t" + invalid UTF-8
        assert!(tpl_core_open(bad.as_ptr()).is_null());
    }
}

#[test]
fn unreadable_library_disables_saving_and_keeps_original() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.json");
    std::fs::write(&path, b"original bytes").unwrap();
    // Make the library unreadable: load_or_init fails (memory_only) instead of recovering.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    // A directory we cannot rename inside also prevents the corrupt-file quarantine.
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
    let readable_now = std::fs::read(&path).is_ok(); // true when running as root
    let core = Core::open(dir.path());
    let st = core.ok(json!({"cmd": "state"}));
    if !readable_now {
        assert_eq!(st["load"]["kind"], "memory_only");
        assert!(st["save"]["last_error"]
            .as_str()
            .unwrap()
            .contains("saving is disabled"));
        let lib = st["library"].clone();
        assert!(core
            .err(json!({"cmd": "update_library", "library": lib}))
            .contains("saving unavailable"));
    }
    drop(core);
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"original bytes",
        "original untouched"
    );
}

#[test]
fn open_creates_demo_and_state_round_trips_schema() {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::open(dir.path());
    let st = core.ok(json!({"cmd": "state"}));
    assert_eq!(st["load"]["kind"], "created_demo");
    assert_eq!(st["library"]["format"], "atext-rs.library");
    assert_eq!(st["abi_version"], 2);
    // The JSON handed to Swift parses back into the Rust model unchanged (stable ids).
    let lib: typelune_core::model::Library = serde_json::from_value(st["library"].clone()).unwrap();
    assert_eq!(
        lib,
        typelune_core::store::read_library_file(&dir.path().join("library.json")).unwrap()
    );
}

#[test]
fn loading_existing_library_does_not_rewrite_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.json");
    typelune_core::store::Store::new(dir.path())
        .save(&typelune_core::model::Library::demo())
        .unwrap();
    let before = std::fs::read(&path).unwrap();
    let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
    {
        let core = Core::open(dir.path());
        assert_eq!(core.ok(json!({"cmd": "state"}))["load"]["kind"], "loaded");
        core.ok(json!({"cmd": "render", "body": "x"}));
    }
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), mtime);
}

#[test]
fn update_validates_and_persists_via_worker() {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::open(dir.path());
    let mut lib = core.ok(json!({"cmd": "state"}))["library"].clone();
    lib["groups"][0]["snippets"][0]["body"] = json!("Xin chào 👋");
    let r = core.ok(json!({"cmd": "update_library", "library": lib}));
    assert!(r["lints"].is_array());
    let f = core.ok(json!({"cmd": "flush", "timeout_ms": 5000}));
    assert_eq!(f["flushed"], true);
    let disk = typelune_core::store::read_library_file(&dir.path().join("library.json")).unwrap();
    assert_eq!(disk.groups[0].snippets[0].body, "Xin chào 👋");

    // Invalid library is rejected and the previous one is kept.
    let mut bad = lib.clone();
    bad["groups"][0]["name"] = json!("  ");
    assert!(core
        .err(json!({"cmd": "update_library", "library": bad}))
        .contains("name is empty"));
    let st = core.ok(json!({"cmd": "state"}));
    assert_eq!(
        st["library"]["groups"][0]["snippets"][0]["body"],
        "Xin chào 👋"
    );
}

#[test]
fn bad_requests_are_errors_not_crashes() {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::open(dir.path());
    assert!(core.raw("{not json")["error"]
        .as_str()
        .unwrap()
        .contains("bad request"));
    assert!(core
        .err(json!({"cmd": "no_such_command"}))
        .contains("bad request"));
    assert!(core
        .err(json!({"cmd": "update_library", "library": {"format": 1}}))
        .contains("bad request"));
}

#[test]
fn render_and_playground_use_rust_engine() {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::open(dir.path());
    let r = core.ok(json!({"cmd": "render", "body": "a{{cursor}}b {{nope}}"}));
    assert_eq!(r["text"], "ab {{nope}}");
    assert_eq!(r["before_cursor"], "a");
    assert_eq!(r["after_cursor"], "b {{nope}}");
    assert_eq!(r["warnings"].as_array().unwrap().len(), 1);

    core.ok(json!({"cmd": "playground_reset"}));
    let mut last = Value::Null;
    for c in ["h", "l", "o", " "] {
        last = core.ok(json!({"cmd": "playground_edit", "backspaces": 0, "inserted": c}));
    }
    let exp = &last["expansion"];
    assert_eq!(exp["insert"], "Hello there! ");
    assert_eq!(exp["delete_text"], "hlo ");
    assert_eq!(exp["consumed_chars"], 1);

    // Decomposed Vietnamese input arriving as one committed string (IME-style).
    core.ok(json!({"cmd": "playground_reset"}));
    let r = core.ok(json!({"cmd": "playground_edit", "backspaces": 0, "inserted": "cmo\u{31B}n "}));
    assert_eq!(r["expansion"]["delete_text"], "cmo\u{31B}n ");
    assert_eq!(r["expansion"]["consumed_chars"], 6);
}

#[test]
fn import_merge_replace_and_protected_export() {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::open(dir.path());
    let export = dir.path().join("out.json");
    core.ok(json!({"cmd": "export", "path": export.to_str().unwrap()}));
    let parsed = core.ok(json!({"cmd": "parse_import", "path": export.to_str().unwrap()}));
    let n = parsed["summary"]["snippets"].as_u64().unwrap();
    let merged = core.ok(json!({"cmd": "merge_import", "library": parsed["library"]}));
    assert_eq!(merged["merged"]["snippets"].as_u64().unwrap(), n);
    let total: usize = merged["library"]["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["snippets"].as_array().unwrap().len())
        .sum();
    assert_eq!(total as u64, 2 * n);
    core.ok(json!({"cmd": "replace_library", "library": parsed["library"]}));

    let bad = dir.path().join("bad.json");
    std::fs::write(&bad, "{\"format\":\"other\"}").unwrap();
    assert!(core
        .err(json!({"cmd": "parse_import", "path": bad.to_str().unwrap()}))
        .contains("format"));
    let home = dirs::home_dir().unwrap();
    let protected = home.join("Library/Application Support/aText/x.json");
    assert!(core
        .err(json!({"cmd": "export", "path": protected.to_str().unwrap()}))
        .contains("Refusing"));
}

#[test]
fn native_is_off_and_main_thread_guarded() {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::open(dir.path());
    let st = core.ok(json!({"cmd": "native_status"}));
    assert_eq!(st["state"], "off");
    assert_eq!(st["stats"]["diag"]["key_events_seen"], 0);
    // libtest runs tests off the main thread, so these must be refused (no tap is created).
    assert!(core
        .err(json!({"cmd": "native_start"}))
        .contains("main thread"));
    assert!(core
        .err(json!({"cmd": "native_refresh_layout"}))
        .contains("main thread"));
    assert_eq!(core.ok(json!({"cmd": "native_status"}))["state"], "error");
    core.ok(json!({"cmd": "native_stop"}));
}

#[test]
fn stop_without_running_tap_is_immediate_and_keeps_core_usable() {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::open(dir.path());
    let t = std::time::Instant::now();
    let st = core.ok(json!({"cmd": "native_stop"}));
    assert!(t.elapsed() < std::time::Duration::from_millis(500));
    assert_eq!(st["state"], "off");
    // The core stays usable after stop (state lock released and re-acquired correctly).
    assert_eq!(
        core.ok(json!({"cmd": "render", "body": "ok"}))["text"],
        "ok"
    );
}

#[test]
fn preferences_live_in_the_data_folder_with_safe_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::open(dir.path());
    let p = core.ok(json!({"cmd": "preferences_get"}));
    assert_eq!(p["isolated"], true, "an explicit folder is isolated");
    assert_eq!(p["writable"], true);
    assert_eq!(p["preferences"]["start_expansion_at_launch"], false);
    assert_eq!(p["preferences"]["excluded_apps"], json!([]));
    assert!(
        !dir.path().join("preferences.json").exists(),
        "reading never writes"
    );
    let mut prefs = p["preferences"].clone();
    prefs["excluded_apps"] = json!([{"bundle_id": "com.example.Editor", "name": "Editor"}]);
    core.ok(json!({"cmd": "preferences_set", "preferences": prefs}));
    assert!(dir.path().join("preferences.json").exists());
    let mut bad = prefs.clone();
    bad["excluded_apps"] = json!([{"bundle_id": "not a bundle id"}]);
    assert!(core
        .err(json!({"cmd": "preferences_set", "preferences": bad}))
        .contains("not a valid bundle identifier"));
    drop(core);
    let again = Core::open(dir.path());
    let p = again.ok(json!({"cmd": "preferences_get"}));
    assert_eq!(
        p["preferences"]["excluded_apps"][0]["bundle_id"],
        "com.example.Editor"
    );
}

#[test]
fn backup_now_list_preview_and_restore_keep_the_current_library_first() {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::open(dir.path());
    let original = core.ok(json!({"cmd": "state"}))["library"].clone();
    let made = core.ok(json!({"cmd": "backup_now"}));
    let name = made["created"].as_str().unwrap().to_owned();
    assert_eq!(made["backups"].as_array().unwrap().len(), 1);
    assert_eq!(made["retention"], 30);

    let mut edited = original.clone();
    edited["groups"][0]["name"] = json!("Edited");
    core.ok(json!({"cmd": "update_library", "library": edited}));
    let preview = core.ok(json!({"cmd": "backup_read", "name": name}));
    assert_eq!(preview["library"], original);

    let restored = core.ok(json!({"cmd": "backup_restore", "name": name}));
    assert_eq!(restored["library"], original);
    let pre = restored["pre_restore"].as_str().unwrap().to_owned();
    assert!(pre.ends_with("-pre-restore.json"));
    // The pre-restore snapshot holds the edited library: restoring it is the way back.
    let kept = core.ok(json!({"cmd": "backup_read", "name": pre}));
    assert_eq!(kept["library"]["groups"][0]["name"], "Edited");

    // Bad names and damaged snapshots fail closed and change nothing.
    assert!(core
        .err(json!({"cmd": "backup_restore", "name": "../library.json"}))
        .contains("not a Typelune backup"));
    let damaged = dir
        .path()
        .join("backups/library-20200101-000000-manual.json");
    std::fs::write(&damaged, "{ broken").unwrap();
    assert!(core.ok(json!({"cmd": "backup_list"}))["backups"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["name"] == "library-20200101-000000-manual.json"));
    core.err(json!({"cmd": "backup_restore", "name": "library-20200101-000000-manual.json"}));
    assert_eq!(core.ok(json!({"cmd": "state"}))["library"], original);
}

#[test]
fn csv_preview_never_changes_the_library() {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::open(dir.path());
    let before = core.ok(json!({"cmd": "state"}))["library"].clone();
    let csv = dir.path().join("in.csv");
    std::fs::write(
        &csv,
        "\u{feff}abbreviation,body,tags\r\nhlo,\"Hi, there\",greeting\r\nnew,\"two\r\nlines\",\r\n,missing,\r\n",
    )
    .unwrap();
    let parsed = core.ok(json!({"cmd": "parse_csv_import", "path": csv.to_str().unwrap()}));
    assert_eq!(parsed["report"]["imported"], 2);
    assert_eq!(
        parsed["report"]["skipped"][0]["line"], 5,
        "after a two-line record"
    );
    assert!(parsed["report"]["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|w| w["message"].as_str().unwrap().contains("already used")));
    assert_eq!(parsed["summary"]["snippets"], 2);
    assert_eq!(core.ok(json!({"cmd": "state"}))["library"], before);
    // Confirming is the existing merge path.
    let merged = core.ok(json!({"cmd": "merge_import", "library": parsed["library"]}));
    assert_eq!(merged["merged"]["snippets"], 2);
}

#[test]
fn quick_insert_is_refused_when_expansion_is_off() {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::open(dir.path());
    let lib = core.ok(json!({"cmd": "state"}))["library"].clone();
    let id = lib["groups"][0]["snippets"][0]["id"].clone();
    let err = core.err(json!({"cmd": "native_quick_insert", "token": 1, "snippet_id": id}));
    assert!(err.contains("off"), "{err}");
    core.ok(json!({"cmd": "native_release_target"}));
    core.ok(json!({"cmd": "native_set_frontmost", "pid": 1}));
    let st = core.ok(json!({"cmd": "native_status"}));
    assert_eq!(st["state"], "off");
    assert_eq!(st["stats"]["diag"]["quick_inserts_queued"], 0);
    // Opening Quick Search suspends automatic matching without changing the run state.
    let st = core.ok(json!({"cmd": "native_inhibit", "on": true}));
    assert_eq!(
        (st["inhibited"].clone(), st["state"].clone()),
        (json!(true), json!("off"))
    );
    let st = core.ok(json!({"cmd": "native_inhibit", "on": false}));
    assert_eq!(st["inhibited"], false);
}

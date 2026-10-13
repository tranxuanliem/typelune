//! End-to-end checks over the shipped demo library: persistence -> engine -> rendering,
//! applied to a simulated text field the same way the Playground applies replacements.

use chrono::{Local, TimeZone};
use typelune_core::engine::{Engine, KeyInput};
use typelune_core::model::Library;
use typelune_core::store::{self, LoadOutcome, Store};
use typelune_core::template::RenderContext;

fn ctx() -> RenderContext {
    RenderContext::new(
        Local.with_ymd_and_hms(2026, 10, 13, 14, 30, 0).unwrap(),
        Some("pasted text".into()),
    )
}

/// Type `keys` into a fresh text field; returns (text, caret char index).
fn type_into(engine: &mut Engine, keys: &str) -> (String, usize) {
    let mut text: Vec<char> = Vec::new();
    let mut caret = 0usize;
    for c in keys.chars() {
        if c == '\u{8}' {
            if caret > 0 {
                caret -= 1;
                text.remove(caret);
            }
            engine.feed(KeyInput::Backspace);
            continue;
        }
        text.insert(caret, c);
        caret += 1;
        if let Some(exp) = engine.feed(KeyInput::Char(c)) {
            let r = exp.render(&ctx());
            let start = caret - r.delete_chars;
            let typed: String = text[start..caret].iter().collect();
            assert_eq!(
                typed, r.delete_text,
                "replacement must delete exactly what was typed"
            );
            let ins: Vec<char> = r.insert.chars().collect();
            text.splice(start..caret, ins.iter().copied());
            caret = start + ins.len() - r.after_cursor.chars().count();
        }
    }
    (text.into_iter().collect(), caret)
}

fn demo_engine() -> Engine {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path());
    let lib = match store.load_or_init().unwrap() {
        LoadOutcome::CreatedDemo(l) => l,
        other => panic!("expected demo library, got {other:?}"),
    };
    // Reload from disk to make sure the persisted form drives the engine.
    let reloaded = store::read_library_file(&store.library_path()).unwrap();
    assert_eq!(lib, reloaded);
    Engine::new(&reloaded)
}

#[test]
fn demo_snippets_expand_like_the_playground_hints() {
    let cases: &[(&str, &str)] = &[
        ("hlo ", "Hello there! "),
        ("sig\n", "Best regards,\nAlex Example\nexample.com\n"),
        ("on ;d", "on 2026-10-13"),
        (";t", "14:30"),
        (";ld", "Tuesday, 13 October 2026"),
        (";q", "“pasted text”"),
        ("xc ", "Xin chào, rất vui được gặp bạn! "),
        ("cmơn.", "Cảm ơn bạn nhiều nhé.."),
        ("API ", "Application Programming Interface "),
        ("api ", "api "),
        ("a ->> b", "a → b"),
        ("shrug ", "¯\\_(ツ)_/¯ "),
    ];
    for (keys, want) in cases {
        let mut e = demo_engine();
        let (text, _) = type_into(&mut e, keys);
        assert_eq!(&text, want, "typing {keys:?}");
    }
}

#[test]
fn cursor_placeholder_places_caret() {
    let mut e = demo_engine();
    let (text, caret) = type_into(&mut e, "mtg ");
    assert_eq!(
        text,
        "Meeting notes 2026-10-13\n\nAttendees: \nAction items:\n-  " // body + kept delimiter
    );
    let before: String = text.chars().take(caret).collect();
    assert!(before.ends_with("Attendees: "), "caret after 'Attendees: '");
}

#[test]
fn disabled_group_in_demo_does_not_expand() {
    let mut e = demo_engine();
    assert_eq!(type_into(&mut e, "hlo2 ").0, "hlo2 ");
}

#[test]
fn vietnamese_decomposed_typing_and_backspace() {
    let mut e = demo_engine();
    // "cmơn" with ơ typed as o + combining horn, after a typo corrected by backspace.
    let (text, _) = type_into(&mut e, "cmx\u{8}o\u{31B}n ");
    assert_eq!(text, "Cảm ơn bạn nhiều nhé. ");
}

#[test]
fn export_then_import_merge_keeps_engine_behaviour() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("export.json");
    store::export_to(&path, &Library::demo()).unwrap();
    let imported = store::import_from(&path).unwrap();
    let mut lib = Library::empty();
    store::merge_into(&mut lib, imported);
    let mut e = Engine::new(&lib);
    assert_eq!(type_into(&mut e, "hlo ").0, "Hello there! ");
}

#[test]
fn malformed_import_is_rejected_without_touching_library() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path());
    store.load_or_init().unwrap();
    let before = std::fs::read_to_string(store.library_path()).unwrap();
    let bad = dir.path().join("bad.json");
    std::fs::write(
        &bad,
        r#"{"format":"atext-rs.library","version":1,"groups":[{"name":5}]}"#,
    )
    .unwrap();
    assert!(store::import_from(&bad).is_err());
    assert_eq!(
        std::fs::read_to_string(store.library_path()).unwrap(),
        before
    );
}

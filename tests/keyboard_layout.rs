//! Keyboard-layout fallback check. Runs on the process main thread (harness = false) because
//! the TextInputSources API must be called there. Reads the current layout only; it does
//! not post events, create taps, or need any permission.

#[cfg(target_os = "macos")]
fn main() {
    use typelune_core::native::macos::{current_layout_snapshot, translate_keycode};
    let Some(layout) = current_layout_snapshot() else {
        println!("keyboard_layout: SKIPPED (current layout has no uchr data)");
        return;
    };
    let space = translate_keycode(&layout, 49, 0);
    assert_eq!(
        space, " ",
        "space key must translate to a space on every layout"
    );
    let ret = translate_keycode(&layout, 36, 0);
    assert_eq!(ret, "\r", "return key translates to CR");
    let letter = translate_keycode(&layout, 17, 0);
    assert_eq!(
        letter.chars().count(),
        1,
        "a letter key yields one char: {letter:?}"
    );
    assert!(!letter.chars().any(char::is_control));
    let shifted = translate_keycode(&layout, 17, 0x0002_0000);
    assert_eq!(shifted.chars().count(), 1);
    println!("keyboard_layout: 3 checks passed (space, return, letter key incl. shift)");
}

#[cfg(not(target_os = "macos"))]
fn main() {
    println!("keyboard_layout: SKIPPED (not macOS)");
}

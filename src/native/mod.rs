//! Opt-in system-wide expansion.
//!
//! Platform-independent pieces (key classification, injection planning) live here so they
//! can be unit-tested without touching the OS. The macOS event tap is in [`macos`].

use crate::engine::{KeyInput, Replacement};
use unicode_segmentation::UnicodeSegmentation;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "macos")]
pub use macos::{open_accessibility_settings, NativeController, PendingStop, StopOutcome};

#[cfg(not(target_os = "macos"))]
pub mod unsupported;
#[cfg(not(target_os = "macos"))]
pub use unsupported::{open_accessibility_settings, NativeController, PendingStop, StopOutcome};

/// Marker written into `kCGEventSourceUserData` of every event we post, so our own tap
/// can recognise and skip synthetic input.
pub const SYNTHETIC_MARKER: i64 = 0x6174_6578_7273; // fixed value; never change (QA history)

/// Max UTF-16 units per synthetic keyboard event (CoreGraphics truncates longer strings).
pub const MAX_UTF16_PER_EVENT: usize = 20;

// macOS virtual key codes (layout-independent positions).
pub mod keycode {
    pub const RETURN: u16 = 36;
    pub const TAB: u16 = 48;
    pub const DELETE: u16 = 51; // Backspace
    pub const ESCAPE: u16 = 53;
    pub const KEYPAD_ENTER: u16 = 76;
    pub const HOME: u16 = 115;
    pub const PAGE_UP: u16 = 116;
    pub const FORWARD_DELETE: u16 = 117;
    pub const END: u16 = 119;
    pub const PAGE_DOWN: u16 = 121;
    pub const LEFT: u16 = 123;
    pub const RIGHT: u16 = 124;
    pub const DOWN: u16 = 125;
    pub const UP: u16 = 126;
}

// CGEventFlags masks.
pub const FLAG_CONTROL: u64 = 0x0004_0000;
pub const FLAG_COMMAND: u64 = 0x0010_0000;

/// Translate one key-down event into engine inputs. Pure function for testability.
///
/// * Command/Control chords reset the buffer (shortcuts, not text).
/// * Navigation keys, Escape and Forward-Delete reset (caret moved / context unknown).
/// * Backspace edits the buffer.
/// * Return/Enter/Tab become `\n`/`\t`.
/// * Keys producing no text (dead keys, function keys) reset, because the resulting
///   character cannot be observed reliably at this level.
pub fn classify_key(keycode: u16, flags: u64, chars: &str) -> Vec<KeyInput> {
    use keycode::*;
    if flags & (FLAG_COMMAND | FLAG_CONTROL) != 0 {
        return vec![KeyInput::Reset];
    }
    match keycode {
        DELETE => return vec![KeyInput::Backspace],
        RETURN | KEYPAD_ENTER => return vec![KeyInput::Char('\n')],
        TAB => return vec![KeyInput::Char('\t')],
        ESCAPE | HOME | END | PAGE_UP | PAGE_DOWN | FORWARD_DELETE | LEFT | RIGHT | UP | DOWN => {
            return vec![KeyInput::Reset]
        }
        _ => {}
    }
    if chars.is_empty() || chars.chars().any(|c| c.is_control()) {
        return vec![KeyInput::Reset];
    }
    chars.chars().map(KeyInput::Char).collect()
}

/// One synthetic input step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InjectStep {
    Backspaces(usize),
    /// Text without newlines, at most [`MAX_UTF16_PER_EVENT`] UTF-16 units
    /// (unless a single grapheme cluster is longer).
    Text(String),
    Return,
    Left(usize),
}

/// Turn a rendered replacement into synthetic key steps.
///
/// Backspace and Left-arrow counts use extended grapheme clusters, matching how macOS
/// text views move/delete over user-perceived characters.
pub fn plan_injection(rep: &Replacement) -> Vec<InjectStep> {
    let mut steps = Vec::new();
    let del = rep.delete_text.graphemes(true).count();
    if del > 0 {
        steps.push(InjectStep::Backspaces(del));
    }
    for (i, line) in rep.insert.split('\n').enumerate() {
        if i > 0 {
            steps.push(InjectStep::Return);
        }
        let mut chunk = String::new();
        let mut units = 0;
        for g in line.graphemes(true) {
            let n = g.encode_utf16().count();
            if units + n > MAX_UTF16_PER_EVENT && !chunk.is_empty() {
                steps.push(InjectStep::Text(std::mem::take(&mut chunk)));
                units = 0;
            }
            chunk.push_str(g);
            units += n;
        }
        if !chunk.is_empty() {
            steps.push(InjectStep::Text(chunk));
        }
    }
    let left = rep.after_cursor.graphemes(true).count();
    if left > 0 {
        steps.push(InjectStep::Left(left));
    }
    steps
}

/// Why a queued expansion was not (fully) injected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbortReason {
    Stopped,
    Paused,
    /// Typing, click, navigation key or shortcut happened after the match.
    InputChanged,
    SecureInput,
    /// Focused app/element is different from the one the abbreviation was typed into.
    FocusChanged,
    /// The focused app could not be identified via Accessibility; we fail closed.
    FocusUnknown,
    /// The key event's target process differs from the focused app.
    TargetMismatch,
    /// The focused app is Typelune itself; the in-app editors handle their own text.
    OwnApp,
}

impl AbortReason {
    pub fn describe(self) -> &'static str {
        match self {
            AbortReason::Stopped => "stopped",
            AbortReason::Paused => "paused",
            AbortReason::InputChanged => "other input arrived (typing, click, arrow or shortcut)",
            AbortReason::SecureInput => "secure keyboard entry became active",
            AbortReason::FocusChanged => "focus moved to another app or field",
            AbortReason::FocusUnknown => "focused app could not be identified (fail closed)",
            AbortReason::TargetMismatch => "key target app differs from focused app",
            AbortReason::OwnApp => "focused app is Typelune itself (never injected)",
        }
    }
}

/// Conditions re-checked immediately before injection and between injection steps.
#[derive(Debug, Clone, Copy)]
pub struct GateSnapshot {
    pub shutdown: bool,
    pub paused: bool,
    pub job_generation: u64,
    pub current_generation: u64,
    pub secure_input: bool,
    /// `None` = focus could not be determined.
    pub focus_matches: Option<bool>,
}

/// Pure gate evaluation; the first failing condition wins.
pub fn evaluate_gates(g: &GateSnapshot) -> Result<(), AbortReason> {
    if g.shutdown {
        return Err(AbortReason::Stopped);
    }
    if g.paused {
        return Err(AbortReason::Paused);
    }
    if g.job_generation != g.current_generation {
        return Err(AbortReason::InputChanged);
    }
    if g.secure_input {
        return Err(AbortReason::SecureInput);
    }
    match g.focus_matches {
        Some(true) => Ok(()),
        Some(false) => Err(AbortReason::FocusChanged),
        None => Err(AbortReason::FocusUnknown),
    }
}

/// Snapshot of permissions as reported by the OS (read-only checks, no prompts).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub struct Permissions {
    pub supported: bool,
    pub accessibility: bool,
    pub listen_events: bool,
    pub post_events: bool,
    pub secure_input: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeState {
    Off,
    Running,
    Paused,
    /// Tap exists but the system disabled it (timeout/user input); we try to re-enable.
    Disabled,
    /// Stop requested; threads are being joined.
    Stopping,
    Error(String),
}

#[derive(Debug, Clone, Default)]
pub struct NativeStats {
    pub expansions: u64,
    pub last_snippet: Option<String>,
    pub last_at: Option<chrono::DateTime<chrono::Local>>,
    pub secure_input_skips: u64,
    pub tap_reenabled: u64,
    pub last_warning: Option<String>,
    /// Expansions cancelled before or during injection (focus change, typing, stop, ...).
    pub aborted: u64,
    pub last_abort: Option<String>,
    /// Session-only diagnostic counters (no typed content). Reset on every Start.
    pub diag: NativeDiagnostics,
}

/// Counts of what the event tap saw, by category only. Never contains typed text,
/// key codes or snippet content; never written to disk or sent anywhere.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct NativeDiagnostics {
    /// Key-down callbacks received by the keyboard tap (before any filtering).
    pub key_events_seen: u64,
    /// Key-downs that carried usable text and were fed to the engine.
    pub text_inputs: u64,
    /// Key-downs whose Unicode string was empty (dead keys, keycode-only synthetic events).
    pub empty_text_events: u64,
    /// Key-downs that reset matching: shortcuts, navigation, control characters.
    pub reset_inputs: u64,
    pub backspaces: u64,
    /// Mouse-down events that reset matching.
    pub mouse_resets: u64,
    /// Our own synthetic events recognised and skipped.
    pub own_event_skips: u64,
    pub paused_skips: u64,
    /// Real key presses that arrived while an expansion was pending (cancelled it).
    pub typed_during_injection: u64,
    /// Key events on which macOS supplied a target PID.
    pub target_pid_present: u64,
    /// Empty-Unicode key events recovered via the keyboard-layout fallback.
    pub layout_translated: u64,
    /// Key events targeting this app's own windows (not matched globally).
    pub own_app_skips: u64,
    /// Abbreviation matches handed to the injector.
    pub matches_queued: u64,
    /// Mouse clicks that arrived while an expansion was pending (cancelled it).
    pub mouse_during_injection: u64,
    /// Non-content description of the last event that cancelled an injection
    /// (source PID / self, marker match, source state, category, ms after match).
    pub last_cancel_event: Option<String>,
}

/// Diagnostic category of a key-down; mirrors the branches of [`classify_key`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyCategory {
    Text,
    EmptyText,
    Reset,
    Backspace,
}

pub fn key_category(keycode: u16, flags: u64, chars: &str) -> KeyCategory {
    let inputs = classify_key(keycode, flags, chars);
    match inputs.as_slice() {
        [KeyInput::Backspace] => KeyCategory::Backspace,
        [KeyInput::Reset] => {
            let special = flags & (FLAG_COMMAND | FLAG_CONTROL) != 0
                || matches!(
                    keycode,
                    keycode::ESCAPE
                        | keycode::HOME
                        | keycode::END
                        | keycode::PAGE_UP
                        | keycode::PAGE_DOWN
                        | keycode::FORWARD_DELETE
                        | keycode::LEFT
                        | keycode::RIGHT
                        | keycode::UP
                        | keycode::DOWN
                );
            if chars.is_empty() && !special {
                KeyCategory::EmptyText
            } else {
                KeyCategory::Reset
            }
        }
        _ => KeyCategory::Text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rep(delete: &str, insert: &str, after: &str) -> Replacement {
        Replacement {
            delete_chars: delete.chars().count(),
            delete_text: delete.into(),
            insert: insert.into(),
            after_cursor: after.into(),
            warnings: vec![],
        }
    }

    fn gates() -> GateSnapshot {
        GateSnapshot {
            shutdown: false,
            paused: false,
            job_generation: 7,
            current_generation: 7,
            secure_input: false,
            focus_matches: Some(true),
        }
    }

    #[test]
    fn gates_pass_when_everything_matches() {
        assert_eq!(evaluate_gates(&gates()), Ok(()));
    }

    #[test]
    fn gates_fail_closed() {
        let cases = [
            (
                GateSnapshot {
                    shutdown: true,
                    ..gates()
                },
                AbortReason::Stopped,
            ),
            (
                GateSnapshot {
                    paused: true,
                    ..gates()
                },
                AbortReason::Paused,
            ),
            (
                GateSnapshot {
                    current_generation: 8,
                    ..gates()
                },
                AbortReason::InputChanged,
            ),
            (
                GateSnapshot {
                    secure_input: true,
                    ..gates()
                },
                AbortReason::SecureInput,
            ),
            (
                GateSnapshot {
                    focus_matches: Some(false),
                    ..gates()
                },
                AbortReason::FocusChanged,
            ),
            (
                GateSnapshot {
                    focus_matches: None,
                    ..gates()
                },
                AbortReason::FocusUnknown,
            ),
        ];
        for (g, want) in cases {
            assert_eq!(evaluate_gates(&g), Err(want));
        }
        // Stop takes precedence over everything else.
        let all_bad = GateSnapshot {
            shutdown: true,
            paused: true,
            current_generation: 9,
            secure_input: true,
            focus_matches: None,
            ..gates()
        };
        assert_eq!(evaluate_gates(&all_bad), Err(AbortReason::Stopped));
    }

    #[test]
    fn key_categories_for_diagnostics() {
        assert_eq!(key_category(17, 0, "t"), KeyCategory::Text);
        assert_eq!(key_category(49, 0, " "), KeyCategory::Text);
        assert_eq!(key_category(0, 0, "test"), KeyCategory::Text);
        assert_eq!(key_category(17, 0, ""), KeyCategory::EmptyText);
        assert_eq!(key_category(keycode::LEFT, 0, ""), KeyCategory::Reset);
        assert_eq!(key_category(17, FLAG_COMMAND, "t"), KeyCategory::Reset);
        assert_eq!(
            key_category(keycode::ESCAPE, 0, "\u{1b}"),
            KeyCategory::Reset
        );
        assert_eq!(
            key_category(keycode::DELETE, 0, "\u{8}"),
            KeyCategory::Backspace
        );
        assert_eq!(key_category(keycode::RETURN, 0, "\r"), KeyCategory::Text);
    }

    #[test]
    fn chords_and_navigation_reset() {
        assert_eq!(classify_key(0, FLAG_COMMAND, "a"), vec![KeyInput::Reset]);
        assert_eq!(
            classify_key(0, FLAG_CONTROL, "\u{1}"),
            vec![KeyInput::Reset]
        );
        assert_eq!(classify_key(keycode::LEFT, 0, ""), vec![KeyInput::Reset]);
        assert_eq!(
            classify_key(keycode::ESCAPE, 0, "\u{1b}"),
            vec![KeyInput::Reset]
        );
        assert_eq!(
            classify_key(keycode::FORWARD_DELETE, 0, ""),
            vec![KeyInput::Reset]
        );
    }

    #[test]
    fn editing_keys() {
        assert_eq!(
            classify_key(keycode::DELETE, 0, "\u{8}"),
            vec![KeyInput::Backspace]
        );
        assert_eq!(
            classify_key(keycode::RETURN, 0, "\r"),
            vec![KeyInput::Char('\n')]
        );
        assert_eq!(
            classify_key(keycode::TAB, 0, "\t"),
            vec![KeyInput::Char('\t')]
        );
    }

    #[test]
    fn text_keys_including_option_and_unicode() {
        // Option (0x80000) is allowed: it produces characters on many layouts.
        assert_eq!(classify_key(0, 0x80000, "å"), vec![KeyInput::Char('å')]);
        assert_eq!(classify_key(0, 0, "ư"), vec![KeyInput::Char('ư')]);
        assert_eq!(classify_key(0, 0, ""), vec![KeyInput::Reset]);
    }

    #[test]
    fn plan_simple() {
        let steps = plan_injection(&rep("hlo ", "Hello ", ""));
        assert_eq!(
            steps,
            vec![InjectStep::Backspaces(4), InjectStep::Text("Hello ".into())]
        );
    }

    #[test]
    fn plan_multiline_and_cursor() {
        let steps = plan_injection(&rep("sig\n", "Best,\nAlex\n", "lex\n"));
        assert_eq!(
            steps,
            vec![
                InjectStep::Backspaces(4),
                InjectStep::Text("Best,".into()),
                InjectStep::Return,
                InjectStep::Text("Alex".into()),
                InjectStep::Return,
                InjectStep::Left(4),
            ]
        );
    }

    #[test]
    fn plan_chunks_by_utf16_without_splitting_graphemes() {
        let insert = "👍🏽".repeat(6); // each is 4 UTF-16 units, 1 grapheme
        let steps = plan_injection(&rep("", &insert, ""));
        let texts: Vec<&String> = steps
            .iter()
            .filter_map(|s| match s {
                InjectStep::Text(t) => Some(t),
                _ => None,
            })
            .collect();
        assert_eq!(texts.len(), 2);
        assert!(texts
            .iter()
            .all(|t| t.encode_utf16().count() <= MAX_UTF16_PER_EVENT));
        assert_eq!(texts.iter().map(|t| t.as_str()).collect::<String>(), insert);
    }

    #[test]
    fn plan_counts_graphemes_for_delete_and_left() {
        // "e" + combining acute is one grapheme; Vietnamese precomposed chars are one each.
        let steps = plan_injection(&rep("cafe\u{301} ", "Cà phê ", "phê "));
        assert_eq!(steps[0], InjectStep::Backspaces(5));
        assert_eq!(*steps.last().unwrap(), InjectStep::Left(4));
    }
}

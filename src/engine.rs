//! Plain-text expansion engine.
//!
//! The engine is fed one key event at a time (`Char`, `Backspace`, `Reset`) and keeps a
//! small, bounded, in-memory buffer of recently typed characters. It never stores or
//! transmits that buffer anywhere else.
//!
//! POC rules (deliberate choices, not a claim of parity with any other product):
//! * **Trigger** – each snippet fires *immediately* when its last character is typed, or
//!   *after a delimiter* (space, punctuation, newline, ...). Snippets inherit the library
//!   default unless they override it.
//! * **Word boundary** – when enabled, an abbreviation whose first character is a letter or
//!   digit only matches if the character before it is not a letter/digit (or the buffer
//!   started fresh). Abbreviations that start with punctuation (e.g. `;d`) skip this check.
//! * **Longest match** – if several abbreviations match at the same point, the longest
//!   (in characters) wins; ties go to case-sensitive rules, then to library order.
//! * **Case** – case-insensitive rules compare Unicode lower-case forms; the body is inserted
//!   unchanged (no case adaptation in v0.1).
//! * **Disabled** – disabled snippets and every snippet in a disabled group never match.
//! * **Unicode** – abbreviations and typed text are compared in NFC; a typed combining mark
//!   is composed with the previous character where possible.
//! * After an expansion the buffer is cleared, so expansions never chain.

use crate::model::{Library, TriggerMode};
use crate::template::{render, RenderContext};
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

/// Longest abbreviation accepted by the engine (characters).
pub const MAX_ABBREVIATION_CHARS: usize = 48;
/// Maximum number of recent characters kept in memory.
pub const MAX_BUFFER_CHARS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyInput {
    Char(char),
    Backspace,
    /// Anything that makes the typing context unknown: mouse click, arrow keys,
    /// shortcut chords, focus change, escape, ...
    Reset,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineSettings {
    pub default_trigger: TriggerMode,
    pub delimiters: Vec<char>,
    pub keep_delimiter: bool,
    pub require_word_boundary: bool,
}

impl Default for EngineSettings {
    fn default() -> Self {
        Self::from(&crate::model::Settings::default())
    }
}

impl From<&crate::model::Settings> for EngineSettings {
    fn from(s: &crate::model::Settings) -> Self {
        let mut delimiters: Vec<char> = s.delimiters.chars().collect();
        // Return keys arrive as '\r' from some sources; treat it like '\n'.
        if delimiters.contains(&'\n') && !delimiters.contains(&'\r') {
            delimiters.push('\r');
        }
        Self {
            default_trigger: s.default_trigger,
            delimiters,
            keep_delimiter: s.keep_delimiter,
            require_word_boundary: s.require_word_boundary,
        }
    }
}

#[derive(Debug, Clone)]
struct Rule {
    snippet_id: Uuid,
    title: String,
    /// NFC abbreviation chars.
    abbr: Vec<char>,
    case_sensitive: bool,
    mode: TriggerMode,
    body: String,
    order: usize,
}

/// A match produced by the engine; the caller performs the actual text replacement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expansion {
    pub snippet_id: Uuid,
    pub title: String,
    pub abbreviation: String,
    /// Exactly what the user typed that must be removed (abbreviation + delimiter, if any),
    /// as raw chars: decomposed input stays decomposed.
    pub typed: String,
    /// Raw snippet body (placeholders not yet rendered).
    pub body: String,
    /// Text to re-insert after the body (the triggering delimiter when `keep_delimiter`).
    pub suffix: String,
}

/// A fully rendered replacement, ready to apply to a text field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replacement {
    /// Number of chars to delete before the caret.
    pub delete_chars: usize,
    /// The text being deleted (lets native code count grapheme clusters).
    pub delete_text: String,
    pub insert: String,
    /// Text between the desired caret position and the end of `insert`.
    pub after_cursor: String,
    pub warnings: Vec<String>,
}

impl Expansion {
    pub fn render(&self, ctx: &RenderContext) -> Replacement {
        let rendered = render(&self.body, ctx);
        let mut insert = rendered.text.clone();
        insert.push_str(&self.suffix);
        let after_cursor = if rendered.cursor.is_some() {
            let mut s = rendered.text_after_cursor().to_owned();
            s.push_str(&self.suffix);
            s
        } else {
            String::new()
        };
        Replacement {
            delete_chars: self.typed.chars().count(),
            delete_text: self.typed.clone(),
            insert,
            after_cursor,
            warnings: rendered.warnings,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Engine {
    rules: Vec<Rule>,
    settings: EngineSettings,
    /// NFC characters used for matching.
    buffer: Vec<char>,
    /// Exactly what was typed for each entry of `buffer` (may hold a base char plus
    /// combining marks). Used so replacements delete the raw input, not its NFC form.
    raw: Vec<String>,
    /// True when chars were dropped from the front, so the buffer start is not a boundary.
    truncated: bool,
}

impl Engine {
    pub fn new(library: &Library) -> Self {
        let mut e = Self::default();
        e.load(library);
        e
    }

    /// Rebuild the rule set from the library. Keeps the typing buffer.
    pub fn load(&mut self, library: &Library) {
        self.settings = EngineSettings::from(&library.settings);
        self.rules.clear();
        let mut order = 0;
        for group in &library.groups {
            for snip in &group.snippets {
                order += 1;
                if !group.enabled || !snip.enabled {
                    continue;
                }
                let abbr: Vec<char> = snip.abbreviation.nfc().collect();
                if abbr.is_empty() || abbr.len() > MAX_ABBREVIATION_CHARS {
                    continue;
                }
                self.rules.push(Rule {
                    snippet_id: snip.id,
                    title: snip.display_name(),
                    abbr,
                    case_sensitive: snip.case_sensitive,
                    mode: snip.trigger.resolve(self.settings.default_trigger),
                    body: snip.body.clone(),
                    order,
                });
            }
        }
    }

    pub fn settings(&self) -> &EngineSettings {
        &self.settings
    }

    pub fn active_rule_count(&self) -> usize {
        self.rules.len()
    }

    /// Current buffer contents (for diagnostics in the playground only).
    pub fn buffer(&self) -> String {
        self.buffer.iter().collect()
    }

    pub fn reset(&mut self) {
        self.buffer.clear();
        self.raw.clear();
        self.truncated = false;
    }

    pub fn feed(&mut self, input: KeyInput) -> Option<Expansion> {
        match input {
            KeyInput::Reset => {
                self.reset();
                None
            }
            KeyInput::Backspace => {
                self.backspace();
                None
            }
            KeyInput::Char(c) => self.feed_char(c),
        }
    }

    /// Convenience for tests and the preview: feed every char of `text`, return the last expansion.
    pub fn feed_str(&mut self, text: &str) -> Vec<Expansion> {
        text.chars()
            .filter_map(|c| self.feed(KeyInput::Char(c)))
            .collect()
    }

    fn feed_char(&mut self, c: char) -> Option<Expansion> {
        let c = if c == '\r' { '\n' } else { c };
        if c.is_control() && c != '\n' && c != '\t' {
            self.reset();
            return None;
        }

        if self.is_delimiter(c) {
            if let Some(rule) = self.best_match(TriggerMode::Delimiter) {
                let exp = self.make_expansion(rule, Some(c));
                self.reset();
                return Some(exp);
            }
        }

        self.push(c);

        if let Some(rule) = self.best_match(TriggerMode::Immediate) {
            let exp = self.make_expansion(rule, None);
            self.reset();
            return Some(exp);
        }
        None
    }

    fn is_delimiter(&self, c: char) -> bool {
        self.settings.delimiters.contains(&c)
    }

    /// Remove one typed char. If the last entry was composed from several raw chars
    /// (e.g. `o` + combining horn), only the last raw char is removed and the entry is
    /// recomposed, mirroring a text field deleting one char.
    fn backspace(&mut self) {
        let Some(raw) = self.raw.last_mut() else {
            return;
        };
        raw.pop();
        if raw.is_empty() {
            self.raw.pop();
            self.buffer.pop();
            return;
        }
        let mut nfc = raw.nfc();
        match (nfc.next(), nfc.next()) {
            (Some(c), None) => *self.buffer.last_mut().unwrap() = c,
            _ => {
                // Cannot be represented as a single char any more; drop the entry.
                self.raw.pop();
                self.buffer.pop();
            }
        }
    }

    fn push(&mut self, c: char) {
        if unicode_normalization::char::is_combining_mark(c) {
            if let Some(prev) = self.buffer.last().copied() {
                if let Some(composed) = unicode_normalization::char::compose(prev, c) {
                    *self.buffer.last_mut().unwrap() = composed;
                    self.raw.last_mut().unwrap().push(c);
                    return;
                }
            }
        }
        self.buffer.push(c);
        self.raw.push(c.to_string());
        if self.buffer.len() > MAX_BUFFER_CHARS {
            let excess = self.buffer.len() - MAX_BUFFER_CHARS;
            self.buffer.drain(..excess);
            self.raw.drain(..excess);
            self.truncated = true;
        }
    }

    fn best_match(&self, mode: TriggerMode) -> Option<usize> {
        let mut best: Option<usize> = None;
        for (i, rule) in self.rules.iter().enumerate() {
            if rule.mode != mode || !self.rule_matches(rule) {
                continue;
            }
            best = match best {
                None => Some(i),
                Some(b) => {
                    let cur = &self.rules[b];
                    let better = rule.abbr.len() > cur.abbr.len()
                        || (rule.abbr.len() == cur.abbr.len()
                            && rule.case_sensitive
                            && !cur.case_sensitive)
                        || (rule.abbr.len() == cur.abbr.len()
                            && rule.case_sensitive == cur.case_sensitive
                            && rule.order < cur.order);
                    if better {
                        Some(i)
                    } else {
                        Some(b)
                    }
                }
            };
        }
        best
    }

    fn rule_matches(&self, rule: &Rule) -> bool {
        let n = rule.abbr.len();
        if self.buffer.len() < n {
            return false;
        }
        let start = self.buffer.len() - n;
        let tail = &self.buffer[start..];
        let equal = if rule.case_sensitive {
            tail == rule.abbr.as_slice()
        } else {
            tail.iter()
                .zip(rule.abbr.iter())
                .all(|(a, b)| a == b || a.to_lowercase().eq(b.to_lowercase()))
        };
        if !equal {
            return false;
        }
        if self.settings.require_word_boundary && rule.abbr[0].is_alphanumeric() {
            if start == 0 {
                return !self.truncated;
            }
            let prev = self.buffer[start - 1];
            return !prev.is_alphanumeric();
        }
        true
    }

    fn make_expansion(&self, rule_idx: usize, delimiter: Option<char>) -> Expansion {
        let rule = &self.rules[rule_idx];
        let n = rule.abbr.len();
        // Raw input, so callers delete exactly what was typed (decomposed or not).
        let mut typed: String = self.raw[self.raw.len() - n..].concat();
        let mut suffix = String::new();
        if let Some(d) = delimiter {
            typed.push(d);
            if self.settings.keep_delimiter {
                suffix.push(d);
            }
        }
        Expansion {
            snippet_id: rule.snippet_id,
            title: rule.title.clone(),
            abbreviation: rule.abbr.iter().collect(),
            typed,
            body: rule.body.clone(),
            suffix,
        }
    }
}

/// Problems worth surfacing in the editor. None of them block saving.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lint {
    pub snippet_id: Uuid,
    pub message: String,
}

pub fn lint(library: &Library) -> Vec<Lint> {
    let settings = EngineSettings::from(&library.settings);
    let mut out = Vec::new();
    struct Active {
        id: Uuid,
        key: String,
        abbr: String,
        mode: TriggerMode,
    }
    let mut active: Vec<Active> = Vec::new();
    for group in &library.groups {
        for s in &group.snippets {
            let abbr: String = s.abbreviation.nfc().collect();
            let mode = s.trigger.resolve(settings.default_trigger);
            if abbr.is_empty() {
                out.push(Lint {
                    snippet_id: s.id,
                    message: "No abbreviation: this snippet can never expand.".into(),
                });
                continue;
            }
            if abbr.chars().count() > MAX_ABBREVIATION_CHARS {
                out.push(Lint {
                    snippet_id: s.id,
                    message: format!(
                        "Abbreviation longer than {MAX_ABBREVIATION_CHARS} characters is ignored."
                    ),
                });
            }
            if abbr.chars().any(|c| c.is_whitespace()) {
                out.push(Lint {
                    snippet_id: s.id,
                    message: "Abbreviation contains whitespace; typing it may trigger other rules or reset matching.".into(),
                });
            }
            if mode == TriggerMode::Delimiter
                && abbr
                    .chars()
                    .last()
                    .is_some_and(|c| settings.delimiters.contains(&c))
            {
                out.push(Lint {
                    snippet_id: s.id,
                    message: "Abbreviation ends with a delimiter character; use the immediate trigger instead.".into(),
                });
            }
            if group.enabled && s.enabled {
                let key = if s.case_sensitive {
                    abbr.clone()
                } else {
                    abbr.to_lowercase()
                };
                active.push(Active {
                    id: s.id,
                    key,
                    abbr,
                    mode,
                });
            }
        }
    }
    for (i, a) in active.iter().enumerate() {
        for (j, b) in active.iter().enumerate() {
            if i == j {
                continue;
            }
            if a.key == b.key && a.mode == b.mode && j < i {
                out.push(Lint {
                    snippet_id: a.id,
                    message: format!(
                        "Duplicate abbreviation '{}': an earlier snippet wins.",
                        a.abbr
                    ),
                });
            }
            if a.mode == TriggerMode::Immediate
                && b.key.len() > a.key.len()
                && b.key.starts_with(&a.key)
            {
                out.push(Lint {
                    snippet_id: b.id,
                    message: format!(
                        "Unreachable while immediate snippet '{}' is enabled (it fires first).",
                        a.abbr
                    ),
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Group, Snippet, SnippetTrigger};
    use chrono::{Local, TimeZone};

    fn lib(snippets: Vec<Snippet>) -> Library {
        let mut g = Group::new("g");
        g.snippets = snippets;
        Library {
            groups: vec![g],
            ..Library::empty()
        }
    }

    fn ctx() -> RenderContext {
        RenderContext::new(
            Local.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap(),
            Some("CLIP".into()),
        )
    }

    /// Simulates a text field: applies expansions to a String like the playground does.
    fn type_into(engine: &mut Engine, keys: &str) -> String {
        let mut text = String::new();
        for c in keys.chars() {
            if c == '\u{8}' {
                text.pop();
                engine.feed(KeyInput::Backspace);
                continue;
            }
            text.push(c);
            if let Some(exp) = engine.feed(KeyInput::Char(c)) {
                let r = exp.render(&ctx());
                for _ in 0..r.delete_chars {
                    text.pop();
                }
                text.push_str(&r.insert);
            }
        }
        text
    }

    #[test]
    fn delimiter_trigger_keeps_delimiter() {
        let mut e = Engine::new(&lib(vec![Snippet::new("", "hlo", "Hello")]));
        assert_eq!(type_into(&mut e, "hlo "), "Hello ");
        assert_eq!(type_into(&mut e, "say hlo."), "say Hello.");
    }

    #[test]
    fn delimiter_trigger_without_keep() {
        let mut l = lib(vec![Snippet::new("", "hlo", "Hello")]);
        l.settings.keep_delimiter = false;
        let mut e = Engine::new(&l);
        assert_eq!(type_into(&mut e, "hlo "), "Hello");
    }

    #[test]
    fn no_expansion_without_delimiter() {
        let mut e = Engine::new(&lib(vec![Snippet::new("", "hlo", "Hello")]));
        assert_eq!(type_into(&mut e, "hlo"), "hlo");
        assert!(e.feed_str("x").is_empty());
    }

    #[test]
    fn immediate_trigger() {
        let mut e = Engine::new(&lib(vec![
            Snippet::new("", "->>", "→").trigger(SnippetTrigger::Immediate)
        ]));
        assert_eq!(type_into(&mut e, "a ->> b"), "a → b");
    }

    #[test]
    fn word_boundary_required() {
        let mut e = Engine::new(&lib(vec![Snippet::new("", "hlo", "Hello")]));
        assert_eq!(type_into(&mut e, "ohlo "), "ohlo ");
        assert_eq!(type_into(&mut e, "x_hlo "), "x_Hello ");
        e.reset();
        assert_eq!(type_into(&mut e, "1hlo "), "1hlo ");
    }

    #[test]
    fn word_boundary_can_be_disabled() {
        let mut l = lib(vec![Snippet::new("", "hlo", "Hello")]);
        l.settings.require_word_boundary = false;
        let mut e = Engine::new(&l);
        assert_eq!(type_into(&mut e, "ohlo "), "oHello ");
    }

    #[test]
    fn punctuation_abbreviation_skips_boundary() {
        let mut e = Engine::new(&lib(vec![
            Snippet::new("", ";d", "{{date}}").trigger(SnippetTrigger::Immediate)
        ]));
        assert_eq!(type_into(&mut e, "today;d"), "today2026-01-02");
    }

    #[test]
    fn longest_match_wins_delimiter() {
        let mut e = Engine::new(&lib(vec![
            Snippet::new("", "lo", "SHORT").trigger(SnippetTrigger::Immediate),
            Snippet::new("", "ad", "AD"),
            Snippet::new("", "addr", "1 Example Street"),
        ]));
        assert_eq!(type_into(&mut e, "addr "), "1 Example Street ");
        e.reset();
        assert_eq!(type_into(&mut e, "ad "), "AD ");
    }

    #[test]
    fn longest_match_wins_immediate_suffix_overlap() {
        // Both "x" and "ox" end the buffer when "ox" is typed; boundary disabled so both are eligible.
        let mut l = lib(vec![
            Snippet::new("", "x", "1").trigger(SnippetTrigger::Immediate),
            Snippet::new("", "ox", "2").trigger(SnippetTrigger::Immediate),
        ]);
        l.settings.require_word_boundary = false;
        let mut e = Engine::new(&l);
        let exps = e.feed_str("ox");
        assert_eq!(exps.len(), 1);
        assert_eq!(exps[0].body, "2");
        assert_eq!(exps[0].typed, "ox");
    }

    #[test]
    fn tie_break_is_deterministic() {
        let a = Snippet::new("first", "dup", "A");
        let b = Snippet::new("second", "dup", "B");
        let mut e = Engine::new(&lib(vec![a, b]));
        assert_eq!(type_into(&mut e, "dup "), "A ");
        // Case-sensitive rule beats case-insensitive one of equal length.
        let mut e = Engine::new(&lib(vec![
            Snippet::new("", "Dup", "insensitive"),
            Snippet::new("", "Dup", "sensitive").case_sensitive(true),
        ]));
        assert_eq!(type_into(&mut e, "Dup "), "sensitive ");
    }

    #[test]
    fn case_sensitivity() {
        let mut e = Engine::new(&lib(vec![
            Snippet::new("", "API", "Application Programming Interface").case_sensitive(true),
            Snippet::new("", "brb", "be right back"),
        ]));
        assert_eq!(type_into(&mut e, "api "), "api ");
        assert_eq!(
            type_into(&mut e, "API "),
            "Application Programming Interface "
        );
        assert_eq!(type_into(&mut e, "BRB "), "be right back ");
    }

    #[test]
    fn unicode_vietnamese_abbreviation_and_body() {
        let mut e = Engine::new(&lib(vec![
            Snippet::new("", "cmơn", "Cảm ơn bạn nhiều nhé."),
            Snippet::new("", "đc", "địa chỉ"),
        ]));
        assert_eq!(type_into(&mut e, "cmơn "), "Cảm ơn bạn nhiều nhé. ");
        assert_eq!(type_into(&mut e, "ĐC,"), "địa chỉ,");
        // Vietnamese letters count as word characters for the boundary check.
        e.reset();
        assert_eq!(type_into(&mut e, "ơđc "), "ơđc ");
    }

    #[test]
    fn unicode_decomposed_input_is_composed() {
        let mut e = Engine::new(&lib(vec![Snippet::new("", "cmơn", "OK")]));
        // "ơ" typed as 'o' + COMBINING HORN (U+031B).
        let exps = e.feed_str("cmo\u{31B}n ");
        assert_eq!(exps.len(), 1);
        assert_eq!(exps[0].abbreviation, "cmơn");
        // `typed` is the raw input: 6 chars, not the 5-char NFC form.
        assert_eq!(exps[0].typed, "cmo\u{31B}n ");
        let r = exps[0].render(&ctx());
        assert_eq!(r.delete_chars, 6);
    }

    #[test]
    fn decomposed_input_is_fully_replaced_in_text_field() {
        let mut e = Engine::new(&lib(vec![Snippet::new("", "cmơn", "Cảm ơn")]));
        assert_eq!(type_into(&mut e, "x cmo\u{31B}n "), "x Cảm ơn ");
        // Decomposed "ệ" = e + U+0323 + U+0302 (two combining marks).
        let mut e = Engine::new(&lib(vec![Snippet::new("", "hệ", "hệ thống")]));
        assert_eq!(type_into(&mut e, "he\u{323}\u{302} "), "hệ thống ");
    }

    #[test]
    fn backspace_removes_one_raw_char_of_composed_entry() {
        let mut e = Engine::new(&lib(vec![Snippet::new("", "cmo", "X")]));
        // Type "cmơ" decomposed, delete the horn: buffer is "cmo" again.
        e.feed_str("cmo\u{31B}");
        e.feed(KeyInput::Backspace);
        assert_eq!(e.buffer(), "cmo");
        assert_eq!(e.feed_str(" ").len(), 1);
    }

    #[test]
    fn unicode_abbreviation_stored_decomposed_still_matches() {
        let mut e = Engine::new(&lib(vec![Snippet::new("", "cmo\u{31B}n", "OK")]));
        assert_eq!(e.feed_str("cmơn ").len(), 1);
    }

    #[test]
    fn emoji_and_cjk() {
        let mut e = Engine::new(&lib(vec![
            Snippet::new("", ":ok", "👍🏽 done").trigger(SnippetTrigger::Immediate),
            Snippet::new("", "nh", "你好"),
        ]));
        assert_eq!(type_into(&mut e, "🙂 :ok"), "🙂 👍🏽 done");
        assert_eq!(type_into(&mut e, " nh\n"), " 你好\n");
    }

    #[test]
    fn multiline_body_and_return_delimiter() {
        let mut e = Engine::new(&lib(vec![Snippet::new("", "sig", "Best,\nAlex")]));
        assert_eq!(type_into(&mut e, "sig\n"), "Best,\nAlex\n");
        let exp = e.feed_str("sig\r");
        assert_eq!(exp[0].typed, "sig\n");
    }

    #[test]
    fn backspace_corrects_buffer() {
        let mut e = Engine::new(&lib(vec![Snippet::new("", "hlo", "Hello")]));
        assert_eq!(type_into(&mut e, "hlx\u{8}o "), "Hello ");
        e.reset();
        assert_eq!(type_into(&mut e, "hlo\u{8}\u{8}\u{8}hlo "), "Hello ");
    }

    #[test]
    fn backspace_on_empty_buffer_is_harmless() {
        let mut e = Engine::new(&lib(vec![Snippet::new("", "hlo", "Hello")]));
        e.feed(KeyInput::Backspace);
        e.feed(KeyInput::Backspace);
        assert_eq!(e.feed_str("hlo ").len(), 1);
    }

    #[test]
    fn reset_breaks_partial_abbreviation() {
        let mut e = Engine::new(&lib(vec![Snippet::new("", "hlo", "Hello")]));
        e.feed_str("hl");
        e.feed(KeyInput::Reset);
        assert!(e.feed_str("o ").is_empty());
        // After a reset the buffer start counts as a boundary.
        e.feed_str("abc");
        e.feed(KeyInput::Reset);
        assert_eq!(e.feed_str("hlo ").len(), 1);
    }

    #[test]
    fn control_chars_reset() {
        let mut e = Engine::new(&lib(vec![Snippet::new("", "hlo", "Hello")]));
        e.feed_str("hl");
        e.feed(KeyInput::Char('\u{1b}'));
        assert!(e.feed_str("o ").is_empty());
    }

    #[test]
    fn disabled_snippet_and_group_never_match() {
        let mut s = Snippet::new("", "off", "nope");
        s.enabled = false;
        let mut l = lib(vec![s, Snippet::new("", "on", "yes")]);
        let mut g2 = Group::new("disabled");
        g2.enabled = false;
        g2.snippets.push(Snippet::new("", "grp", "nope"));
        l.groups.push(g2);
        let mut e = Engine::new(&l);
        assert_eq!(e.active_rule_count(), 1);
        assert_eq!(type_into(&mut e, "off grp on "), "off grp yes ");
    }

    #[test]
    fn disabled_longer_rule_falls_back_to_shorter() {
        let mut long = Snippet::new("", "addr", "LONG");
        long.enabled = false;
        let mut e = Engine::new(&lib(vec![
            long,
            Snippet::new("", "dr", "SHORT").trigger(SnippetTrigger::Immediate),
        ]));
        // With boundary required, "dr" inside "addr" must not fire.
        assert_eq!(type_into(&mut e, "addr"), "addr");
        e.reset();
        assert_eq!(type_into(&mut e, "dr"), "SHORT");
    }

    #[test]
    fn custom_delimiters() {
        let mut l = lib(vec![Snippet::new("", "hlo", "Hello")]);
        l.settings.delimiters = "#".into();
        let mut e = Engine::new(&l);
        assert_eq!(type_into(&mut e, "hlo hlo#"), "hlo Hello#");
    }

    #[test]
    fn buffer_is_bounded() {
        let mut e = Engine::new(&lib(vec![Snippet::new("", "hlo", "Hello")]));
        let long: String = std::iter::repeat_n('a', 500).collect();
        e.feed_str(&long);
        assert!(e.buffer().chars().count() <= MAX_BUFFER_CHARS);
        // Abbreviation glued to the long word must not match; after a space it must.
        assert!(e.feed_str("hlo ").is_empty());
        assert_eq!(e.feed_str("hlo ").len(), 1);
    }

    #[test]
    fn truncated_buffer_start_is_not_boundary() {
        let mut l = lib(vec![
            Snippet::new("", "aaaa", "X").trigger(SnippetTrigger::Immediate)
        ]);
        l.settings.require_word_boundary = true;
        let mut e = Engine::new(&l);
        let long: String = std::iter::repeat_n('b', MAX_BUFFER_CHARS + 10).collect();
        e.feed_str(&long);
        assert!(e.feed_str("aaaa").is_empty());
    }

    #[test]
    fn expansions_do_not_chain() {
        let mut e = Engine::new(&lib(vec![
            Snippet::new("", "aa", "bb").trigger(SnippetTrigger::Immediate),
            Snippet::new("", "bb", "cc").trigger(SnippetTrigger::Immediate),
        ]));
        assert_eq!(type_into(&mut e, "aa"), "bb");
    }

    #[test]
    fn render_with_cursor_and_suffix() {
        let mut e = Engine::new(&lib(vec![Snippet::new("", "tag", "<b>{{cursor}}</b>")]));
        let exp = e.feed_str("tag ").pop().unwrap();
        let r = exp.render(&ctx());
        assert_eq!(r.delete_chars, 4);
        assert_eq!(r.insert, "<b></b> ");
        assert_eq!(r.after_cursor, "</b> ");
    }

    #[test]
    fn variables_in_expansion() {
        let mut e = Engine::new(&lib(vec![Snippet::new(
            "",
            "now",
            "{{date}} {{time}} {{clipboard}} {{bogus}}",
        )]));
        let exp = e.feed_str("now ").pop().unwrap();
        let r = exp.render(&ctx());
        assert_eq!(r.insert, "2026-01-02 03:04 CLIP {{bogus}} ");
        assert_eq!(r.warnings.len(), 1);
    }

    #[test]
    fn lint_reports_conflicts() {
        let l = lib(vec![
            Snippet::new("", "ad", "x").trigger(SnippetTrigger::Immediate),
            Snippet::new("", "addr", "y").trigger(SnippetTrigger::Immediate),
            Snippet::new("", "dup", "1"),
            Snippet::new("", "DUP", "2"),
            Snippet::new("", "", "3"),
        ]);
        let lints = lint(&l);
        assert!(lints.iter().any(|x| x.message.contains("Unreachable")));
        assert!(lints.iter().any(|x| x.message.contains("Duplicate")));
        assert!(lints.iter().any(|x| x.message.contains("No abbreviation")));
    }
}

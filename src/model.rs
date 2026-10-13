//! Data model for the snippet library.
//!
//! The on-disk JSON format is owned by this project (`atext-rs.library`, version 1).
//! It is not compatible with, and never written over, files from other expansion tools.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Format literal written to disk. Kept from the pre-0.3 project name for compatibility:
/// renaming it is a schema change and is reserved for a future v2 schema.
pub const LIBRARY_FORMAT: &str = "atext-rs.library";
/// Format literals accepted when loading or importing (v1 schema).
pub const ACCEPTED_FORMATS: [&str; 2] = [LIBRARY_FORMAT, "typelune.library"];
pub const LIBRARY_VERSION: u32 = 1;

/// When a snippet fires.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TriggerMode {
    /// Expand as soon as the last character of the abbreviation is typed.
    Immediate,
    /// Expand only after a delimiter character (space, punctuation, newline, ...) is typed.
    #[default]
    Delimiter,
}

impl TriggerMode {
    pub fn label(self) -> &'static str {
        match self {
            TriggerMode::Immediate => "Immediately",
            TriggerMode::Delimiter => "After delimiter",
        }
    }
}

/// Per-snippet trigger override.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SnippetTrigger {
    #[default]
    UseDefault,
    Immediate,
    Delimiter,
}

impl SnippetTrigger {
    pub fn resolve(self, default: TriggerMode) -> TriggerMode {
        match self {
            SnippetTrigger::UseDefault => default,
            SnippetTrigger::Immediate => TriggerMode::Immediate,
            SnippetTrigger::Delimiter => TriggerMode::Delimiter,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SnippetTrigger::UseDefault => "Library default",
            SnippetTrigger::Immediate => "Immediately",
            SnippetTrigger::Delimiter => "After delimiter",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snippet {
    pub id: Uuid,
    #[serde(default)]
    pub title: String,
    pub abbreviation: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub trigger: SnippetTrigger,
}

impl Snippet {
    pub fn new(title: &str, abbreviation: &str, body: &str) -> Self {
        Self {
            id: Uuid::new_v4(),
            title: title.to_owned(),
            abbreviation: abbreviation.to_owned(),
            body: body.to_owned(),
            tags: Vec::new(),
            enabled: true,
            case_sensitive: false,
            trigger: SnippetTrigger::UseDefault,
        }
    }

    pub fn with_tags(mut self, tags: &[&str]) -> Self {
        self.tags = tags.iter().map(|t| (*t).to_owned()).collect();
        self
    }

    pub fn case_sensitive(mut self, value: bool) -> Self {
        self.case_sensitive = value;
        self
    }

    pub fn trigger(mut self, trigger: SnippetTrigger) -> Self {
        self.trigger = trigger;
        self
    }

    /// Case-insensitive search across title, abbreviation, body and tags.
    pub fn matches_query(&self, query: &str) -> bool {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return true;
        }
        self.title.to_lowercase().contains(&q)
            || self.abbreviation.to_lowercase().contains(&q)
            || self.body.to_lowercase().contains(&q)
            || self.tags.iter().any(|t| t.to_lowercase().contains(&q))
    }

    pub fn display_name(&self) -> String {
        if self.title.trim().is_empty() {
            let first_line = self.body.lines().next().unwrap_or("").trim();
            if first_line.is_empty() {
                "(untitled)".to_owned()
            } else {
                truncate_chars(first_line, 40)
            }
        } else {
            self.title.clone()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Group {
    pub id: Uuid,
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub snippets: Vec<Snippet>,
}

impl Group {
    pub fn new(name: &str) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.to_owned(),
            enabled: true,
            snippets: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// Default trigger for snippets that do not override it.
    #[serde(default)]
    pub default_trigger: TriggerMode,
    /// Characters that complete an abbreviation in delimiter mode.
    #[serde(default = "default_delimiters")]
    pub delimiters: String,
    /// Re-insert the delimiter that triggered the expansion after the expanded text.
    #[serde(default = "default_true")]
    pub keep_delimiter: bool,
    /// Require that the abbreviation starts at a word boundary
    /// (start of input or after a non-alphanumeric character).
    #[serde(default = "default_true")]
    pub require_word_boundary: bool,
}

pub fn default_delimiters() -> String {
    " \t\n.,;:!?()[]{}\"'".to_owned()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            default_trigger: TriggerMode::Delimiter,
            delimiters: default_delimiters(),
            keep_delimiter: true,
            require_word_boundary: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Library {
    pub format: String,
    pub version: u32,
    #[serde(default)]
    pub settings: Settings,
    #[serde(default)]
    pub groups: Vec<Group>,
}

impl Default for Library {
    fn default() -> Self {
        Self::empty()
    }
}

impl Library {
    pub fn empty() -> Self {
        Self {
            format: LIBRARY_FORMAT.to_owned(),
            version: LIBRARY_VERSION,
            settings: Settings::default(),
            groups: Vec::new(),
        }
    }

    /// Fabricated, harmless demo content. Contains no real personal data.
    pub fn demo() -> Self {
        let mut general = Group::new("General");
        general.snippets = vec![
            Snippet::new("Greeting", "hlo", "Hello there!").with_tags(&["demo", "greeting"]),
            Snippet::new(
                "Sign-off",
                "sig",
                "Best regards,\nAlex Example\nexample.com",
            )
            .with_tags(&["demo", "email"]),
            Snippet::new("Demo email", "eml", "alex@example.com").with_tags(&["demo"]),
            Snippet::new(
                "Meeting note",
                "mtg",
                "Meeting notes {{date}}\n\nAttendees: {{cursor}}\nAction items:\n- ",
            )
            .with_tags(&["demo", "template"]),
        ];

        let mut dates = Group::new("Dates & clipboard");
        dates.snippets = vec![
            Snippet::new("Today (ISO)", ";d", "{{date}}")
                .with_tags(&["date"])
                .trigger(SnippetTrigger::Immediate),
            Snippet::new("Now (time)", ";t", "{{time}}")
                .with_tags(&["time"])
                .trigger(SnippetTrigger::Immediate),
            Snippet::new("Long date", ";ld", "{{date:%A, %d %B %Y}}")
                .with_tags(&["date"])
                .trigger(SnippetTrigger::Immediate),
            Snippet::new("Quote clipboard", ";q", "“{{clipboard}}”")
                .with_tags(&["clipboard"])
                .trigger(SnippetTrigger::Immediate),
        ];

        let mut unicode = Group::new("Unicode samples");
        unicode.snippets = vec![
            Snippet::new(
                "Vietnamese greeting",
                "xc",
                "Xin chào, rất vui được gặp bạn!",
            )
            .with_tags(&["vietnamese", "unicode"]),
            Snippet::new("Vietnamese thanks", "cmơn", "Cảm ơn bạn nhiều nhé.")
                .with_tags(&["vietnamese", "unicode"]),
            Snippet::new("Arrow", "->>", "→")
                .with_tags(&["symbol"])
                .trigger(SnippetTrigger::Immediate),
            Snippet::new("Shrug", "shrug", "¯\\_(ツ)_/¯").with_tags(&["emoji"]),
            Snippet::new(
                "Case sensitive API",
                "API",
                "Application Programming Interface",
            )
            .with_tags(&["case"])
            .case_sensitive(true),
        ];

        let mut archived = Group::new("Archived (disabled)");
        archived.enabled = false;
        archived.snippets = vec![
            Snippet::new("Old greeting", "hlo2", "This group is disabled.").with_tags(&["demo"]),
        ];

        Self {
            groups: vec![general, dates, unicode, archived],
            ..Self::empty()
        }
    }

    pub fn snippet_count(&self) -> usize {
        self.groups.iter().map(|g| g.snippets.len()).sum()
    }

    pub fn find_snippet(&self, id: Uuid) -> Option<(usize, usize)> {
        for (gi, g) in self.groups.iter().enumerate() {
            if let Some(si) = g.snippets.iter().position(|s| s.id == id) {
                return Some((gi, si));
            }
        }
        None
    }

    pub fn group_index(&self, id: Uuid) -> Option<usize> {
        self.groups.iter().position(|g| g.id == id)
    }
}

fn default_true() -> bool {
    true
}

pub fn truncate_chars(s: &str, max: usize) -> String {
    let mut out: String = s.chars().take(max).collect();
    if s.chars().count() > max {
        out.push('…');
    }
    out
}

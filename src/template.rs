//! Placeholder rendering for snippet bodies.
//!
//! Supported syntax (POC):
//!
//! | Token                | Result                                         |
//! |----------------------|------------------------------------------------|
//! | `{{date}}`           | local date, `YYYY-MM-DD`                       |
//! | `{{time}}`           | local time, `HH:MM`                            |
//! | `{{datetime}}`       | local date and time, `YYYY-MM-DD HH:MM`        |
//! | `{{date:FORMAT}}`    | local date/time with a strftime `FORMAT`       |
//! | `{{time:FORMAT}}`    | same as `{{date:FORMAT}}`                      |
//! | `{{clipboard}}`      | current plain-text clipboard                   |
//! | `{{cursor}}`         | place the caret here after expansion (first only) |
//! | `\{{`                | a literal `{{`                                 |
//!
//! Anything else inside `{{ }}` is preserved verbatim and reported as a warning,
//! so mistakes stay visible instead of being silently dropped.

use chrono::format::{Item, StrftimeItems};
use chrono::{DateTime, Local};

pub const CURSOR_TOKEN: &str = "{{cursor}}";

/// Inputs available while rendering. Tests pass fixed values; the app passes live ones.
#[derive(Debug, Clone)]
pub struct RenderContext {
    pub now: DateTime<Local>,
    /// `None` means the clipboard could not be read (token is preserved).
    pub clipboard: Option<String>,
}

impl RenderContext {
    pub fn new(now: DateTime<Local>, clipboard: Option<String>) -> Self {
        Self { now, clipboard }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    pub text: String,
    /// Caret position as a char index into `text`, if `{{cursor}}` was used.
    pub cursor: Option<usize>,
    pub warnings: Vec<String>,
}

impl Rendered {
    /// Number of chars between the caret and the end of the text.
    pub fn chars_after_cursor(&self) -> usize {
        match self.cursor {
            Some(c) => self.text.chars().count() - c,
            None => 0,
        }
    }

    /// Text after the caret (used to compute how far to move the caret back).
    pub fn text_after_cursor(&self) -> &str {
        match self.cursor {
            Some(c) => {
                let byte = self
                    .text
                    .char_indices()
                    .nth(c)
                    .map(|(b, _)| b)
                    .unwrap_or(self.text.len());
                &self.text[byte..]
            }
            None => "",
        }
    }
}

/// Does the body reference the clipboard? Lets callers skip clipboard access when unneeded.
pub fn uses_clipboard(body: &str) -> bool {
    body.contains("{{clipboard}}")
}

pub fn render(body: &str, ctx: &RenderContext) -> Rendered {
    let mut out = String::with_capacity(body.len());
    let mut out_chars = 0usize;
    let mut cursor: Option<usize> = None;
    let mut warnings = Vec::new();
    let mut rest = body;

    let push = |out: &mut String, out_chars: &mut usize, s: &str| {
        out.push_str(s);
        *out_chars += s.chars().count();
    };

    while let Some(open) = rest.find("{{") {
        let (before, after_open) = rest.split_at(open);
        // Escaped `\{{` -> literal `{{`.
        if let Some(prefix) = before.strip_suffix('\\') {
            push(&mut out, &mut out_chars, prefix);
            push(&mut out, &mut out_chars, "{{");
            rest = &after_open[2..];
            continue;
        }
        push(&mut out, &mut out_chars, before);

        let inner_start = &after_open[2..];
        let Some(close) = inner_start.find("}}") else {
            warnings.push("Unterminated '{{' kept as text".to_owned());
            push(&mut out, &mut out_chars, after_open);
            rest = "";
            break;
        };
        let token = &inner_start[..close];
        let raw = &after_open[..close + 4];
        rest = &inner_start[close + 2..];

        match expand_token(token, ctx) {
            TokenResult::Text(s) => push(&mut out, &mut out_chars, &s),
            TokenResult::Cursor => {
                if cursor.is_none() {
                    cursor = Some(out_chars);
                } else {
                    warnings.push(
                        "Only the first {{cursor}} is used; extra one kept as text".to_owned(),
                    );
                    push(&mut out, &mut out_chars, raw);
                }
            }
            TokenResult::Keep(reason) => {
                warnings.push(reason);
                push(&mut out, &mut out_chars, raw);
            }
        }
    }
    push(&mut out, &mut out_chars, rest);

    Rendered {
        text: out,
        cursor,
        warnings,
    }
}

enum TokenResult {
    Text(String),
    Cursor,
    Keep(String),
}

fn expand_token(token: &str, ctx: &RenderContext) -> TokenResult {
    match token {
        "date" => TokenResult::Text(ctx.now.format("%Y-%m-%d").to_string()),
        "time" => TokenResult::Text(ctx.now.format("%H:%M").to_string()),
        "datetime" => TokenResult::Text(ctx.now.format("%Y-%m-%d %H:%M").to_string()),
        "cursor" => TokenResult::Cursor,
        "clipboard" => match &ctx.clipboard {
            Some(text) => TokenResult::Text(text.clone()),
            None => TokenResult::Keep("Clipboard has no plain text; {{clipboard}} kept".to_owned()),
        },
        _ => {
            if let Some(fmt) = token
                .strip_prefix("date:")
                .or_else(|| token.strip_prefix("time:"))
            {
                match format_date(ctx, fmt) {
                    Some(s) => TokenResult::Text(s),
                    None => TokenResult::Keep(format!("Invalid date format '{fmt}' kept as text")),
                }
            } else {
                TokenResult::Keep(format!(
                    "Unknown placeholder '{{{{{token}}}}}' kept as text"
                ))
            }
        }
    }
}

fn format_date(ctx: &RenderContext, fmt: &str) -> Option<String> {
    if fmt.is_empty() {
        return None;
    }
    let items: Vec<Item<'_>> = StrftimeItems::new(fmt).collect();
    if items.iter().any(|i| matches!(i, Item::Error)) {
        return None;
    }
    use std::fmt::Write;
    let mut s = String::new();
    write!(s, "{}", ctx.now.format_with_items(items.into_iter())).ok()?;
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ctx() -> RenderContext {
        let now = Local.with_ymd_and_hms(2026, 10, 13, 9, 5, 0).unwrap();
        RenderContext::new(now, Some("clip✓".to_owned()))
    }

    #[test]
    fn plain_text_untouched() {
        let r = render("Hello\nworld", &ctx());
        assert_eq!(r.text, "Hello\nworld");
        assert_eq!(r.cursor, None);
        assert!(r.warnings.is_empty());
    }

    #[test]
    fn date_time_tokens() {
        let r = render("{{date}} {{time}} | {{datetime}}", &ctx());
        assert_eq!(r.text, "2026-10-13 09:05 | 2026-10-13 09:05");
    }

    #[test]
    fn custom_date_format() {
        let r = render("{{date:%d/%m/%Y}}", &ctx());
        assert_eq!(r.text, "13/10/2026");
        let r = render("{{time:%H.%M}}", &ctx());
        assert_eq!(r.text, "09.05");
    }

    #[test]
    fn invalid_date_format_is_kept() {
        let r = render("x {{date:%Q}} y", &ctx());
        assert_eq!(r.text, "x {{date:%Q}} y");
        assert_eq!(r.warnings.len(), 1);
    }

    #[test]
    fn clipboard_token() {
        assert_eq!(render("[{{clipboard}}]", &ctx()).text, "[clip✓]");
        let no_clip = RenderContext::new(ctx().now, None);
        let r = render("[{{clipboard}}]", &no_clip);
        assert_eq!(r.text, "[{{clipboard}}]");
        assert_eq!(r.warnings.len(), 1);
    }

    #[test]
    fn unknown_tokens_preserved_visibly() {
        let r = render("a {{nope}} b {{ date }}", &ctx());
        assert_eq!(r.text, "a {{nope}} b {{ date }}");
        assert_eq!(r.warnings.len(), 2);
    }

    #[test]
    fn unterminated_token_preserved() {
        let r = render("a {{date b", &ctx());
        assert_eq!(r.text, "a {{date b");
        assert_eq!(r.warnings.len(), 1);
    }

    #[test]
    fn escape_literal_braces() {
        let r = render(r"show \{{date}} literally", &ctx());
        assert_eq!(r.text, "show {{date}} literally");
        assert!(r.warnings.is_empty());
    }

    #[test]
    fn cursor_position_in_chars_with_unicode() {
        let r = render("Chào {{cursor}}bạn", &ctx());
        assert_eq!(r.text, "Chào bạn");
        assert_eq!(r.cursor, Some(5));
        assert_eq!(r.chars_after_cursor(), 3);
        assert_eq!(r.text_after_cursor(), "bạn");
    }

    #[test]
    fn only_first_cursor_used() {
        let r = render("a{{cursor}}b{{cursor}}c", &ctx());
        assert_eq!(r.text, "ab{{cursor}}c");
        assert_eq!(r.cursor, Some(1));
        assert_eq!(r.warnings.len(), 1);
    }

    #[test]
    fn multiline_cursor() {
        let r = render("line1\n{{cursor}}\nline3", &ctx());
        assert_eq!(r.text, "line1\n\nline3");
        assert_eq!(r.cursor, Some(6));
        assert_eq!(r.text_after_cursor(), "\nline3");
    }
}

//! CSV import with a preview report. Nothing here touches the current library: the caller
//! shows the report and merges or replaces only after the user confirms.
//!
//! Format (RFC 4180 style, see `docs/import.md`):
//! * UTF-8, with or without a byte-order mark. Other encodings are refused.
//! * Comma-separated. Fields may be quoted with `"`; inside quotes, `""` is a literal quote
//!   and commas and line breaks are kept. CRLF, LF and CR line endings are accepted.
//! * The first record is a header. Column names are matched case-insensitively after
//!   trimming: `abbreviation` and `body` are required; `title`, `group` and `tags` are
//!   optional. Other columns are ignored and listed in the report.
//! * `tags` are separated by commas or semicolons. An empty `group` means "Imported".
//!
//! Rows that cannot be imported (wrong number of fields, empty or too-long abbreviation,
//! limits exceeded, duplicate abbreviation within the file) are skipped and listed with
//! their line number. Bodies are imported as plain text: placeholders that Typelune does not
//! support, and `%`-style codes from other tools, stay literal and are listed as warnings.
//! Imported content is never executed.
//!
//! Parsing is bounded: file size, field length, column count and row count are all capped.

use crate::engine::MAX_ABBREVIATION_CHARS;
use crate::model::{Group, Library, Snippet};
use crate::store::{
    self, StoreError, MAX_BODY_CHARS, MAX_FILE_BYTES, MAX_NAME_CHARS, MAX_SNIPPETS, MAX_TAGS,
    MAX_TAG_CHARS,
};
use crate::template::{self, RenderContext};
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;

pub const MAX_COLUMNS: usize = 64;
/// Upper bound for any single field, in characters.
pub const MAX_FIELD_CHARS: usize = MAX_BODY_CHARS;
/// Issues listed individually in the report; the rest are only counted.
pub const MAX_LISTED_ISSUES: usize = 200;
pub const DEFAULT_GROUP: &str = "Imported";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CsvIssue {
    /// 1-based line where the record starts.
    pub line: usize,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CsvReport {
    /// Data records read (excluding the header and blank lines).
    pub rows: usize,
    pub imported: usize,
    pub skipped: Vec<CsvIssue>,
    pub skipped_count: usize,
    pub warnings: Vec<CsvIssue>,
    pub warning_count: usize,
    pub ignored_columns: Vec<String>,
}

impl CsvReport {
    fn skip(&mut self, line: usize, message: impl Into<String>) {
        self.skipped_count += 1;
        if self.skipped.len() < MAX_LISTED_ISSUES {
            self.skipped.push(CsvIssue {
                line,
                message: message.into(),
            });
        }
    }

    pub fn warn(&mut self, line: usize, message: impl Into<String>) {
        self.warning_count += 1;
        if self.warnings.len() < MAX_LISTED_ISSUES {
            self.warnings.push(CsvIssue {
                line,
                message: message.into(),
            });
        }
    }
}

#[derive(Debug)]
pub struct CsvImport {
    pub library: Library,
    pub report: CsvReport,
}

/// A file-level problem: nothing is imported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CsvError(pub String);

impl std::fmt::Display for CsvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CsvError {}

/// One parsed record and the line it starts on.
type Record = (usize, Vec<String>);

/// Split CSV text into records. Bounded by [`MAX_COLUMNS`], [`MAX_FIELD_CHARS`] and
/// `max_records`.
#[allow(unused_assignments)] // the final `end_record!` resets state that is not read again
pub fn parse_records(text: &str, max_records: usize) -> Result<Vec<Record>, CsvError> {
    let mut records: Vec<Record> = Vec::new();
    let mut fields: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut field_chars = 0usize;
    let mut line = 1usize;
    let mut record_line = 1usize;
    let mut in_quotes = false;
    // The current field started with a quote (and the quote has been closed).
    let mut was_quoted = false;
    let mut chars = text.chars().peekable();

    let push_char = |field: &mut String, n: &mut usize, c: char, line: usize| {
        *n += 1;
        if *n > MAX_FIELD_CHARS {
            return Err(CsvError(format!(
                "Line {line}: a field is longer than {MAX_FIELD_CHARS} characters."
            )));
        }
        field.push(c);
        Ok(())
    };

    macro_rules! end_field {
        () => {{
            if fields.len() >= MAX_COLUMNS {
                return Err(CsvError(format!(
                    "Line {record_line}: more than {MAX_COLUMNS} columns."
                )));
            }
            fields.push(std::mem::take(&mut field));
            field_chars = 0;
            was_quoted = false;
        }};
    }
    macro_rules! end_record {
        () => {{
            end_field!();
            let blank = fields.len() == 1 && fields[0].is_empty();
            if !blank {
                if records.len() >= max_records {
                    return Err(CsvError(format!(
                        "The file has more than {} rows; split it into smaller files.",
                        max_records.saturating_sub(1)
                    )));
                }
                records.push((record_line, std::mem::take(&mut fields)));
            } else {
                fields.clear();
            }
        }};
    }

    while let Some(c) = chars.next() {
        if in_quotes {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    push_char(&mut field, &mut field_chars, '"', record_line)?;
                }
                '"' => {
                    in_quotes = false;
                    was_quoted = true;
                }
                '\r' => {
                    // Inside quotes a CRLF or CR is a line break in the value: keep it as LF.
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    line += 1;
                    push_char(&mut field, &mut field_chars, '\n', record_line)?;
                }
                '\n' => {
                    line += 1;
                    push_char(&mut field, &mut field_chars, '\n', record_line)?;
                }
                _ => push_char(&mut field, &mut field_chars, c, record_line)?,
            }
            continue;
        }
        match c {
            ',' => end_field!(),
            '\r' | '\n' => {
                if c == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                end_record!();
                line += 1;
                record_line = line;
            }
            '"' if field.is_empty() && !was_quoted => in_quotes = true,
            _ if was_quoted => {
                return Err(CsvError(format!(
                    "Line {line}: unexpected text after a closing quote. Quotes inside a quoted field must be doubled (\"\")."
                )));
            }
            _ => push_char(&mut field, &mut field_chars, c, record_line)?,
        }
    }
    if in_quotes {
        return Err(CsvError(format!(
            "Line {record_line}: a quoted field is never closed."
        )));
    }
    if !field.is_empty() || !fields.is_empty() || was_quoted {
        end_record!();
    }
    Ok(records)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Column {
    Abbreviation,
    Body,
    Title,
    Group,
    Tags,
}

fn column_for(header: &str) -> Option<Column> {
    match header.trim().to_lowercase().as_str() {
        "abbreviation" => Some(Column::Abbreviation),
        "body" => Some(Column::Body),
        "title" => Some(Column::Title),
        "group" => Some(Column::Group),
        "tags" => Some(Column::Tags),
        _ => None,
    }
}

/// `body` with every `{{…}}` span removed, so `%` codes inside Typelune placeholders (such as
/// `{{date:%d/%m/%Y}}`) are not mistaken for another tool's codes.
fn outside_placeholders(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(open) = rest.find("{{") {
        out.push_str(&rest[..open]);
        match rest[open + 2..].find("}}") {
            Some(close) => rest = &rest[open + 2 + close + 2..],
            None => {
                rest = &rest[open..];
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Codes used by other text expanders (e.g. `%clipboard`, `%Y`, `%|`). Typelune does not
/// interpret them; they are inserted literally, so the user should know.
fn foreign_code(body: &str) -> Option<String> {
    let body = &outside_placeholders(body);
    let mut it = body.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        if c == '%' {
            if let Some(&(_, n)) = it.peek() {
                if n.is_ascii_alphabetic() || matches!(n, '|' | '{' | '@' | '>' | '<') {
                    let code: String = body[i..].chars().take(12).collect();
                    let code: String = code
                        .split(|ch: char| ch.is_whitespace())
                        .next()
                        .unwrap_or("")
                        .into();
                    return Some(code);
                }
            }
        }
    }
    None
}

fn normalize_newlines(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

/// Decode and parse CSV bytes into a new library plus a report.
pub fn import_bytes(bytes: &[u8]) -> Result<CsvImport, CsvError> {
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(CsvError(format!(
            "The file is {} bytes; the limit is {MAX_FILE_BYTES} bytes.",
            bytes.len()
        )));
    }
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let text = std::str::from_utf8(bytes).map_err(|e| {
        CsvError(format!(
            "The file is not UTF-8 text (invalid byte at offset {}). Save it as “CSV UTF-8” and try again.",
            e.valid_up_to()
        ))
    })?;
    let records = parse_records(text, MAX_SNIPPETS + 1)?;
    let Some(((_, header), rows)) = records.split_first() else {
        return Err(CsvError("The file is empty.".into()));
    };

    let mut report = CsvReport::default();
    let mut columns: Vec<Option<Column>> = Vec::with_capacity(header.len());
    for name in header {
        let col = column_for(name);
        if let Some(c) = col {
            if columns.contains(&Some(c)) {
                return Err(CsvError(format!(
                    "The header lists “{}” more than once.",
                    name.trim()
                )));
            }
        } else {
            report
                .ignored_columns
                .push(crate::model::truncate_chars(name.trim(), 60));
        }
        columns.push(col);
    }
    let index = |c: Column| columns.iter().position(|x| *x == Some(c));
    let (Some(abbr_i), Some(body_i)) = (index(Column::Abbreviation), index(Column::Body)) else {
        return Err(CsvError(
            "The first row must be a header with at least “abbreviation” and “body” columns (optional: title, group, tags).".into(),
        ));
    };
    let (title_i, group_i, tags_i) = (
        index(Column::Title),
        index(Column::Group),
        index(Column::Tags),
    );

    let ctx = RenderContext::new(chrono::Local::now(), Some(String::new()));
    let mut groups: Vec<Group> = Vec::new();
    let mut group_index: HashMap<String, usize> = HashMap::new();
    let mut first_line_of: HashMap<String, usize> = HashMap::new();

    for (line, fields) in rows {
        let line = *line;
        report.rows += 1;
        if fields.len() != header.len() {
            report.skip(
                line,
                format!(
                    "has {} fields; the header has {}",
                    fields.len(),
                    header.len()
                ),
            );
            continue;
        }
        let get = |i: Option<usize>| i.map(|i| fields[i].as_str()).unwrap_or("");
        let abbreviation = fields[abbr_i].trim().to_owned();
        let body = normalize_newlines(&fields[body_i]);
        let title = get(title_i).trim().to_owned();
        let group = get(group_i).trim();
        let group = if group.is_empty() {
            DEFAULT_GROUP.to_owned()
        } else {
            group.to_owned()
        };
        let mut tags: Vec<String> = Vec::new();
        for t in get(tags_i).split([',', ';']) {
            let t = t.trim();
            if !t.is_empty() && !tags.iter().any(|x| x == t) {
                tags.push(t.to_owned());
            }
        }

        let problem = if abbreviation.is_empty() {
            Some("empty abbreviation".to_owned())
        } else if abbreviation.chars().count() > MAX_ABBREVIATION_CHARS {
            Some(format!(
                "abbreviation longer than {MAX_ABBREVIATION_CHARS} characters"
            ))
        } else if abbreviation.chars().any(|c| c.is_control()) {
            Some("abbreviation contains a line break or control character".to_owned())
        } else if body.chars().count() > MAX_BODY_CHARS {
            Some(format!("body longer than {MAX_BODY_CHARS} characters"))
        } else if title.chars().count() > MAX_NAME_CHARS {
            Some(format!("title longer than {MAX_NAME_CHARS} characters"))
        } else if group.chars().count() > MAX_NAME_CHARS || group.chars().any(|c| c.is_control()) {
            Some("group name is too long or contains line breaks".to_owned())
        } else if tags.len() > MAX_TAGS {
            Some(format!("more than {MAX_TAGS} tags"))
        } else if tags.iter().any(|t| t.chars().count() > MAX_TAG_CHARS) {
            Some(format!("a tag is longer than {MAX_TAG_CHARS} characters"))
        } else {
            None
        };
        if let Some(problem) = problem {
            report.skip(line, problem);
            continue;
        }
        let key = abbreviation.to_lowercase();
        if let Some(first) = first_line_of.get(&key) {
            report.skip(
                line,
                format!("duplicate abbreviation “{abbreviation}”; the row on line {first} is kept"),
            );
            continue;
        }
        first_line_of.insert(key, line);

        if body.is_empty() {
            report.warn(line, format!("“{abbreviation}” has an empty body"));
        }
        let mut seen = Vec::new();
        for w in template::render(&body, &ctx).warnings {
            if !seen.contains(&w) {
                report.warn(line, format!("“{abbreviation}”: {w}"));
                seen.push(w);
            }
        }
        if let Some(code) = foreign_code(&body) {
            report.warn(
                line,
                format!("“{abbreviation}” contains “{code}”, which Typelune does not interpret; it will be typed literally"),
            );
        }

        let mut snippet = Snippet::new(&title, &abbreviation, &body);
        snippet.tags = tags;
        let gi = *group_index.entry(group.clone()).or_insert_with(|| {
            groups.push(Group::new(&group));
            groups.len() - 1
        });
        groups[gi].snippets.push(snippet);
        report.imported += 1;
    }

    let library = Library {
        groups,
        ..Library::empty()
    };
    let errors = store::validate(&library);
    if !errors.is_empty() {
        return Err(CsvError(StoreError::Invalid(errors).to_string()));
    }
    Ok(CsvImport { library, report })
}

/// Read a CSV file (regular files only, bounded, never blocking on a pipe) and parse it.
pub fn import_file(path: &Path) -> Result<CsvImport, CsvError> {
    let bytes = store::read_regular_file(path, MAX_FILE_BYTES, false)
        .map_err(|e| CsvError(e.to_string()))?;
    import_bytes(&bytes)
}

/// Add warnings for abbreviations that the target library already uses.
pub fn note_existing_conflicts(import: &mut CsvImport, existing: &Library) {
    let used: std::collections::HashSet<String> = existing
        .groups
        .iter()
        .flat_map(|g| g.snippets.iter())
        .map(|s| s.abbreviation.to_lowercase())
        .filter(|a| !a.is_empty())
        .collect();
    let mut conflicts = Vec::new();
    for s in import.library.groups.iter().flat_map(|g| g.snippets.iter()) {
        if used.contains(&s.abbreviation.to_lowercase()) {
            conflicts.push(s.abbreviation.clone());
        }
    }
    for a in conflicts {
        import.report.warn(
            0,
            format!("“{a}” is already used in your library; merging would create a duplicate"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snippets(i: &CsvImport) -> Vec<&Snippet> {
        i.library
            .groups
            .iter()
            .flat_map(|g| g.snippets.iter())
            .collect()
    }

    #[test]
    fn quoted_commas_newlines_escaped_quotes_crlf_and_bom() {
        let csv = "\u{feff}Abbreviation,Body,Title,Group,Tags\r\n\
                   sig,\"Best,\r\nAlex \"\"A\"\" Example\",Sign-off,Mail,\"demo, email\"\r\n\
                   xc,Xin chào,,,\r\n";
        let i = import_bytes(csv.as_bytes()).unwrap();
        assert_eq!(i.report.rows, 2);
        assert_eq!(i.report.imported, 2);
        assert!(i.report.skipped.is_empty());
        let s = snippets(&i);
        assert_eq!(s[0].abbreviation, "sig");
        assert_eq!(s[0].body, "Best,\nAlex \"A\" Example");
        assert_eq!(s[0].title, "Sign-off");
        assert_eq!(s[0].tags, ["demo", "email"]);
        assert_eq!(i.library.groups[0].name, "Mail");
        assert_eq!(s[1].body, "Xin chào");
        assert_eq!(i.library.groups[1].name, DEFAULT_GROUP);
        assert!(store::validate(&i.library).is_empty());
    }

    #[test]
    fn header_is_required_and_case_insensitive() {
        assert!(import_bytes(b"").unwrap_err().0.contains("empty"));
        assert!(import_bytes(b"title,body\nx,y\n")
            .unwrap_err()
            .0
            .contains("abbreviation"));
        assert!(import_bytes(b"body,BODY,abbreviation\n")
            .unwrap_err()
            .0
            .contains("more than once"));
        let i = import_bytes(b" BODY , Abbreviation ,Notes\nhello,hi,x\n").unwrap();
        assert_eq!(snippets(&i)[0].abbreviation, "hi");
        assert_eq!(i.report.ignored_columns, ["Notes"]);
    }

    #[test]
    fn invalid_and_duplicate_rows_are_reported_with_lines() {
        let csv = format!(
            "abbreviation,body\n\
             ok,fine\n\
             ,no abbreviation\n\
             {},too long\n\
             OK,duplicate of line 2\n\
             a,b,extra\n\
             \n\
             last,\"multi\nline\"\n\
             after,x\n",
            "a".repeat(MAX_ABBREVIATION_CHARS + 1)
        );
        let i = import_bytes(csv.as_bytes()).unwrap();
        assert_eq!(i.report.imported, 3);
        let lines: Vec<usize> = i.report.skipped.iter().map(|s| s.line).collect();
        assert_eq!(lines, [3, 4, 5, 6]);
        assert!(i.report.skipped[2].message.contains("line 2 is kept"));
        assert!(i.report.skipped[3].message.contains("3 fields"));
        assert_eq!(snippets(&i)[2].abbreviation, "after");
    }

    #[test]
    fn unsupported_tokens_stay_literal_and_are_warned() {
        let csv = "abbreviation,body\nd1,{{date}} and {{fill:name}}\nd2,%clipboard 100%\nd3,\nd4,{{date:%d/%m/%Y}} at 50%\n";
        let i = import_bytes(csv.as_bytes()).unwrap();
        let s = snippets(&i);
        assert_eq!(s[0].body, "{{date}} and {{fill:name}}", "stored verbatim");
        assert_eq!(s[1].body, "%clipboard 100%");
        let w: Vec<&str> = i
            .report
            .warnings
            .iter()
            .map(|w| w.message.as_str())
            .collect();
        assert!(w
            .iter()
            .any(|m| m.contains("d1") && m.contains("fill:name")));
        assert!(w.iter().any(|m| m.contains("%clipboard")));
        assert!(w.iter().any(|m| m.contains("empty body")));
        assert!(
            !w.iter().any(|m| m.contains("{{date}}")),
            "supported tokens are fine"
        );
        assert!(
            !w.iter().any(|m| m.contains("d4")),
            "% codes inside a supported placeholder are not foreign"
        );
        assert_eq!(s[3].body, "{{date:%d/%m/%Y}} at 50%");
    }

    #[test]
    fn malformed_and_unbounded_input_fails_closed() {
        assert!(import_bytes(b"abbreviation,body\nx,\"open\n")
            .unwrap_err()
            .0
            .contains("never closed"));
        assert!(import_bytes(b"abbreviation,body\nx,\"a\"b\n")
            .unwrap_err()
            .0
            .contains("closing quote"));
        assert!(import_bytes(b"abbreviation,body\nx,\xff\n")
            .unwrap_err()
            .0
            .contains("UTF-8"));
        let wide = format!("abbreviation,body{}\n", ",x".repeat(MAX_COLUMNS));
        assert!(import_bytes(wide.as_bytes())
            .unwrap_err()
            .0
            .contains("columns"));
        let long = format!(
            "abbreviation,body\nx,\"{}\"\n",
            "y".repeat(MAX_FIELD_CHARS + 1)
        );
        assert!(import_bytes(long.as_bytes())
            .unwrap_err()
            .0
            .contains("longer than"));
        let mut many = String::from("abbreviation,body\n");
        for n in 0..=MAX_SNIPPETS {
            many.push_str(&format!("a{n},b\n"));
        }
        assert!(import_bytes(many.as_bytes())
            .unwrap_err()
            .0
            .contains("more than"));
        let big = vec![b'a'; MAX_FILE_BYTES as usize + 1];
        assert!(import_bytes(&big).unwrap_err().0.contains("limit"));
    }

    #[test]
    fn line_numbers_follow_multiline_records_and_cr_endings() {
        let csv = "abbreviation,body\r\"a\",\"x\r\ny\"\r,\r";
        let i = import_bytes(csv.as_bytes()).unwrap();
        assert_eq!(snippets(&i)[0].body, "x\ny");
        assert_eq!(i.report.skipped[0].line, 4);
    }

    #[test]
    fn non_regular_files_are_refused_without_blocking() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("snippets.csv");
        assert!(std::process::Command::new("/usr/bin/mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success());
        let (tx, rx) = std::sync::mpsc::channel();
        let path = fifo.clone();
        std::thread::spawn(move || {
            let _ = tx.send(import_file(&path).is_err());
        });
        assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(5)), Ok(true));
        assert!(import_file(dir.path()).is_err(), "a folder is refused");
    }

    #[test]
    fn header_only_or_all_invalid_files_import_nothing() {
        let i = import_bytes(
            b"abbreviation,body
",
        )
        .unwrap();
        assert_eq!((i.report.rows, i.report.imported), (0, 0));
        assert!(i.library.groups.is_empty());
        let i = import_bytes(
            b"abbreviation,body
,x
,y
",
        )
        .unwrap();
        assert_eq!(
            (i.report.rows, i.report.imported, i.report.skipped_count),
            (2, 0, 2)
        );
        assert!(i.library.groups.is_empty());
    }

    #[test]
    fn conflicts_with_existing_library_are_warned() {
        let mut i = import_bytes(b"abbreviation,body\nHLO,x\nnew,y\n").unwrap();
        note_existing_conflicts(&mut i, &Library::demo());
        assert_eq!(i.report.warning_count, 1);
        assert!(i.report.warnings[0].message.contains("HLO"));
    }
}

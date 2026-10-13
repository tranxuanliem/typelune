# Importing snippets

Typelune imports two formats. Nothing in your library changes until you confirm, and both
Merge and Replace can be undone (Edit › Undo).

- **Typelune JSON** (`File › Import Library…`, `.json`): a library exported by Typelune.
  Unchanged since v0.3.
- **CSV** (`.csv`): a spreadsheet-style table, described below.

Other text expanders' own file formats (for example `.atext` or `.textexpander`) are **not**
supported. Export from those apps to CSV first, if they offer it.

## CSV format

- **Encoding:** UTF-8, with or without a byte-order mark. Other encodings are refused with a
  message; save the file as "CSV UTF-8" and try again.
- **Structure:** comma-separated, RFC 4180 style. A field may be wrapped in double quotes;
  inside quotes, `""` is one literal quote, and commas and line breaks are kept. CRLF, LF
  and CR line endings are all accepted; line breaks inside a body become plain line breaks.
- **Header row (required):** column names are matched case-insensitively, ignoring
  surrounding spaces.

| Column | Required | Becomes |
|---|---|---|
| `abbreviation` | yes | The abbreviation. Surrounding spaces are trimmed. |
| `body` | yes | The text, exactly as written. |
| `title` | no | The title. |
| `group` | no | The group; rows with the same group name share one group. Empty means "Imported". |
| `tags` | no | Tags separated by commas or semicolons. |

Other columns are ignored and listed in the preview.

Imported snippets use the library's default trigger, are enabled and are not
case-sensitive. Your library settings are kept, even when you choose **Replace**.

## Preview and feedback

The preview lists, with line numbers:

- **Skipped rows:** wrong number of fields, empty abbreviation, abbreviation longer than 48
  characters or containing a line break, a title, group or tag that is too long, more than
  32 tags, a body longer than 100,000 characters, or an abbreviation that already appeared
  earlier in the file (compared case-insensitively; the first row is kept).
- **Warnings:** empty bodies, placeholders Typelune does not support (for example
  `{{fill:name}}`), `%`-style codes from other tools (for example `%clipboard`), and
  abbreviations that your library already uses (merging would create a duplicate, which
  the editor then flags). Supported placeholders such as `{{date:%d/%m/%Y}}` are not
  warned about.

Unsupported placeholders and codes are imported as **literal text**: they are typed exactly
as written. Imported content is never executed.

If no row can be imported, Merge and Replace are not offered.

## Limits

The file must be at most 10 MB, with at most 10,000 rows and 64 columns; a single field may
hold at most 100,000 characters. A file that breaks the CSV rules (for example a quote that
is never closed) is refused as a whole, with the line number.

## Export

Typelune exports JSON only. CSV export is not part of this release.

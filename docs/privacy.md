# Privacy and permissions

Typelune works entirely on your Mac. It makes no network connections and has no
analytics.

## Your library

- **Location:** `~/Library/Application Support/io.github.tranxuanliem.typelune/library.json`.
  To use another folder (for example, to test), set `TYPELUNE_DATA_DIR`.
- **Saving:**
  - Writes are atomic.
  - The previous version is kept as `library.json.bak`.
  - A corrupt file is moved aside, never deleted.
- **Preferences** (excluded apps, the Quick Search shortcut, start at launch) are stored in
  `preferences.json` in the same folder. They contain no snippets and nothing you typed.

## Backups

- **Daily:** before the first change of each day, the previous version of the library is
  saved as `backups/library-YYYYMMDD-daily.json`. At most one per day.
- **Manual:** *Backup Now* in the Backups window.
- **Before restore:** restoring a backup first saves the current library as a backup. If
  that fails, nothing is restored. Edit › Undo in the Backups window also undoes a restore.
- **Retention:** the newest 30 backups are kept and older ones deleted. Today's daily
  backup is never the one deleted.
- Backups are complete, validated libraries written with exclusive temporary files. They
  are never overwritten. Links, folders, pipes, damaged and oversized files are refused,
  never followed or restored. Backups stay on your Mac.
- **Upgrading from the pre-0.3 prototype:** its library is **copied** once into the folder
  above. The original is only read; it is never moved, changed or deleted. A small
  `migrated-from.json` note records the source path, size and SHA-256 checksum. It does
  not contain your snippets.

## System-wide expansion

**Off by default.** System-wide expansion is off when Typelune starts, unless you turned on
*Start system-wide expansion when Typelune opens* in Settings › General (off by default).
Even then it starts only if Accessibility permission is already granted; otherwise
Typelune says it did not start. Closing the window does not stop it: Typelune keeps running
in the menu bar until you press **Stop**, **Pause** or quit.

**Login item.** *Open Typelune at login* is off by default and uses macOS's own login-item
service (`SMAppService`). macOS may ask you to approve it in System Settings › General ›
Login Items; Typelune shows that status. Runs with a separate data folder
(`TYPELUNE_DATA_DIR`) cannot register a login item.

**Permission.** It needs **Accessibility** permission, which you grant yourself in System
Settings → Privacy & Security → Accessibility. Typelune never changes privacy settings and
never asks programmatically. The *Open Accessibility Settings…* button only opens that
page.

**While it runs:**
- Typed characters are kept only in a 64-character in-memory buffer used for matching. It is
  cleared when you switch apps and while Quick Search is open. Nothing you type is logged,
  saved or sent anywhere.
- The *System-wide Expansion* window shows session-only information. None of it is saved
  or sent anywhere, and none of it is typed text:
  - counters of event categories (keys seen, resets, matches, expansions, aborts);
  - the last expanded snippet's *label* and time;
  - the last abort or cancellation reason, which may include process IDs;
  - template warnings.

  Typed text is never logged.
- Expansion pauses while macOS **Secure Keyboard Entry** is active. Not every app turns
  that on for its password fields, so not every password field is detected. Use
  **excluded apps** (Settings › Exclusions) for apps where that matters: in them, typing is
  not recorded or matched and nothing is inserted. Apps are identified by bundle
  identifier; a process that is not inside an app bundle cannot be excluded. When
  exclusions are set and the receiving app of a key cannot be identified, the key is
  ignored.
- Typing in Typelune's own windows is ignored. The Playground has its own matcher.

**Before any text is replaced**, and again before every synthetic key, Typelune checks that:
- the focused app and text field are still the ones you typed in;
- no newer real input has arrived;
- expansion is not paused or stopped;
- Secure Keyboard Entry is not active;
- the app is not excluded.

If a check fails, nothing more is injected. Injected keys go only to the verified app.

## Quick Search

The global shortcut (default ⌃⌥⌘T, changeable or removable in Settings › General) opens
a small search panel over the app you are using. It needs no permission: macOS delivers
only that one key combination to Typelune.

- **Copy** puts the expanded text on the clipboard. It always works, even when
  system-wide expansion is off.
- **Insert** types the snippet into the field you came from. It needs system-wide
  expansion to be running (not paused) and Accessibility permission. Before the panel
  opens, Typelune notes the app and the exact text field (read-only). After the panel
  closes it waits briefly for that same field to be focused again and then applies all the
  checks above. If the app does not tell macOS which field is focused, Insert is not
  offered (use Copy). Nothing you typed is erased, and the clipboard is not used or
  changed by Insert.
- While the panel is open, Typelune is the active app so your typing goes to the search
  field, and typing never expands abbreviations. Escape, Copy and Insert give focus back to
  the app you came from (unless you switched to another app meanwhile). If macOS does not
  let Typelune take the keyboard, the panel says so; click the search field before typing.
- Disabled snippets are listed (marked *Disabled*) and can still be copied or inserted on
  purpose; disabling a snippet only stops its abbreviation from expanding.

## Builds

Release builds are **ad-hoc signed and not notarized**. macOS will warn before opening
one; that warning is accurate. If you prefer, build from source.

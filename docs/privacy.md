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
- **Upgrading from the pre-0.3 prototype:** its library is **copied** once into the folder
  above. The original is only read; it is never moved, changed or deleted. A small
  `migrated-from.json` note records the source path, size and SHA-256 checksum. It does
  not contain your snippets.

## System-wide expansion

**Off by default.** System-wide expansion is off every time Typelune starts. It runs only
after you press **Start** in *System-wide Expansion*.

**Permission.** It needs **Accessibility** permission, which you grant yourself in System
Settings → Privacy & Security → Accessibility. Typelune never changes privacy settings and
never asks programmatically. The *Open Accessibility Settings…* button only opens that
page.

**While it runs:**
- Typed characters are kept only in a 64-character in-memory buffer used for matching.
  Nothing you type is logged, saved or sent anywhere.
- The *System-wide Expansion* window shows session-only information. None of it is saved
  or sent anywhere, and none of it is typed text:
  - counters of event categories (keys seen, resets, matches, expansions, aborts);
  - the last expanded snippet's *label* and time;
  - the last abort or cancellation reason, which may include process IDs;
  - template warnings.

  Typed text is never logged.
- Expansion pauses while macOS **Secure Keyboard Entry** is active. Not every app turns
  that on for its password fields, so not every password field is detected.
- Typing in Typelune's own windows is ignored. The Playground has its own matcher.

**Before any text is replaced**, Typelune checks that:
- the focused app and text field are still the ones you typed in;
- no newer real input has arrived;
- expansion is not paused or stopped;
- Secure Keyboard Entry is not active.

If a check fails, nothing is injected. Injected keys go only to the verified app.

## Builds

Release builds are **ad-hoc signed and not notarized**. macOS will warn before opening
one; that warning is accurate. If you prefer, build from source.

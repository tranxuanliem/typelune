# Architecture

```
Typelune.app/Contents/MacOS/Typelune  (SwiftUI, macos/Sources)
        │  C ABI: include/typelune_core.h  (JSON request → JSON reply, tpl_* symbols)
        ▼
libtypelune_core.a  (Rust, src/)
  engine.rs    matching (triggers, boundaries, longest match, Unicode)
  template.rs  placeholders
  store.rs     JSON persistence, validation, import/export, data-folder resolution
  backup.rs    dated snapshots (daily, manual, before restore) with retention
  prefs.rs     preferences.json in the data folder (exclusions, shortcut, start at launch)
  csv_import.rs  bounded CSV parser and import report
  migrate.rs   one-time copy of the pre-0.3 library
  saver.rs     background save worker
  native/      opt-in system-wide expansion (event tap, injector, safety checks)
  ffi.rs       the C ABI
```

## Responsibilities

- **Rust** owns matching, rendering, validation, persistence, import/export, migration and
  system-wide expansion.
- **SwiftUI/AppKit** owns the interface:
  - a three-column `NavigationSplitView` with groups, a searchable list and an editor;
  - toolbar, menus and keyboard shortcuts;
  - a `MenuBarExtra` menu; closing the last window keeps the app running there
    (`applicationShouldTerminateAfterLastWindowClosed` is `false`); only Quit stops it;
  - Settings, Playground, System-wide and Backups windows;
  - the Quick Search panel (an `NSPanel` that activates Typelune while open) and its global shortcut (Carbon
    `RegisterEventHotKey`, exclusive registration);
  - launch at login through `SMAppService.mainApp`;
  - standard Open/Save panels.
- The legacy egui UI (`--features legacy-egui`) is a developer fallback. It requires an
  explicit, isolated `TYPELUNE_DATA_DIR` and refuses the real library folders.

## C ABI

- **Functions:** `tpl_abi_version`, `tpl_core_open`, `tpl_core_call`, `tpl_core_close`,
  `tpl_string_free`. The ABI version is 2 and lives in `Cargo.toml` and `src/ffi.rs`.
  v0.4 only adds JSON commands (preferences, backups, CSV, Quick Search), so the ABI
  version is unchanged.
- **Errors:** every entry point catches panics and returns errors as JSON. NULL handles
  and strings, invalid UTF-8 and malformed JSON are reported as errors.
- **Pointer validity:** non-NULL arguments must be valid. A handle must be live (opened and
  not yet closed), and a string must be a NUL-terminated buffer the caller owns. A dangling
  or arbitrary non-NULL pointer is undefined behaviour; the core cannot detect it.
- **Ownership:** the caller owns the handle and must close it once, and must free each
  reply string once.
- **Threading:** calls may come from any thread. `native_start` and
  `native_refresh_layout` must run on the main thread (Carbon TextInputSources), and the
  core checks this.
- **Swift side:** `TypeluneCore` holds a readers/writer *lifetime lock* across every call,
  so `close` never frees the core while a call is in flight.

## Background features

**Preferences** live in `preferences.json` next to the library, owned by Rust. A run with
an explicit data folder (`TYPELUNE_DATA_DIR`, tests) therefore has its own preferences
and never changes the real ones. Such a run is reported as *isolated*, and the app then
refuses to register a login item. Missing file: defaults, nothing written until a change.
Corrupt file: defaults, moved aside on the next change. Symbolic link or newer version:
read-only for the session.

**Start at launch** is a remembered opt-in (default off). It starts expansion only if
Accessibility is already granted; otherwise it reports why it did not start.

**App exclusions** (bundle identifiers) are checked twice:
- in the event tap, before a key is recorded or matched: the receiving app is the key
  event's target PID, else the owner of the system-wide Accessibility focused element.
  If the receiving app is unknown, the key is always skipped and matching is reset. Switching
  apps resets matching;
- in the injector, right before every synthetic key (see below).

The bundle identifiers of a process come from its executable path (`proc_pidpath`) and the
enclosing `.app` bundles (CoreFoundation `CFBundle`), cached per PID and re-validated by
path.

Keyboard focus is resolved from the system-wide `AXFocusedUIElement` and its
`AXUIElementGetPid` owner, independently of the active application. This handles
nonactivating panels such as Raycast. If the system exposes no focused element, the
fallback is `AXFocusedApplication` and that application's `AXFocusedUIElement`; an
exposed fallback element must belong to that application. Only unsupported/no-value
attribute responses permit fallback; IPC timeouts and other query errors abort capture.
Unreadable or inconsistent element ownership fails closed. An app-only identity remains possible when no element
is exposed; Quick Search still requires an exact element. An explicit key-event target
must still match the captured focus owner, and focus is re-read before every posted event.
No event-target PID is used to manufacture an Accessibility focus identity. The event
tap uses only one focused-element query without application fallback. AX messaging has
a 5 ms timeout (also set explicitly on fallback application objects); injector queries
use the same short timeout because the system-object setting is process-wide. This
bounds AX IPC waiting but does not establish a measured end-to-end tap latency guarantee.
An out-of-range explicit event target skips matching even with no exclusions configured.

**Quick Search insertion** reuses the injector:
1. Before the panel opens, automatic matching is suspended (inhibited) so search typing
   can never expand anything, and the focused app and text element are captured through
   Accessibility (read-only). An app that does not expose its focused element is
   Copy-only.
2. Typelune then activates (`NSApp.activate()`) and the panel becomes key, re-keyed when
   `didBecomeActive` arrives. An inactive app's panel is not guaranteed to receive
   keyboard input (v0.4 builds before this change used a non-activating panel, and typing
   could reach the app underneath). Activation is a request macOS may refuse or delay;
   until the panel really has the keyboard it shows "Typelune does not have keyboard focus
   yet", and inhibition keeps any stray keys from expanding.
3. Focus returns to the original app (`yieldActivation(to:)` + `activate()`) after Escape,
   the close button, the shortcut, Copy or Insert, but only while Typelune is still
   frontmost: never over an app the user clicked meanwhile. Clicking away ends the
   session without moving focus.
4. On Insert, the panel hides, focus is handed back as above, inhibition is released and
   Rust checks (running, not paused, fresh token, exact field known, not
   Typelune, not excluded, no Secure Keyboard Entry). Then the injector waits (bounded,
   1.5 s) for the exact captured app *and* element to be focused again and types the
   rendered snippet. Nothing is erased and the clipboard is not used.

**Injection granularity.** All gates (stop, pause, newer input, Secure Keyboard Entry,
exclusion, and the focused app *and* element) are re-verified immediately before each
synthetic key event: every Backspace, Return and Left arrow, and every text chunk of up to
20 UTF-16 units. A single event cannot be checked part-way.

**Backups** are written by Rust into `backups/` in the data folder; see
[privacy.md](privacy.md#backups).

## Concurrency

There is no async runtime, by design. The work is one blocking file write plus OS run-loop
work that must stay on dedicated threads.

- **Saves.** A single save thread with one pending slot; the newest snapshot wins.
  - The UI never waits on disk I/O.
  - Edits are refused if the thread cannot start, after shutdown, or when the library
    could not be opened or migrated.
- **Injection queue.** Bounded (`sync_channel(1)` with `try_send`), so the event-tap
  callback never blocks.
- **Stop.** Shutdown is signalled immediately; the threads are then joined (up to 2 s)
  without holding the core lock. The Swift app runs Stop off the main actor and shows
  *Stopping…*.

## Data and compatibility

- **Schema:** v1, unchanged in v0.4. The on-disk format literal stays `atext-rs.library`
  for compatibility, and `typelune.library` is also accepted. Renaming the literal is
  reserved for a v2 schema. New v0.4 data (exclusions, shortcut, start at launch) is in
  `preferences.json`, not in the library, so v0.3 reads v0.4 libraries unchanged.
- **Identifiers:** IDs are UUIDs. Swift keeps them as strings, so the identity is kept
  exactly. Rust writes UUIDs in canonical lowercase form, so an ID written with different
  casing by another tool may change case on the next save.
- **No rewrite on open:** opening a valid existing library never rewrites it. The recovery
  path (a corrupt file is moved aside and demo data written) and first-run demo creation do
  write files.
- **Data-folder resolution:**
  1. `TYPELUNE_DATA_DIR`;
  2. `ATEXT_RS_DATA_DIR` (deprecated);
  3. the default folder. Only this one runs the one-time copy from
     `Application Support/dev.atext-rs.poc/`.

## Identity

`Cargo.toml` is the single source for the version and for
`[package.metadata.typelune]`: name, bundle ID, executable, minimum macOS, copyright,
data-folder variables, C header, prefix and ABI version. `scripts/build-app.sh` generates
`Info.plist` from it. `scripts/check-identity.sh` fails on any drift between it and the
plist, signature, binary, store constants, Swift type names, header, ABI or docs. It also
scans publishable files for personal or machine data.

## Toolchain

`scripts/swift-env.sh` uses `$SWIFT_SDK` if it is set. Otherwise it picks the default macOS
SDK, provided that SDK can compile SwiftUI's `@State`. Some newer SDKs implement `@State`
as a macro whose compiler plugin ships only with Xcode. With Command Line Tools only, the
script falls back to an installed MacOSX26.x SDK. The minimum macOS is 15.0. Builds remap
the home and checkout paths so binaries don't embed them.

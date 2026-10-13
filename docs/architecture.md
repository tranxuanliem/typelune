# Architecture

```
Typelune.app/Contents/MacOS/Typelune  (SwiftUI, macos/Sources)
        │  C ABI: include/typelune_core.h  (JSON request → JSON reply, tpl_* symbols)
        ▼
libtypelune_core.a  (Rust, src/)
  engine.rs    matching (triggers, boundaries, longest match, Unicode)
  template.rs  placeholders
  store.rs     JSON persistence, validation, import/export, data-folder resolution
  migrate.rs   one-time copy of the pre-0.3 library
  saver.rs     background save worker
  native/      opt-in system-wide expansion (event tap, injector, safety checks)
  ffi.rs       the C ABI
```

## Responsibilities

- **Rust** owns matching, rendering, validation, persistence, import/export, migration and
  system-wide expansion.
- **SwiftUI** owns the interface:
  - a three-column `NavigationSplitView` with groups, a searchable list and an editor;
  - toolbar, menus and keyboard shortcuts;
  - Settings, Playground and System-wide windows;
  - standard Open/Save panels.
- The legacy egui UI (`--features legacy-egui`) is a developer fallback. It requires an
  explicit, isolated `TYPELUNE_DATA_DIR` and refuses the real library folders.

## C ABI

- **Functions:** `tpl_abi_version`, `tpl_core_open`, `tpl_core_call`, `tpl_core_close`,
  `tpl_string_free`. The ABI version is 2 and lives in `Cargo.toml` and `src/ffi.rs`.
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

- **Schema:** v1. The on-disk format literal stays `atext-rs.library` for compatibility,
  and `typelune.library` is also accepted. Renaming the literal is reserved for a v2
  schema.
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

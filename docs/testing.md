# Testing

Run everything with `scripts/test-all.sh`. CI runs the same script, then builds the app and
runs `scripts/check-identity.sh --app dist/Typelune.app`.

The tests use temporary folders only. They never start system-wide expansion, never post
keyboard events, and never read or write a real library.

## What is covered

**Rust unit tests (`cargo test`)**
- Engine:
  - triggers, word boundaries, longest match and tie-breaks;
  - case rules and disabled snippets;
  - Unicode, including decomposed Vietnamese.
- Templates and placeholders.
- Store:
  - validation and atomic writes;
  - concurrent saves and exports leave complete files and no temp files;
  - corrupt files are moved aside without overwriting earlier backups;
  - a symlinked library fails closed;
  - the egui fallback-folder rules.
- Migration:
  - byte-for-byte copy that is never repeated;
  - never overwrites, including a dangling symlink at the target;
  - eight threads racing in one process copy exactly once;
  - other operations' temp files are never removed;
  - invalid, oversized and unreadable sources, and an unwritable target, all fail closed.
- Save worker: bounded, coalescing, fails closed.
- Native pure logic:
  - key classification and the injection plan;
  - safety checks;
  - own-event and own-app filters.
- Core open:
  - migration;
  - fail-closed cases (no demo data, edits refused).

**End-to-end tests** (`tests/end_to_end.rs`): the demo library goes through the store and
the engine into a simulated text field.

**C ABI tests** (`tests/ffi_bridge.rs`):
- null and invalid input;
- schema round-trip with no rewrite on open;
- import, merge and replace;
- the protected export path;
- main-thread guards;
- stop.

**Keyboard layout** (`tests/keyboard_layout.rs`): keycode translation on the main thread.

**Swift bridge tests** (`macos/Tests/main.swift`):
- lossless Codable round-trip against the file on disk;
- Playground math and typing through the Rust engine;
- selection rules;
- calls racing `close()` (lifetime barrier).

**Static checks**
- `cargo fmt`, and `clippy -D warnings` with and without `legacy-egui`.
- `swift format lint --strict`.
- The Swift app type-checks with warnings as errors.
- `check-identity.sh --self-test` plants problems to prove the detectors work; the normal
  run checks the source tree and the built bundle.

## Not covered by automated tests

These need a person at the Mac:
- the visible UI;
- file panels;
- system-wide expansion in real apps.

System-wide expansion was checked by hand in Apple Notes with a physical keyboard during
development. Re-check it after any change to `src/native/`.

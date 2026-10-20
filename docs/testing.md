# Testing

Run everything with `scripts/test-all.sh`. CI runs the same script, then builds the app and
runs `scripts/check-identity.sh --app dist/Typelune.app`.

The tests use temporary folders only. They never start system-wide expansion, never post
keyboard events, never register login items or global shortcuts, never change permissions,
and never read or write a real library, real preferences or real backups.

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
- Backups:
  - strict snapshot names; at most one daily snapshot per day, even with 8 racing writers;
  - today's daily snapshot survives a burst of 35 manual snapshots, and the total never
    exceeds 30;
  - invalid, oversized, damaged, folder, named-pipe and symlinked snapshots (and a
    symlinked backups folder) are refused, never followed, deleted or replaced;
  - same-second snapshots never overwrite each other; no temp files are left.
- Preferences: safe defaults (no auto-start), nothing written on read, round trip,
  validation, corrupt files moved aside, symlinks and newer versions never overwritten.
- CSV: BOM, quoted commas, line breaks and escaped quotes, CRLF/CR, header rules, line
  numbers for skipped and duplicate rows, unsupported placeholders and `%` codes kept
  literal and warned (but not `%` inside supported placeholders), header-only and
  all-invalid files, size/row/column/field limits, unterminated quotes, invalid UTF-8,
  named pipes and folders refused without blocking.
- Reading files: named pipes are refused without blocking (library and import).
- Native pure logic:
  - key classification and the injection plan;
  - safety checks, including exclusion (which wins over a matching focus);
  - the automatic-matching decision: Quick Search typing never feeds the matcher, unknown
    receiving apps always cancel pending work and reset matching, regardless of exclusions;
  - Quick Search refusals: off, stopping, paused, missing or stale target, Typelune
    itself, field not identified exactly, excluded app, Secure Keyboard Entry; a manual
    plan never erases text;
  - enclosing app bundles and case-insensitive exclusion matching;
  - a focus change in the middle of a multi-chunk insertion stops before the next event;
  - own-event and own-app filters;
  - pure macOS focus-owner selection: panel element ownership, consistent application
    fallback, missing/invalid element PIDs, rejection of mismatched fallback ownership,
    and AX timeout/error responses distinguished from absent attributes;
  - pure receiving-process selection: explicit event targets take precedence, missing
    targets use AX focus, and out-of-range targets fail closed.
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
- stop;
- preferences in the data folder (isolated, safe defaults, validation, persistence);
- Backup Now, preview, restore with a pre-restore snapshot, refused names and damaged
  snapshots;
- CSV preview leaves the library unchanged until merge;
- Quick Search insertion refused while off; inhibition does not change the run state.

**Keyboard layout** (`tests/keyboard_layout.rs`): keycode translation on the main thread.

**Swift bridge tests** (`macos/Tests/main.swift`):
- lossless Codable round-trip against the file on disk;
- Playground math and typing through the Rust engine;
- selection rules;
- calls racing `close()` (lifetime barrier);
- preferences isolation and a cleared shortcut staying cleared;
- v0.3 libraries stay byte-identical when preferences and backups are used;
- backups, restore and CSV preview through the bridge;
- Quick Search ranking and keyboard selection, Insert-vs-Copy availability;
- shortcut validation, start-at-launch and login-item rules.

**Static checks**
- `cargo fmt`, and `clippy -D warnings` with and without `legacy-egui`.
- `swift format lint --strict`.
- The Swift app type-checks with warnings as errors.
- `check-identity.sh --self-test` plants problems to prove the detectors work; the normal
  run checks the source tree and the built bundle.

## Not covered by automated tests

These need a person at the Mac:
- the visible UI, including the menu bar menu and keeping running after the window closes;
- file panels and the installed-app picker;
- system-wide expansion in real apps, exclusions in a real app, and Quick Search Insert;
- the global shortcut (registration, conflicts, use from another app);
- launch at login (`SMAppService` behaviour depends on where the app is installed and
  on macOS approval);
- Undo of a restore in the Backups window.

System-wide expansion in v0.2.0 was checked by hand in Apple Notes with a physical keyboard. The v0.3.0 naming update keeps the injection behavior, but its new bundle identity needs a fresh Accessibility grant before repeating that test. Re-check after any change to `src/native/`.

## v0.3.0 local validation

- Rust: 99 unit tests, 6 end-to-end tests, 11 C ABI tests, and 3 keyboard-layout checks passed.
- Swift: all 42 bridge checks passed, plus strict formatting and warnings-as-errors typechecking.
- Source identity, detector self-test, bundle identity and ad-hoc signature verification passed.
- A build from a source folder containing spaces passed without embedded personal home paths.
- Visible UI: About shows Typelune 0.3.0 and the new icon; Playground expands the English and Vietnamese example rules.
- First-launch migration was checked byte for byte against the original file, which remained unchanged. System-wide expansion remained off.

## v0.4.0 local validation

Automated, on the final build:
- Rust: 132 unit tests, 6 end-to-end tests, 15 C ABI tests, and 3 keyboard-layout checks passed.
- Swift: all 125 bridge checks passed, plus strict formatting and warnings-as-errors typechecking.
- `cargo fmt`, clippy (with and without `legacy-egui`), the identity self-test, source and bundle
  identity, and ad-hoc signature verification passed.
- The Quick Search focus rules (when to hand focus back, what counts as a click-away) are
  covered by pure policy tests. These test the decisions only, not real AppKit activation or
  keyboard routing.

Observed in the GUI by the reviewer (synthetic input, isolated `TYPELUNE_DATA_DIR`,
synthetic data only):
- CSV: the preview wrote nothing; a UTF-8 file with BOM, CRLF, multi-line Vietnamese text and
  escaped quotes imported correctly; Merge, Undo and Redo worked.
- Backups: Backup Now, the daily snapshot before the first save of the day, restore with the
  pre-restore snapshot kept, and Undo of the restore from the Backups window.
- Exclusions: adding an app with the installed-app picker; the exclusion survived a relaunch.
- Quick Search shortcut setting: Clear (stored as no shortcut), Record (a new combination was
  accepted) and Default.
- Quick Search opened from the menu: typing filtered, arrow keys moved the selection, Escape
  closed it; Copy put the exact multi-line Vietnamese text on the clipboard.
- Playground expansion, and Quit from the menu exits the app.
- The real library remained byte-identical throughout.

CI compatibility fix after the first push: GitHub CI (Xcode 16.4, macOS 15.5 SDK) refused
the Undo handler because it captured `UndoManager`, which that SDK does not mark Sendable,
in a `@Sendable` closure. The handler now captures a main-actor-isolated weak reference to
the originating window's `UndoManager`, so Redo still goes on the same manager and the
manager no longer retains itself through its own handler. Locally (macOS 26.5 SDK) all
checks above pass with this change. The macOS 15.5 SDK is not installed here; GitHub CI
checks compatibility with that SDK. The reviewer repeated restore, Undo and Redo in the
Backups window after this fix: five QA snippets became two, Undo restored five, Redo
restored two, and a final Undo restored five. Each result was checked in the saved QA file.

Synthetic GUI typing is not the same as physical keys or a global shortcut pressed in another
app; those are listed separately below.

Confirmed by a person with a physical keyboard, on the activating-panel build, with Accessibility granted
to that exact build and all Typelune windows closed:
- With expansion running, an abbreviation followed by Space expanded in Apple Notes.
- The global shortcut (⌃⌥⌘T) pressed in Notes opened Quick Search. Typing reached the search
  field, and Return inserted the multi-line Vietnamese snippet into Notes. The reviewer
  separately checked the native statistics afterwards: running, three expansions, the last
  Quick Search insert recorded as successful, no aborts. The final Notes text is as reported
  by the person; Notes was cleared before a screenshot was taken.
- In TextEdit, which was excluded: the shortcut opened Quick Search and typing reached
  the search field; Return did not insert
  (Copy-only, as intended for an excluded app). Pasting afterwards produced the exact
  multi-line text with Vietnamese diacritics and quotation marks.

The subsequent SDK compatibility change affected only the Undo reference; the Quick Search
and native injection code used by these physical tests is unchanged.

On the earlier build, a person also verified that an abbreviation followed by Space stayed
literal in excluded TextEdit. The exclusion and automatic-matching code did not change in
the panel activation fix; that particular physical test was not repeated on the final build.

Focus failure found and fixed during this validation: the first v0.4 build used a
non-activating panel. With all Typelune windows closed, the shortcut opened the panel from
TextEdit but typing still went to TextEdit, because an inactive app's panel is not guaranteed
the keyboard. The panel now activates Typelune while it is open and hands focus back to the
original app afterwards (see architecture.md). The physical results above are on that build.

Not verified:
- registering the real login item with macOS;
- input-method composition (marked text) in the Quick Search field;
- returning focus to the original app after Escape or Copy, and click-away, with real
  activation (only the policy tests above);
- staying active in the background over a longer session.

### Nonactivating panel compatibility

The focus-owner and receiving-process regressions use pure values, without reading live
fields or posting events. They do not prove that a particular Raycast release exposes
its query field through Accessibility. Real Raycast verification remains manual: with
expansion explicitly enabled in a controlled session, check expansion in its query field
while another app remains active, exclusions for Raycast, focus changes during insertion,
and Secure Keyboard Entry refusal. Also check ordinary app expansion and Quick Search's
exact-field return requirement. Do not run this manual procedure as part of automated tests.

Unknown-target automatic jobs are rejected before Accessibility queries or event work.
The regression tests construct controller state only and never start the tap. The 5 ms AX
timeout remains unmeasured in real insertion flows; a timeout after insertion begins can
leave a partial replacement. This correction does not change that latency design.

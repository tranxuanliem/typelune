# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/) (0.x: anything may change).

## [Unreleased]

## [0.3.0] - 2026-10-13

First public developer preview as **Typelune**.

### Added
- New name and brand: original icon (with hand-tuned small sizes), wordmark and palette.
  See `docs/brand.md`.
- Bundle identifier `io.github.tranxuanliem.typelune` and data folder
  `~/Library/Application Support/io.github.tranxuanliem.typelune/`.
- One-time copy of the pre-0.3 prototype library:
  - byte for byte, verified after copying;
  - never moves or changes the original, and never overwrites;
  - unique temp file plus atomic no-clobber link;
  - fails closed when the original is unreadable or invalid.
- Product metadata kept in one place (`Cargo.toml`), with `scripts/check-identity.sh`
  covering metadata, signature, minimum macOS, private-data scan and legacy-name allowlist.
- SDK discovery that builds with Xcode or with Command Line Tools only.
- Docs, plus GitHub CI and a release pipeline that publishes ad-hoc-signed,
  not-notarized prereleases.

### Changed
- Crate renamed to `typelune-core` (`libtypelune_core.a`).
- C ABI renamed to `tpl_*` (ABI version 2, header `include/typelune_core.h`).
- Swift types renamed to `TypeluneApp` / `TypeluneCore`; the executable and app are now
  `Typelune`.
- The data-folder variable is now `TYPELUNE_DATA_DIR`. `ATEXT_RS_DATA_DIR` still works but
  is deprecated.
- The legacy egui interface is now an optional developer feature (`legacy-egui`).

### Compatibility
- The library schema is unchanged: v1, format literal `atext-rs.library`. Files with
  `typelune.library` are also accepted.

## [0.2.0] - 2026-10-13
### Added
- Native SwiftUI interface on a Rust core, connected through a small panic-guarded C ABI.
- Background save worker (bounded, coalescing; fails closed).
- Stop that does not block the main thread.
- Typing in the app's own windows is ignored by system-wide expansion.

## [0.1.2] - 2026-10-13
### Fixed
- System-wide injection is delivered to the verified focused app, and synthetic events
  are tagged at the source. Verified by hand in Apple Notes.

## [0.1.1] - 2026-10-13
### Added
- Diagnostic counters for system-wide expansion. They count events only; no typed content.

### Fixed
- Keycode-only key events are handled.
- Start fails explicitly when key monitoring is not permitted.
- Long key strings are no longer truncated.

## [0.1.0] - 2026-10-13
### Added
- Initial prototype:
  - expansion engine, placeholders and JSON storage;
  - Playground;
  - opt-in macOS system-wide expansion.

[Unreleased]: https://github.com/tranxuanliem/typelune/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/tranxuanliem/typelune/releases/tag/v0.3.0

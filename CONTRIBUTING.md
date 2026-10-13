# Contributing

Thanks for your interest. Typelune is a small project, so please open an issue first for
anything larger than a fix.

## Set up

1. Install:
   - Rust stable, with `rustfmt` and `clippy`;
   - Python 3.11 or later;
   - either Xcode, or the Command Line Tools with a macOS 26.x SDK.
2. `scripts/test-all.sh` runs every check CI runs.
3. `scripts/build-app.sh` builds `dist/Typelune.app`.

## Conventions

**Rust**
- Run `cargo fmt`.
- `cargo clippy --all-targets -- -D warnings` must be clean, both with and without
  `--features legacy-egui`.
- No `unsafe` without a `// SAFETY:` comment.

**Swift**
- Format with `swift format --configuration macos/.swift-format`.
- The app must type-check with warnings treated as errors.
- Use `@Observable @MainActor` models, stable identifiers, native controls, system colors
  and fonts.

**C ABI** (`src/ffi.rs`, `include/typelune_core.h`)
- Symbols start with `tpl_`.
- Requests and replies are JSON.
- Every entry point catches panics.
- Breaking changes bump `abi_version` in `Cargo.toml` and `ABI_VERSION` in `src/ffi.rs`.

**Product identity** (name, bundle ID, minimum macOS, version)
- Lives only in `Cargo.toml`.
- `scripts/check-identity.sh` enforces it.

**Dependencies**
- Adding or updating a dependency means updating `THIRD_PARTY_NOTICES.txt`; see
  `docs/releasing.md`.

**Commits**
- Clear imperative subject lines.
- One logical change per pull request.
- Update `CHANGELOG.md` under *Unreleased*.

## Tests must stay safe

Tests must never:
- start system-wide expansion;
- post keyboard events;
- touch the real library.

Use temp folders (`TYPELUNE_DATA_DIR` or the test helpers).

## Privacy

Never commit:
- personal paths, usernames or email addresses;
- logs of typed text;
- machine-specific QA notes.

`scripts/check-identity.sh` scans for common cases. Keep local notes in `private/`, which
git ignores.

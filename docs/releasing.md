# Releasing

Releases are **GitHub pre-releases** built by CI from the tagged source. They are ad-hoc
signed and **not notarized**, and the release title and notes say so.

## Steps

1. Update `version` in `Cargo.toml`.
2. Move the *Unreleased* notes in `CHANGELOG.md` into a `## [x.y.z] - YYYY-MM-DD`
   section.
3. Run `scripts/test-all.sh`, then `scripts/build-app.sh`, then
   `scripts/check-identity.sh --app dist/Typelune.app`.
4. Commit, then push a tag `vX.Y.Z` that matches the Cargo version.
5. `.github/workflows/release.yml` then:
   - checks that the tag equals `v` + the Cargo version;
   - runs every check, builds the app and checks the bundle identity;
   - zips the app with `ditto` as `Typelune-X.Y.Z-macos-arm64-unsigned.zip` and writes
     `SHA256SUMS.txt`;
   - adds a GitHub build-provenance attestation for the zip;
   - creates the pre-release with notes from `scripts/release-notes.sh`.

## Third-party notices

`THIRD_PARTY_NOTICES.txt` lists the licenses of the Rust dependencies linked into the
app, including original license texts. `scripts/build-app.sh` copies it into
`Contents/Resources/` before signing, and `check-identity.sh --app` verifies that the
bundled copy matches. Regenerate it only when the dependency graph changes (for example
with `cargo tree --locked`). It is the one file excluded from the generic email scan,
because it quotes upstream authors' public contact details.

## Rules

- **Never weaken signing or Gatekeeper in docs or notes.** No advice to strip quarantine
  attributes and no requests to disable security features. The honest options are: accept
  the macOS warning knowingly, or build from source.
- **Publishable files contain no personal or machine data.** `check-identity.sh` enforces
  this. Local-only deny patterns can go in `private/publish-deny-patterns.txt`, which is
  gitignored.
- **Builds remap the home and checkout paths**, and the identity check fails if a home path
  ends up in the binary.
- **Notarized releases** need an Apple Developer ID certificate and notarization
  credentials in CI secrets. That is planned, not done.

## Bundle identity

| Field | Value (from `Cargo.toml`) |
|---|---|
| Name / executable | Typelune |
| Bundle ID | `io.github.tranxuanliem.typelune` (permanent) |
| Minimum macOS | 15.0 |
| Data folder | `~/Library/Application Support/io.github.tranxuanliem.typelune/` |

Changing the bundle ID would be a new app to macOS: the Accessibility permission would
have to be granted again and the data folder would change. Don't change it.

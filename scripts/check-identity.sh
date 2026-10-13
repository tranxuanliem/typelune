#!/usr/bin/env bash
# Strict identity and publication checks.
#   scripts/check-identity.sh              source-tree checks
#   scripts/check-identity.sh --app PATH   also verify a built bundle against Cargo.toml
#   scripts/check-identity.sh --self-test  prove the detectors catch planted problems
# Optional, local-only extra deny patterns (one ERE per line) are read from
# private/publish-deny-patterns.txt, which is gitignored and never published.
set -euo pipefail
cd "$(dirname "$0")/.."
eval "$(python3 scripts/product.py)"
meta() { python3 -c "import tomllib,sys;print(tomllib.load(open('Cargo.toml','rb'))['package']['metadata']['typelune'][sys.argv[1]])" "$1"; }
DATA_DIR_ENV="$(meta data_dir_env)"; LEGACY_ENV="$(meta legacy_data_dir_env)"; LEGACY_ID="$(meta legacy_bundle_id)"
fail=0
err() { echo "identity: $*" >&2; fail=1; }
tmpd="$(mktemp -d)"; trap 'rm -rf "$tmpd"' EXIT

# Generic sensitive patterns (machine paths, session/QA identifiers, keys, personal email).
SENSITIVE='/Users/[A-Za-z]|/home/[A-Za-z]|\.t3/|CDHash=|ev_[0-9a-f]{16}|BEGIN (RSA |OPENSSH |EC )?PRIVATE KEY|[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+\.(com|net|org|io|dev)'
SENSITIVE_ALLOW='users\.noreply\.github\.com|@example\.(com|org|net)|/Users/example/|/Users/runner/'
deny_file=private/publish-deny-patterns.txt

# Third-party license notices legitimately contain upstream authors' public emails; that
# one file skips the generic scan (the local deny-list still applies to it).
NOTICES=THIRD_PARTY_NOTICES.txt

# Writes matching lines of $1 to $2. Never closes a pipe early (no SIGPIPE under pipefail).
scan_file() {
  local file="$1" out="$2"
  : >"$out"
  if [ "$file" != "$NOTICES" ]; then
    grep -n -I -E "$SENSITIVE" "$file" >"$out.raw" 2>/dev/null || true
    grep -v -E "$SENSITIVE_ALLOW" "$out.raw" >"$out" 2>/dev/null || true
  fi
  if [ -f "$deny_file" ]; then
    grep -v -E '^\s*(#|$)' "$deny_file" >"$out.deny" || true
    [ -s "$out.deny" ] && grep -n -I -i -E -f "$out.deny" "$file" >>"$out" 2>/dev/null || true
  fi
  [ -s "$out" ]
}

# Detect absolute home paths in a binary (CI runner paths allowed). Bounded and pipe-safe.
binary_has_home_paths() {
  local bin="$1" out="$tmpd/strings.txt"
  strings -a "$bin" >"$out" 2>/dev/null || true
  grep -E '/Users/[^/ ]+/' "$out" | grep -v -E '^/Users/runner/' >"$out.hits" || true
  [ -s "$out.hits" ]
}

if [ "${1:-}" = "--self-test" ]; then
  printf 'ok\n/Users/runner/work/x\n' >"$tmpd/clean.bin"
  printf 'x\x00/Users/someone/src/lib.rs\x00y' >"$tmpd/dirty.bin"
  binary_has_home_paths "$tmpd/clean.bin" && { echo "self-test: false positive on runner path" >&2; exit 1; }
  binary_has_home_paths "$tmpd/dirty.bin" || { echo "self-test: missed planted home path" >&2; exit 1; }
  printf 'see /Users/somebody/notes\n' >"$tmpd/leak.md"
  printf 'contact: someone@company.com\n' >"$tmpd/mail.md"
  printf 'id = 123+someone@users.noreply.github.com, alex@example.com and /Users/example/demo\n' >"$tmpd/fine.md"
  scan_file "$tmpd/leak.md" "$tmpd/o1" || { echo "self-test: missed home path in text" >&2; exit 1; }
  scan_file "$tmpd/mail.md" "$tmpd/o2" || { echo "self-test: missed email" >&2; exit 1; }
  scan_file "$tmpd/fine.md" "$tmpd/o3" && { echo "self-test: false positive on allowed text" >&2; exit 1; }
  echo "identity self-test: OK"
  exit 0
fi

# --- metadata consistency ------------------------------------------------------------------
[ -f "$C_HEADER" ] || err "missing C header $C_HEADER"
grep -q "pub const ABI_VERSION: u32 = $ABI_VERSION;" src/ffi.rs || err "src/ffi.rs ABI_VERSION != $ABI_VERSION"
grep -q "ABI version $ABI_VERSION" "$C_HEADER" || err "$C_HEADER does not state ABI version $ABI_VERSION"
grep -q "abiVersion == $ABI_VERSION" macos/Tests/main.swift || err "Swift bridge test does not expect ABI $ABI_VERSION"
for sym in abi_version core_open core_call core_close string_free; do
  grep -q "${C_PREFIX}${sym}" "$C_HEADER" || err "header lacks ${C_PREFIX}${sym}"
  grep -q "fn ${C_PREFIX}${sym}" src/ffi.rs || err "src/ffi.rs lacks ${C_PREFIX}${sym}"
done
grep -q "APP_DIR_NAME: &str = \"$BUNDLE_ID\"" src/store.rs || err "store APP_DIR_NAME != $BUNDLE_ID"
grep -q "LEGACY_APP_DIR_NAME: &str = \"$LEGACY_ID\"" src/store.rs || err "store LEGACY_APP_DIR_NAME != $LEGACY_ID"
grep -q "DATA_DIR_ENV: &str = \"$DATA_DIR_ENV\"" src/store.rs || err "store DATA_DIR_ENV != $DATA_DIR_ENV"
grep -q "LEGACY_DATA_DIR_ENV: &str = \"$LEGACY_ENV\"" src/store.rs || err "store LEGACY_DATA_DIR_ENV != $LEGACY_ENV"
grep -q "struct ${PRODUCT_NAME}App: App" macos/Sources/App/${PRODUCT_NAME}App.swift || err "Swift app type is not ${PRODUCT_NAME}App"
grep -q "Window(\"$PRODUCT_NAME\", id:" macos/Sources/App/${PRODUCT_NAME}App.swift || err "main window title is not $PRODUCT_NAME"
grep -q "final class ${PRODUCT_NAME}Core" macos/Sources/Core/${PRODUCT_NAME}Core.swift || err "Swift bridge is not ${PRODUCT_NAME}Core"
grep -q "## \[$VERSION\]" CHANGELOG.md || err "CHANGELOG.md has no [$VERSION] section"
grep -q "$DATA_DIR_ENV" README.md || err "README does not document $DATA_DIR_ENV"
[ -s "$NOTICES" ] || err "missing $NOTICES"
grep -q "$NOTICES" README.md || err "README does not link $NOTICES"

# --- publishable files --------------------------------------------------------------------
files=()
if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  while IFS= read -r -d '' f; do [ -f "$f" ] && files+=("$f"); done \
    < <(git ls-files -z --cached --others --exclude-standard)
else
  while IFS= read -r -d '' f; do files+=("${f#./}"); done \
    < <(find . -type f -not -path './target/*' -not -path './dist/*' -not -path './private/*' -not -path './.git/*' -print0)
fi
legacy_allow='^(src/store\.rs|src/model\.rs|src/migrate\.rs|src/ffi\.rs|Cargo\.toml|tests/.*|macos/Tests/main\.swift|macos/Sources/Core/Models\.swift|macos/Sources/App/AppModel\.swift|docs/.*|CHANGELOG\.md|README\.md)$'
for f in "${files[@]}"; do
  case "$f" in scripts/check-identity.sh|Cargo.lock|*.icns|*.png) continue ;; esac
  if scan_file "$f" "$tmpd/hits"; then err "sensitive data in $f:"; head -5 "$tmpd/hits" >&2; fi
  if grep -q -E 'TextForge|textforge' "$f"; then err "old product name TextForge in $f"; fi
  if grep -q -i 'atext' "$f" && ! grep -q -E "$legacy_allow" <<<"$f"; then
    err "pre-0.3 name 'atext' in $f (outside the compatibility allowlist)"
  fi
done

# --- built bundle -------------------------------------------------------------------------
if [ "${1:-}" = "--app" ]; then
  app="${2:-dist/$APP_BUNDLE}"
  plist="$app/Contents/Info.plist"
  [ -f "$plist" ] || { err "no Info.plist in $app"; exit 1; }
  pb() { /usr/libexec/PlistBuddy -c "Print :$1" "$plist" 2>/dev/null || echo "<missing>"; }
  [ "$(pb CFBundleIdentifier)" = "$BUNDLE_ID" ] || err "CFBundleIdentifier $(pb CFBundleIdentifier) != $BUNDLE_ID"
  [ "$(pb CFBundleName)" = "$PRODUCT_NAME" ] || err "CFBundleName $(pb CFBundleName) != $PRODUCT_NAME"
  [ "$(pb CFBundleDisplayName)" = "$PRODUCT_NAME" ] || err "CFBundleDisplayName mismatch"
  [ "$(pb CFBundleExecutable)" = "$EXECUTABLE" ] || err "CFBundleExecutable mismatch"
  [ "$(pb CFBundleIconFile)" = "$PRODUCT_NAME" ] || err "CFBundleIconFile mismatch"
  [ "$(pb CFBundleShortVersionString)" = "$VERSION" ] || err "version $(pb CFBundleShortVersionString) != $VERSION"
  [ "$(pb LSMinimumSystemVersion)" = "$MINIMUM_MACOS" ] || err "LSMinimumSystemVersion mismatch"
  [ "$(basename "$app")" = "$APP_BUNDLE" ] || err "bundle folder $(basename "$app") != $APP_BUNDLE"
  bin="$app/Contents/MacOS/$EXECUTABLE"
  [ -x "$bin" ] || err "executable missing"
  [ -f "$app/Contents/Resources/$PRODUCT_NAME.icns" ] || err "icon missing"
  cmp -s "$NOTICES" "$app/Contents/Resources/$NOTICES" || err "bundled $NOTICES missing or differs from source"
  codesign --verify --deep --strict "$app" || err "codesign --verify failed"
  codesign -dv "$app" >"$tmpd/cs.txt" 2>&1 || true
  ident="$(sed -n 's/^Identifier=//p' "$tmpd/cs.txt")"
  [ "$ident" = "$BUNDLE_ID" ] || err "signature identifier '$ident' != $BUNDLE_ID"
  vtool -show-build "$bin" >"$tmpd/vt.txt" 2>/dev/null || true
  minos="$(awk '/minos/ {print $2; exit}' "$tmpd/vt.txt")"
  [ "$minos" = "$MINIMUM_MACOS" ] || err "minos $minos != $MINIMUM_MACOS"
  if binary_has_home_paths "$bin"; then err "binary embeds absolute home paths:"; head -3 "$tmpd/strings.txt.hits" >&2; fi
  strings -a "$bin" >"$tmpd/bs.txt" 2>/dev/null || true
  grep -q -E 'TextForge' "$tmpd/bs.txt" && err "binary still contains the old product name"
fi

[ "$fail" = 0 ] && echo "identity: OK ($PRODUCT_NAME $VERSION, $BUNDLE_ID)" || exit 1

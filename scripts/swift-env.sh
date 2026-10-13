# Shared build settings for the SwiftUI app. Source from the repository root.
# - Product metadata comes from Cargo.toml (scripts/product.py).
# - SDK discovery: use $SWIFT_SDK if set; otherwise the default macOS SDK, provided it can
#   compile SwiftUI's @State. Some newer SDKs implement @State as a macro whose compiler
#   plugin ships only with Xcode; with Command Line Tools alone, fall back to an installed
#   MacOSX26.x SDK, where @State is a property wrapper. Nothing global is changed.
eval "$(python3 scripts/product.py)"
export PATH="$HOME/.cargo/bin:$PATH"
export MACOSX_DEPLOYMENT_TARGET="$MINIMUM_MACOS"
# Keep machine paths (home folder, checkout location) out of built binaries. Use
# CARGO_ENCODED_RUSTFLAGS (0x1f-separated) so paths containing spaces stay intact; keep any
# flags the caller already set (encoded, or whitespace-separated RUSTFLAGS).
_sep=$'\x1f'
if [ -n "${CARGO_ENCODED_RUSTFLAGS:-}" ]; then
  _flags="$CARGO_ENCODED_RUSTFLAGS"
elif [ -n "${RUSTFLAGS:-}" ]; then
  read -r -a _rf <<<"$RUSTFLAGS"
  _flags="$(IFS="$_sep"; echo "${_rf[*]}")"
else
  _flags=""
fi
for _remap in "--remap-path-prefix=$PWD=." "--remap-path-prefix=$HOME=~"; do
  case "$_sep$_flags$_sep" in *"$_sep$_remap$_sep"*) ;; *) _flags="${_flags:+$_flags$_sep}$_remap" ;; esac
done
export CARGO_ENCODED_RUSTFLAGS="$_flags"
unset RUSTFLAGS _flags _rf _remap _sep
SWIFT_TARGET="$(uname -m)-apple-macos${MINIMUM_MACOS}"

swiftui_sdk_works() {
  local sdk="$1" probe
  [ -d "$sdk" ] || return 1
  probe="$(mktemp -d)"
  printf 'import SwiftUI\nstruct P: View { @State private var n = 0\n var body: some View { Text("\\(n)") } }\n' >"$probe/p.swift"
  swiftc -sdk "$sdk" -target "$SWIFT_TARGET" -typecheck "$probe/p.swift" >/dev/null 2>&1
  local ok=$?
  rm -rf "$probe"
  return $ok
}

if [ -z "${SWIFT_SDK:-}" ]; then
  default_sdk="$(xcrun --sdk macosx --show-sdk-path 2>/dev/null || true)"
  sdk_root="$(dirname "${default_sdk:-/Library/Developer/CommandLineTools/SDKs/x}")"
  for candidate in "$default_sdk" "$sdk_root/MacOSX26.5.sdk" "$sdk_root/MacOSX26.sdk"; do
    if [ -n "$candidate" ] && swiftui_sdk_works "$candidate"; then SWIFT_SDK="$candidate"; break; fi
  done
fi
if [ -z "${SWIFT_SDK:-}" ]; then
  echo "error: no macOS SDK here can compile SwiftUI @State. Install Xcode, or Command Line Tools with a MacOSX26.x SDK, or set SWIFT_SDK." >&2
  return 1 2>/dev/null || exit 1
fi
export SWIFT_SDK
# -isysroot for the link step too, so the binary is stamped with the SDK actually used.
SWIFT_FLAGS=(-sdk "$SWIFT_SDK" -target "$SWIFT_TARGET" -swift-version 6 -import-objc-header "$C_HEADER"
  -Xclang-linker -isysroot -Xclang-linker "$SWIFT_SDK" -file-prefix-map "$PWD=." -file-prefix-map "$HOME=~")
RUST_LINK=(-L target/release "-l$LIB_NAME" -framework AppKit -framework CoreGraphics -framework ApplicationServices
  -framework Carbon -framework CoreServices -liconv)

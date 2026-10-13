#!/usr/bin/env bash
# Every check used for a release. Uses temp directories only; never starts system-wide
# expansion, never posts events, never touches the user's library.
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/swift-env.sh
echo "== SDK: $(basename "$SWIFT_SDK")"
echo "== cargo fmt --check";            cargo fmt --check
echo "== cargo clippy -D warnings";     cargo clippy --locked --all-targets -- -D warnings
echo "== cargo clippy (legacy-egui)";   cargo clippy --locked --all-targets --features legacy-egui -- -D warnings
echo "== cargo test";                   cargo test --locked
echo "== swift-format lint --strict";   swift format lint --strict --configuration macos/.swift-format -r macos/Sources macos/Tests scripts/rasterize-svg.swift
echo "== Swift bridge tests"
cargo build --locked --release --lib
mkdir -p target/swift
swiftc "${SWIFT_FLAGS[@]}" macos/Sources/Core/*.swift macos/Tests/main.swift "${RUST_LINK[@]}" -o target/swift/BridgeTests
target/swift/BridgeTests
echo "== Swift app typecheck (warnings are errors)"
swiftc "${SWIFT_FLAGS[@]}" -parse-as-library -warnings-as-errors -typecheck macos/Sources/Core/*.swift macos/Sources/App/*.swift
echo "== identity self-test";            scripts/check-identity.sh --self-test
echo "== identity (source tree)";       scripts/check-identity.sh
echo "ALL CHECKS PASSED"

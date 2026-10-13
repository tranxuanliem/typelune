#!/usr/bin/env bash
# Build dist/Typelune.app: the SwiftUI app linked against the Rust core static library.
# Info.plist is generated from Cargo.toml metadata. The bundle is ad-hoc signed for local
# use only: it is NOT signed with a Developer ID and NOT notarized.
# This script installs nothing, adds no login items and changes no permission settings.
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/swift-env.sh

APP="dist/$APP_BUNDLE"
cargo build --locked --release --lib
mkdir -p target/swift
swiftc "${SWIFT_FLAGS[@]}" -parse-as-library -O \
  macos/Sources/Core/*.swift macos/Sources/App/*.swift "${RUST_LINK[@]}" \
  -o "target/swift/$EXECUTABLE"

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "target/swift/$EXECUTABLE" "$APP/Contents/MacOS/$EXECUTABLE"
cp assets/brand/Typelune.icns "$APP/Contents/Resources/$PRODUCT_NAME.icns"
cp THIRD_PARTY_NOTICES.txt "$APP/Contents/Resources/THIRD_PARTY_NOTICES.txt"  # before signing
xml_escape() { sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g' <<<"$1"; }
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
  <key>CFBundleName</key><string>$PRODUCT_NAME</string>
  <key>CFBundleDisplayName</key><string>$PRODUCT_NAME</string>
  <key>CFBundleExecutable</key><string>$EXECUTABLE</string>
  <key>CFBundleIconFile</key><string>$PRODUCT_NAME</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>LSMinimumSystemVersion</key><string>$MINIMUM_MACOS</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.productivity</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSPrincipalClass</key><string>NSApplication</string>
  <key>NSHumanReadableCopyright</key><string>$(xml_escape "$COPYRIGHT")</string>
</dict>
</plist>
PLIST
plutil -lint "$APP/Contents/Info.plist" >/dev/null
codesign --force --sign - --identifier "$BUNDLE_ID" "$APP"
echo "built $APP ($PRODUCT_NAME $VERSION, $BUNDLE_ID; SDK $(basename "$SWIFT_SDK"); ad-hoc signed, not notarized)"

#!/usr/bin/env bash
# Packages Corgi's desktop window with the corgi binary it runs:
#
# - on Linux, a tarball: corgi-gui-<version>-linux-<arch>.tar.gz holding
#   bin/corgi-gui, bin/corgi, a desktop entry and the licenses;
# - on macOS, an unsigned app: Corgi.app (corgi-gui and corgi side by side
#   in Contents/MacOS), zipped as corgi-gui-<version>-macos-<arch>.zip.
#
# Usage: scripts/package-gui.sh [OUT_DIR]   (default: target/package)
#
# Builds both binaries with `cargo build --release --locked` first; CARGO
# names another cargo. GPUI needs Rust 1.96 and, on Linux, the Wayland,
# xkbcommon and fontconfig development packages (see the README).
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="${1:-$root/target/package}"
cargo="${CARGO:-cargo}"

version="$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' "$root/Cargo.toml")"
if [ -z "$version" ]; then
  echo "package-gui: no version in Cargo.toml's [workspace.package]" >&2
  exit 1
fi
arch="$(uname -m)"
case "$arch" in
  arm64) arch=aarch64 ;;
esac

(cd "$root" && "$cargo" build --release --locked && "$cargo" build --release --locked -p corgi-gui)
release="${CARGO_TARGET_DIR:-$root/target}/release"
for binary in corgi corgi-gui; do
  if [ ! -x "$release/$binary" ]; then
    echo "package-gui: $release/$binary was not built" >&2
    exit 1
  fi
done

mkdir -p "$out"
stage="$(mktemp -d "${TMPDIR:-/tmp}/corgi-gui-package.XXXXXX")"
trap 'rm -rf "$stage"' EXIT

case "$(uname -s)" in
  Darwin)
    app="$stage/Corgi.app"
    mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
    cp "$release/corgi-gui" "$release/corgi" "$app/Contents/MacOS/"
    cp "$root/LICENSE" "$app/Contents/Resources/LICENSE"
    cp "$root/crates/corgi-gui/LICENSE-APACHE" "$app/Contents/Resources/LICENSE-APACHE"
    cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Corgi</string>
  <key>CFBundleDisplayName</key><string>Corgi</string>
  <key>CFBundleIdentifier</key><string>io.github.zyrre.corgi.gui</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleExecutable</key><string>corgi-gui</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict>
</plist>
PLIST
    archive="$out/corgi-gui-$version-macos-$arch.zip"
    rm -f "$archive"
    (cd "$stage" && ditto -c -k --norsrc --noextattr --keepParent Corgi.app "$archive")
    ;;
  *)
    name="corgi-gui-$version"
    dir="$stage/$name"
    mkdir -p "$dir/bin" "$dir/share/applications"
    cp "$release/corgi-gui" "$release/corgi" "$dir/bin/"
    cp "$root/LICENSE" "$dir/LICENSE"
    cp "$root/crates/corgi-gui/LICENSE-APACHE" "$dir/LICENSE-APACHE"
    # StartupWMClass is the window's Wayland app id, which compositor rules
    # (Hyprland's `class`) match.
    cat > "$dir/share/applications/corgi-gui.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Corgi
Comment=Corgi's desktop window on Herdr
Exec=corgi-gui
Terminal=false
Categories=Development;
StartupWMClass=corgi-gui
DESKTOP
    archive="$out/corgi-gui-$version-linux-$arch.tar.gz"
    tar -czf "$archive" -C "$stage" "$name"
    ;;
esac
echo "$archive"

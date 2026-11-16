#!/usr/bin/env bash
# Builds "Simple Design.app" and a .dmg containing it, on macOS. Only needs
# rustup/cargo and the tools that come with Xcode's command-line tools
# (lipo, sips, iconutil, codesign, hdiutil, xcrun).
#
#   ./packaging/macos/build-dmg.sh
#
# The app is a universal binary (Apple Silicon + Intel), with both the GUI and
# simple-design-cli in Contents/MacOS. Output goes to target/macos/.
#
# Signing, both optional:
#   CODESIGN_IDENTITY="Developer ID Application: Name (TEAMID)"
#       signs the app and the .dmg with the hardened runtime. Without it the
#       app is only ad-hoc signed, and Gatekeeper blocks it on other Macs
#       until the user allows it in System Settings > Privacy & Security.
#   NOTARY_PROFILE=<keychain profile>
#       also notarizes the .dmg and staples the ticket. Create the profile
#       once with `xcrun notarytool store-credentials`. Needs CODESIGN_IDENTITY.
set -euo pipefail

if [[ "$(uname)" != "Darwin" ]]; then
    echo "error: this script builds a macOS app and must run on macOS" >&2
    exit 1
fi
if [[ -n "${NOTARY_PROFILE:-}" && -z "${CODESIGN_IDENTITY:-}" ]]; then
    echo "error: NOTARY_PROFILE needs CODESIGN_IDENTITY too" >&2
    exit 1
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../.." && pwd)"
version="$(sed -n 's/^version = "\(.*\)"$/\1/p' "${repo_root}/Cargo.toml" | head -n 1)"
app_name="Simple Design"
out="${repo_root}/target/macos"
app="${out}/${app_name}.app"
dmg="${out}/simple-design-${version}.dmg"
targets=(aarch64-apple-darwin x86_64-apple-darwin)
binaries=(simple-design simple-design-cli)

# Matches Info.plist's LSMinimumSystemVersion.
export MACOSX_DEPLOYMENT_TARGET=11.0

rustup target add "${targets[@]}"
for target in "${targets[@]}"; do
    cargo build --release --locked --target "${target}" --manifest-path "${repo_root}/Cargo.toml"
done

rm -rf "${app}" "${dmg}"
mkdir -p "${app}/Contents/MacOS" "${app}/Contents/Resources"

for bin in "${binaries[@]}"; do
    lipo -create -output "${app}/Contents/MacOS/${bin}" \
        "${repo_root}/target/aarch64-apple-darwin/release/${bin}" \
        "${repo_root}/target/x86_64-apple-darwin/release/${bin}"
done

sed "s|__VERSION__|${version}|g" "${script_dir}/Info.plist" > "${app}/Contents/Info.plist"

# An .icns from the 512x512 app icon (so no 512@2x, which needs a 1024 source).
iconset="$(mktemp -d)/simple-design.iconset"
mkdir -p "${iconset}"
icon="${repo_root}/assets/icon/simple-design.png"
for size in 16 32 128 256 512; do
    sips -z "${size}" "${size}" "${icon}" --out "${iconset}/icon_${size}x${size}.png" >/dev/null
    if (( size * 2 <= 512 )); then
        sips -z $((size * 2)) $((size * 2)) "${icon}" --out "${iconset}/icon_${size}x${size}@2x.png" >/dev/null
    fi
done
iconutil -c icns "${iconset}" -o "${app}/Contents/Resources/simple-design.icns"
rm -rf "$(dirname "${iconset}")"

# Sign the inner binaries first, then the bundle that seals them.
if [[ -n "${CODESIGN_IDENTITY:-}" ]]; then
    sign=(codesign --force --options runtime --timestamp --sign "${CODESIGN_IDENTITY}")
else
    sign=(codesign --force --sign -)
fi
for bin in "${binaries[@]}"; do
    "${sign[@]}" "${app}/Contents/MacOS/${bin}"
done
"${sign[@]}" "${app}"
codesign --verify --strict "${app}"

# The usual drag-to-install layout: the app next to a link to /Applications.
staging="$(mktemp -d)"
cp -R "${app}" "${staging}/"
ln -s /Applications "${staging}/Applications"
hdiutil create -volname "${app_name}" -srcfolder "${staging}" -ov -format UDZO "${dmg}" >/dev/null
rm -rf "${staging}"

if [[ -n "${CODESIGN_IDENTITY:-}" ]]; then
    codesign --force --timestamp --sign "${CODESIGN_IDENTITY}" "${dmg}"
fi
if [[ -n "${NOTARY_PROFILE:-}" ]]; then
    xcrun notarytool submit "${dmg}" --keychain-profile "${NOTARY_PROFILE}" --wait
    xcrun stapler staple "${dmg}"
fi

echo "built ${app}"
echo "built ${dmg}"

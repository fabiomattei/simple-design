#!/usr/bin/env bash
# Installs a .desktop entry for Simple Design so GNOME (and other desktops)
# show the app icon in the dock, app grid, and alt-tab. Wayland only resolves
# window icons through a .desktop file matched by app_id (see main.rs's
# with_app_id) -- there is no other way to attach an icon to a window there.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../.." && pwd)"
binary_path="${repo_root}/target/release/simple-design"
icon_path="${repo_root}/assets/icon.png"

if [[ ! -x "${binary_path}" ]]; then
    echo "error: release binary not found at ${binary_path}" >&2
    echo "build it first with: cargo build --release" >&2
    exit 1
fi

applications_dir="${HOME}/.local/share/applications"
mkdir -p "${applications_dir}"

desktop_dest="${applications_dir}/simple-design.desktop"
sed \
    -e "s|__EXEC_PATH__|${binary_path}|" \
    -e "s|__ICON_PATH__|${icon_path}|" \
    "${script_dir}/simple-design.desktop" > "${desktop_dest}"

echo "installed ${desktop_dest}"

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "${applications_dir}"
fi

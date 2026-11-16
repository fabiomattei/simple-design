#!/usr/bin/env bash
# Installs Simple Design for the current user, without root:
#   - both binaries (GUI + CLI) into ~/.local/bin
#   - the app icon into ~/.local/share/icons/hicolor
#   - a .desktop entry into ~/.local/share/applications
# The .desktop entry is what makes GNOME (and other desktops) show the app
# icon in the dock, app grid, and alt-tab. Wayland only resolves window icons
# through a .desktop file matched by app_id (see main.rs's with_app_id) --
# there is no other way to attach an icon to a window there.
#
# The installed copy is self-contained: it keeps working if this checkout is
# moved or rebuilt. Re-run this script to update it.
#
#   ./packaging/linux/install.sh              # build release + install
#   ./packaging/linux/install.sh --no-build   # install an existing release build
#   ./packaging/linux/install.sh --uninstall  # remove everything installed here
#
# PREFIX (default ~/.local) overrides the install root.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../.." && pwd)"
prefix="${PREFIX:-${HOME}/.local}"

bin_dir="${prefix}/bin"
applications_dir="${prefix}/share/applications"
icons_root="${prefix}/share/icons/hicolor"
icon_dest="${icons_root}/256x256/apps/simple-design.png"
desktop_dest="${applications_dir}/simple-design.desktop"
binaries=(simple-design simple-design-cli)

refresh_caches() {
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database "${applications_dir}" || true
    fi
    if command -v gtk-update-icon-cache >/dev/null 2>&1; then
        gtk-update-icon-cache --quiet --ignore-theme-index "${icons_root}" || true
    fi
}

case "${1:-}" in
    --uninstall)
        for bin in "${binaries[@]}"; do
            rm -fv "${bin_dir}/${bin}"
        done
        rm -fv "${icon_dest}" "${desktop_dest}"
        refresh_caches
        exit 0
        ;;
    --no-build) ;;
    "")
        cargo build --release --locked --manifest-path "${repo_root}/Cargo.toml"
        ;;
    *)
        echo "usage: $0 [--no-build | --uninstall]" >&2
        exit 2
        ;;
esac

for bin in "${binaries[@]}"; do
    if [[ ! -x "${repo_root}/target/release/${bin}" ]]; then
        echo "error: release binary not found at ${repo_root}/target/release/${bin}" >&2
        echo "build it first with: cargo build --release" >&2
        exit 1
    fi
done

for bin in "${binaries[@]}"; do
    install -Dm755 "${repo_root}/target/release/${bin}" "${bin_dir}/${bin}"
    echo "installed ${bin_dir}/${bin}"
done

# The square 256x256 Flatpak icon -- icon themes expect square icons, and
# assets/icon.png is 333x346.
install -Dm644 "${repo_root}/packaging/flatpak/icon.png" "${icon_dest}"
echo "installed ${icon_dest}"

mkdir -p "${applications_dir}"
sed \
    -e "s|__EXEC_PATH__|${bin_dir}/simple-design|" \
    "${script_dir}/simple-design.desktop" > "${desktop_dest}"
echo "installed ${desktop_dest}"

refresh_caches

case ":${PATH}:" in
    *":${bin_dir}:"*) ;;
    *) echo "note: ${bin_dir} is not on your PATH -- add it to use simple-design-cli from a shell" ;;
esac

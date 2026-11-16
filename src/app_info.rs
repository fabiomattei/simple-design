//! What the About dialog says about this program. Everything except the icon comes from
//! `Cargo.toml`'s `[package]` section, so a version bump there is the only edit a release needs.

pub const NAME: &str = "Simple Design";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const DESCRIPTION: &str = env!("CARGO_PKG_DESCRIPTION");
/// `authors` joined by `:`, as Cargo passes them.
pub const AUTHORS: &str = env!("CARGO_PKG_AUTHORS");
/// An SPDX expression, e.g. `GPL-3.0-or-later`.
pub const LICENSE: &str = env!("CARGO_PKG_LICENSE");
pub const HOMEPAGE: &str = env!("CARGO_PKG_HOMEPAGE");
pub const ISSUES_URL: &str = concat!(env!("CARGO_PKG_REPOSITORY"), "/issues");
/// The Flatpak's app id (`packaging/flatpak/`), which is also the window's app id there.
pub const FLATPAK_ID: &str = "io.github.fabiomattei.simple-design";
/// The year development started, for the copyright line.
pub const FIRST_YEAR: u32 = 2026;

/// The lady's slipper orchid, drawn as `assets/icon/simple-design.svg`.
pub const APP_ICON_PNG: &[u8] = include_bytes!("../assets/icon/simple-design.png");

/// `LICENSE` spelled out for people rather than tools.
pub fn license_name() -> &'static str {
    match LICENSE {
        "GPL-3.0-or-later" => "GNU General Public License v3.0 or later",
        other => other,
    }
}

/// `AUTHORS` as a comma-separated list.
pub fn authors() -> String {
    AUTHORS.split(':').collect::<Vec<_>>().join(", ")
}

//! Discovers fonts actually installed on the user's machine (macOS or
//! Linux — see `fontdb::Database::load_system_fonts`, which scans each OS's
//! known font directories directly rather than linking a system library, so
//! it needs nothing beyond what's already on disk). Backs
//! `TextFont::System`, the third tier of font choice after the two egui
//! defaults and the three bundled extras in `fonts.rs`.
//!
//! A system font is inherently machine-specific — a `.sdesign` file opened
//! on a different OS, or one missing the referenced font, won't find it.
//! Every render site (`text_layout.rs`, `canvas.rs`, `export.rs`,
//! `text_outline.rs`) treats a lookup miss as "fall back to
//! `TextFont::Proportional`" rather than panicking or drawing nothing.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

fn database() -> &'static fontdb::Database {
    static DB: OnceLock<fontdb::Database> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        db
    })
}

/// Sorted, deduplicated list of installed font family names, suitable for a
/// picker dropdown. A family can back several faces (regular/bold/italic/…)
/// that all share one name — those collapse to a single entry here, since
/// `TextFont` only ever selects one face per family (see `family_data`).
/// Names starting with `.` (macOS's convention for internal system fonts
/// like `.AppleSystemUIFont` — not meant to be user-selectable, same ones
/// Font Book itself hides) are filtered out.
pub fn family_names() -> &'static [String] {
    static NAMES: OnceLock<Vec<String>> = OnceLock::new();
    NAMES.get_or_init(|| {
        let mut names: Vec<String> = database()
            .faces()
            .filter_map(|face| face.families.first().map(|(name, _)| name.clone()))
            .filter(|name| !name.starts_with('.'))
            .collect();
        names.sort_unstable_by_key(|n| n.to_lowercase());
        names.dedup_by(|a, b| a.to_lowercase() == b.to_lowercase());
        names
    })
}

/// Raw font file bytes and face index for `family`'s best match to a plain
/// regular/normal face (fontdb's own CSS-like matching). `None` if no
/// installed font has this family name.
pub(crate) fn family_data(family: &str) -> Option<(Vec<u8>, u32)> {
    let db = database();
    let id = db.query(&fontdb::Query { families: &[fontdb::Family::Name(family)], ..Default::default() })?;
    db.with_face_data(id, |data, index| (data.to_vec(), index))
}

/// Raw font file bytes and face index for `family`'s bold face — `None` if
/// the family isn't installed *or* it has no face distinct from the plain
/// lookup's (most system fonts only ship one weight). Backs
/// `fonts::ab_glyph_bytes_bold`'s `System` case.
pub(crate) fn family_data_bold(family: &str) -> Option<(Vec<u8>, u32)> {
    let db = database();
    let regular_id = db.query(&fontdb::Query { families: &[fontdb::Family::Name(family)], ..Default::default() });
    let bold_id = db.query(&fontdb::Query {
        families: &[fontdb::Family::Name(family)],
        weight: fontdb::Weight::BOLD,
        ..Default::default()
    })?;
    if Some(bold_id) == regular_id {
        return None;
    }
    db.with_face_data(bold_id, |data, index| (data.to_vec(), index))
}

/// The egui `FontFamily::Name` a bold face for `family` is registered
/// under — distinct from `family` itself (which always names the *regular*
/// weight) so both can be registered/resolved independently. The " (Bold)"
/// suffix could theoretically collide with a real family literally named
/// that; accepted as a negligible risk rather than plumbing a structured
/// key through egui's string-keyed `FontDefinitions`.
fn bold_key(family: &str) -> String {
    format!("{family} (Bold)")
}

fn seen() -> &'static Mutex<HashSet<String>> {
    static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    SEEN.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Kicks off registering `family` with the egui context as a same-named
/// `FontFamily::Name`, the first time it's seen — a process-wide cache
/// short-circuits every call after that (whether the lookup succeeded or
/// not), so a family that isn't actually installed doesn't re-scan the
/// system font database every frame it's drawn. Prefer `resolve_family`
/// over calling this directly — registering alone isn't enough to safely
/// use the family *this* frame (see its doc comment).
pub fn ensure_registered(ctx: &egui::Context, family: &str) {
    {
        if seen().lock().unwrap().contains(family) {
            return;
        }
    }
    seen().lock().unwrap().insert(family.to_owned());

    let Some((bytes, index)) = family_data(family) else { return };
    let mut data = egui::FontData::from_owned(bytes);
    data.index = index;
    ctx.add_font(egui::epaint::text::FontInsert::new(
        family,
        data,
        vec![egui::epaint::text::InsertFontFamily {
            family: egui::FontFamily::Name(family.into()),
            priority: egui::epaint::text::FontPriority::Highest,
        }],
    ));
    // eframe's event loop is reactive (waits for input between frames) —
    // without this, the "next pass" the family needs to become bound might
    // not happen until the user moves the mouse or otherwise generates an
    // event, leaving anything drawn with it stuck on the `Proportional`
    // fallback in the meantime.
    ctx.request_repaint();
}

/// Resolves `family` (an OS font family name) to an egui `FontFamily` safe
/// to put in a `FontId` and lay out on screen *this frame*.
///
/// `Context::add_font` only takes effect "at the start of the next pass"
/// (its own doc comment) — laying out text against a family before then
/// panics inside egui (`FontFamily::{family} is not bound to any fonts`).
/// So this checks whether the context itself already reports the family as
/// bound; if not, it queues registration (harmless to call repeatedly) and
/// falls back to `Proportional` for this one frame. Once egui picks up the
/// queued font on the next pass, subsequent calls return the real family.
pub fn resolve_family(ctx: &egui::Context, family: &str) -> egui::FontFamily {
    let target = egui::FontFamily::Name(family.into());
    let ready = ctx.fonts(|f| f.definitions().families.contains_key(&target));
    if ready {
        return target;
    }
    ensure_registered(ctx, family);
    egui::FontFamily::Proportional
}

/// Bold counterpart to `ensure_registered` — registers `family`'s bold face
/// under `bold_key(family)`, if `family_data_bold` finds one distinct from
/// the regular face. No-op (nothing to register) if it doesn't; callers go
/// through `resolve_family_bold`, which handles that fallback.
pub fn ensure_registered_bold(ctx: &egui::Context, family: &str) {
    let key = bold_key(family);
    {
        if seen().lock().unwrap().contains(&key) {
            return;
        }
    }
    seen().lock().unwrap().insert(key.clone());

    let Some((bytes, index)) = family_data_bold(family) else { return };
    let mut data = egui::FontData::from_owned(bytes);
    data.index = index;
    ctx.add_font(egui::epaint::text::FontInsert::new(
        &key,
        data,
        vec![egui::epaint::text::InsertFontFamily {
            family: egui::FontFamily::Name(key.clone().into()),
            priority: egui::epaint::text::FontPriority::Highest,
        }],
    ));
    ctx.request_repaint();
}

/// Resolves `family`'s bold face to an egui `FontFamily` safe to use this
/// frame, the bold counterpart to `resolve_family`. Falls back to
/// `resolve_family(ctx, family)` (the *regular* weight, not bolded) both
/// while a genuine bold face is still being registered (next-pass timing,
/// same as `resolve_family`) and permanently if the family has no distinct
/// bold face at all — a run marked bold in such a font just doesn't
/// visually thicken, rather than falling all the way back to
/// `Proportional` the way an entirely-missing family would.
pub fn resolve_family_bold(ctx: &egui::Context, family: &str) -> egui::FontFamily {
    let target = egui::FontFamily::Name(bold_key(family).into());
    let ready = ctx.fonts(|f| f.definitions().families.contains_key(&target));
    if ready {
        return target;
    }
    ensure_registered_bold(ctx, family);
    resolve_family(ctx, family)
}

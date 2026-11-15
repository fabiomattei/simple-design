//! File > New from Template: starter `.sdesign` documents a new document is
//! copied from. Built-in templates live in the repo's `templates/` folder and
//! are compiled into the binary; user templates are any `.sdesign` file in
//! `<config_dir>/templates/` (see `config_dir.rs`), which File > Save as
//! Template writes into. A template is an ordinary document — no placeholder
//! syntax — so making one is just designing it and saving it there.

use std::path::PathBuf;

use crate::model::Document;

/// Compiled-in starter templates, as `(name, .sdesign JSON)`.
const BUILT_IN: &[(&str, &str)] = &[
    ("Landing Page", include_str!("../templates/landing-page.sdesign")),
    ("Mobile App Screen", include_str!("../templates/mobile-app-screen.sdesign")),
    ("Social Post", include_str!("../templates/social-post.sdesign")),
    ("Poster & Business Card", include_str!("../templates/poster-and-business-card.sdesign")),
    ("Presentation", include_str!("../templates/presentation.sdesign")),
    ("Story", include_str!("../templates/story.sdesign")),
    ("Resume", include_str!("../templates/resume.sdesign")),
    ("Dashboard", include_str!("../templates/dashboard.sdesign")),
    ("Video Thumbnail", include_str!("../templates/video-thumbnail.sdesign")),
    ("Email Newsletter", include_str!("../templates/email-newsletter.sdesign")),
    ("Flyer - Minimal Mono", include_str!("../templates/flyer-minimal-mono.sdesign")),
    ("Flyer - Neon Night", include_str!("../templates/flyer-neon-night.sdesign")),
    ("Flyer - Pastel", include_str!("../templates/flyer-pastel.sdesign")),
    ("Flyer - Corporate Navy", include_str!("../templates/flyer-corporate-navy.sdesign")),
    ("Flyer - Earthy", include_str!("../templates/flyer-earthy.sdesign")),
    ("Flyer - Retro 70s", include_str!("../templates/flyer-retro-70s.sdesign")),
    ("Flyer - Dark Luxe Gold", include_str!("../templates/flyer-dark-luxe.sdesign")),
    ("Flyer - Brutalist", include_str!("../templates/flyer-brutalist.sdesign")),
    ("Flyer - Ocean", include_str!("../templates/flyer-ocean.sdesign")),
    ("Flyer - Sunset", include_str!("../templates/flyer-sunset.sdesign")),
];

#[derive(Clone)]
pub enum TemplateSource {
    BuiltIn(&'static str),
    User(PathBuf),
}

#[derive(Clone)]
pub struct Template {
    pub name: String,
    pub source: TemplateSource,
}

impl Template {
    /// Parses the template into a fresh, unsaved document named "Untitled",
    /// like File > New. Errors if a user template file is unreadable or not
    /// a valid `.sdesign` document.
    pub fn instantiate(&self) -> anyhow::Result<Document> {
        let mut document: Document = match &self.source {
            TemplateSource::BuiltIn(json) => serde_json::from_str(json)?,
            TemplateSource::User(path) => crate::io::load_from(path)?,
        };
        document.name = "Untitled".to_string();
        Ok(document)
    }
}

/// Built-in templates first (in `BUILT_IN` order), then user templates
/// sorted by name. An unreadable or missing user folder just contributes
/// none — it doesn't exist until the first Save as Template.
pub fn all_templates() -> Vec<Template> {
    let mut templates: Vec<Template> = BUILT_IN
        .iter()
        .map(|(name, json)| Template { name: name.to_string(), source: TemplateSource::BuiltIn(json) })
        .collect();
    templates.extend(user_templates());
    templates
}

/// Case-insensitive lookup by name across `all_templates`.
pub fn find_template(name: &str) -> Option<Template> {
    all_templates().into_iter().find(|t| t.name.eq_ignore_ascii_case(name))
}

/// `<config_dir>/templates`, the folder Save as Template writes into.
/// `None` when no config folder can be determined. Does not create it.
pub fn user_templates_dir() -> Option<PathBuf> {
    Some(crate::config_dir::config_dir()?.join("templates"))
}

fn user_templates() -> Vec<Template> {
    let Some(entries) = user_templates_dir().and_then(|dir| std::fs::read_dir(dir).ok()) else { return Vec::new() };
    let mut templates: Vec<Template> = entries
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "sdesign"))
        .filter_map(|path| {
            let name = path.file_stem()?.to_string_lossy().into_owned();
            Some(Template { name, source: TemplateSource::User(path) })
        })
        .collect();
    templates.sort_by(|a, b| a.name.cmp(&b.name));
    templates
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_built_in_template_parses_into_a_document() {
        for (name, json) in BUILT_IN {
            let template = Template { name: name.to_string(), source: TemplateSource::BuiltIn(json) };
            let document = template.instantiate().unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(document.name, "Untitled");
            assert!(document.pages.iter().any(|p| !p.layers.is_empty()), "{name} has no layers");
        }
    }
}

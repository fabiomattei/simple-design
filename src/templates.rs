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
    ("Invitation - Minimal Mono", include_str!("../templates/invitation-minimal-mono.sdesign")),
    ("Invitation - Neon Night", include_str!("../templates/invitation-neon-night.sdesign")),
    ("Invitation - Pastel", include_str!("../templates/invitation-pastel.sdesign")),
    ("Invitation - Corporate Navy", include_str!("../templates/invitation-corporate-navy.sdesign")),
    ("Invitation - Earthy", include_str!("../templates/invitation-earthy.sdesign")),
    ("Invitation - Retro 70s", include_str!("../templates/invitation-retro-70s.sdesign")),
    ("Invitation - Dark Luxe Gold", include_str!("../templates/invitation-dark-luxe.sdesign")),
    ("Invitation - Brutalist", include_str!("../templates/invitation-brutalist.sdesign")),
    ("Invitation - Ocean", include_str!("../templates/invitation-ocean.sdesign")),
    ("Invitation - Sunset", include_str!("../templates/invitation-sunset.sdesign")),
    ("Menu - Minimal Mono", include_str!("../templates/menu-minimal-mono.sdesign")),
    ("Menu - Neon Night", include_str!("../templates/menu-neon-night.sdesign")),
    ("Menu - Pastel", include_str!("../templates/menu-pastel.sdesign")),
    ("Menu - Corporate Navy", include_str!("../templates/menu-corporate-navy.sdesign")),
    ("Menu - Earthy", include_str!("../templates/menu-earthy.sdesign")),
    ("Menu - Retro 70s", include_str!("../templates/menu-retro-70s.sdesign")),
    ("Menu - Dark Luxe Gold", include_str!("../templates/menu-dark-luxe.sdesign")),
    ("Menu - Brutalist", include_str!("../templates/menu-brutalist.sdesign")),
    ("Menu - Ocean", include_str!("../templates/menu-ocean.sdesign")),
    ("Menu - Sunset", include_str!("../templates/menu-sunset.sdesign")),
    ("Certificate - Minimal Mono", include_str!("../templates/certificate-minimal-mono.sdesign")),
    ("Certificate - Neon Night", include_str!("../templates/certificate-neon-night.sdesign")),
    ("Certificate - Pastel", include_str!("../templates/certificate-pastel.sdesign")),
    ("Certificate - Corporate Navy", include_str!("../templates/certificate-corporate-navy.sdesign")),
    ("Certificate - Earthy", include_str!("../templates/certificate-earthy.sdesign")),
    ("Certificate - Retro 70s", include_str!("../templates/certificate-retro-70s.sdesign")),
    ("Certificate - Dark Luxe Gold", include_str!("../templates/certificate-dark-luxe.sdesign")),
    ("Certificate - Brutalist", include_str!("../templates/certificate-brutalist.sdesign")),
    ("Certificate - Ocean", include_str!("../templates/certificate-ocean.sdesign")),
    ("Certificate - Sunset", include_str!("../templates/certificate-sunset.sdesign")),
    ("Ad Banner - Minimal Mono", include_str!("../templates/ad-banner-minimal-mono.sdesign")),
    ("Ad Banner - Neon Night", include_str!("../templates/ad-banner-neon-night.sdesign")),
    ("Ad Banner - Pastel", include_str!("../templates/ad-banner-pastel.sdesign")),
    ("Ad Banner - Corporate Navy", include_str!("../templates/ad-banner-corporate-navy.sdesign")),
    ("Ad Banner - Earthy", include_str!("../templates/ad-banner-earthy.sdesign")),
    ("Ad Banner - Retro 70s", include_str!("../templates/ad-banner-retro-70s.sdesign")),
    ("Ad Banner - Dark Luxe Gold", include_str!("../templates/ad-banner-dark-luxe.sdesign")),
    ("Ad Banner - Brutalist", include_str!("../templates/ad-banner-brutalist.sdesign")),
    ("Ad Banner - Ocean", include_str!("../templates/ad-banner-ocean.sdesign")),
    ("Ad Banner - Sunset", include_str!("../templates/ad-banner-sunset.sdesign")),
    ("Pricing Page - Minimal Mono", include_str!("../templates/pricing-minimal-mono.sdesign")),
    ("Pricing Page - Neon Night", include_str!("../templates/pricing-neon-night.sdesign")),
    ("Pricing Page - Pastel", include_str!("../templates/pricing-pastel.sdesign")),
    ("Pricing Page - Corporate Navy", include_str!("../templates/pricing-corporate-navy.sdesign")),
    ("Pricing Page - Earthy", include_str!("../templates/pricing-earthy.sdesign")),
    ("Pricing Page - Retro 70s", include_str!("../templates/pricing-retro-70s.sdesign")),
    ("Pricing Page - Dark Luxe Gold", include_str!("../templates/pricing-dark-luxe.sdesign")),
    ("Pricing Page - Brutalist", include_str!("../templates/pricing-brutalist.sdesign")),
    ("Pricing Page - Ocean", include_str!("../templates/pricing-ocean.sdesign")),
    ("Pricing Page - Sunset", include_str!("../templates/pricing-sunset.sdesign")),
    ("Blog Header - Minimal Mono", include_str!("../templates/blog-header-minimal-mono.sdesign")),
    ("Blog Header - Neon Night", include_str!("../templates/blog-header-neon-night.sdesign")),
    ("Blog Header - Pastel", include_str!("../templates/blog-header-pastel.sdesign")),
    ("Blog Header - Corporate Navy", include_str!("../templates/blog-header-corporate-navy.sdesign")),
    ("Blog Header - Earthy", include_str!("../templates/blog-header-earthy.sdesign")),
    ("Blog Header - Retro 70s", include_str!("../templates/blog-header-retro-70s.sdesign")),
    ("Blog Header - Dark Luxe Gold", include_str!("../templates/blog-header-dark-luxe.sdesign")),
    ("Blog Header - Brutalist", include_str!("../templates/blog-header-brutalist.sdesign")),
    ("Blog Header - Ocean", include_str!("../templates/blog-header-ocean.sdesign")),
    ("Blog Header - Sunset", include_str!("../templates/blog-header-sunset.sdesign")),
    ("Quote Card - Minimal Mono", include_str!("../templates/quote-card-minimal-mono.sdesign")),
    ("Quote Card - Neon Night", include_str!("../templates/quote-card-neon-night.sdesign")),
    ("Quote Card - Pastel", include_str!("../templates/quote-card-pastel.sdesign")),
    ("Quote Card - Corporate Navy", include_str!("../templates/quote-card-corporate-navy.sdesign")),
    ("Quote Card - Earthy", include_str!("../templates/quote-card-earthy.sdesign")),
    ("Quote Card - Retro 70s", include_str!("../templates/quote-card-retro-70s.sdesign")),
    ("Quote Card - Dark Luxe Gold", include_str!("../templates/quote-card-dark-luxe.sdesign")),
    ("Quote Card - Brutalist", include_str!("../templates/quote-card-brutalist.sdesign")),
    ("Quote Card - Ocean", include_str!("../templates/quote-card-ocean.sdesign")),
    ("Quote Card - Sunset", include_str!("../templates/quote-card-sunset.sdesign")),
    ("Event Ticket - Minimal Mono", include_str!("../templates/ticket-minimal-mono.sdesign")),
    ("Event Ticket - Neon Night", include_str!("../templates/ticket-neon-night.sdesign")),
    ("Event Ticket - Pastel", include_str!("../templates/ticket-pastel.sdesign")),
    ("Event Ticket - Corporate Navy", include_str!("../templates/ticket-corporate-navy.sdesign")),
    ("Event Ticket - Earthy", include_str!("../templates/ticket-earthy.sdesign")),
    ("Event Ticket - Retro 70s", include_str!("../templates/ticket-retro-70s.sdesign")),
    ("Event Ticket - Dark Luxe Gold", include_str!("../templates/ticket-dark-luxe.sdesign")),
    ("Event Ticket - Brutalist", include_str!("../templates/ticket-brutalist.sdesign")),
    ("Event Ticket - Ocean", include_str!("../templates/ticket-ocean.sdesign")),
    ("Event Ticket - Sunset", include_str!("../templates/ticket-sunset.sdesign")),
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

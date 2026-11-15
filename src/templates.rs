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
    ("Onboarding Screen - Minimal Mono", include_str!("../templates/onboarding-minimal-mono.sdesign")),
    ("Onboarding Screen - Neon Night", include_str!("../templates/onboarding-neon-night.sdesign")),
    ("Onboarding Screen - Pastel", include_str!("../templates/onboarding-pastel.sdesign")),
    ("Onboarding Screen - Corporate Navy", include_str!("../templates/onboarding-corporate-navy.sdesign")),
    ("Onboarding Screen - Earthy", include_str!("../templates/onboarding-earthy.sdesign")),
    ("Onboarding Screen - Retro 70s", include_str!("../templates/onboarding-retro-70s.sdesign")),
    ("Onboarding Screen - Dark Luxe Gold", include_str!("../templates/onboarding-dark-luxe.sdesign")),
    ("Onboarding Screen - Brutalist", include_str!("../templates/onboarding-brutalist.sdesign")),
    ("Onboarding Screen - Ocean", include_str!("../templates/onboarding-ocean.sdesign")),
    ("Onboarding Screen - Sunset", include_str!("../templates/onboarding-sunset.sdesign")),
    ("Business Card - Minimal Mono", include_str!("../templates/business-card-minimal-mono.sdesign")),
    ("Business Card - Neon Night", include_str!("../templates/business-card-neon-night.sdesign")),
    ("Business Card - Pastel", include_str!("../templates/business-card-pastel.sdesign")),
    ("Business Card - Corporate Navy", include_str!("../templates/business-card-corporate-navy.sdesign")),
    ("Business Card - Earthy", include_str!("../templates/business-card-earthy.sdesign")),
    ("Business Card - Retro 70s", include_str!("../templates/business-card-retro-70s.sdesign")),
    ("Business Card - Dark Luxe Gold", include_str!("../templates/business-card-dark-luxe.sdesign")),
    ("Business Card - Brutalist", include_str!("../templates/business-card-brutalist.sdesign")),
    ("Business Card - Ocean", include_str!("../templates/business-card-ocean.sdesign")),
    ("Business Card - Sunset", include_str!("../templates/business-card-sunset.sdesign")),
    ("Video Thumbnail - Minimal Mono", include_str!("../templates/video-thumbnail-minimal-mono.sdesign")),
    ("Video Thumbnail - Neon Night", include_str!("../templates/video-thumbnail-neon-night.sdesign")),
    ("Video Thumbnail - Pastel", include_str!("../templates/video-thumbnail-pastel.sdesign")),
    ("Video Thumbnail - Corporate Navy", include_str!("../templates/video-thumbnail-corporate-navy.sdesign")),
    ("Video Thumbnail - Earthy", include_str!("../templates/video-thumbnail-earthy.sdesign")),
    ("Video Thumbnail - Retro 70s", include_str!("../templates/video-thumbnail-retro-70s.sdesign")),
    ("Video Thumbnail - Dark Luxe Gold", include_str!("../templates/video-thumbnail-dark-luxe.sdesign")),
    ("Video Thumbnail - Brutalist", include_str!("../templates/video-thumbnail-brutalist.sdesign")),
    ("Video Thumbnail - Ocean", include_str!("../templates/video-thumbnail-ocean.sdesign")),
    ("Video Thumbnail - Sunset", include_str!("../templates/video-thumbnail-sunset.sdesign")),
    ("Story - Minimal Mono", include_str!("../templates/story-minimal-mono.sdesign")),
    ("Story - Neon Night", include_str!("../templates/story-neon-night.sdesign")),
    ("Story - Pastel", include_str!("../templates/story-pastel.sdesign")),
    ("Story - Corporate Navy", include_str!("../templates/story-corporate-navy.sdesign")),
    ("Story - Earthy", include_str!("../templates/story-earthy.sdesign")),
    ("Story - Retro 70s", include_str!("../templates/story-retro-70s.sdesign")),
    ("Story - Dark Luxe Gold", include_str!("../templates/story-dark-luxe.sdesign")),
    ("Story - Brutalist", include_str!("../templates/story-brutalist.sdesign")),
    ("Story - Ocean", include_str!("../templates/story-ocean.sdesign")),
    ("Story - Sunset", include_str!("../templates/story-sunset.sdesign")),
    ("Resume - Minimal Mono", include_str!("../templates/resume-minimal-mono.sdesign")),
    ("Resume - Neon Night", include_str!("../templates/resume-neon-night.sdesign")),
    ("Resume - Pastel", include_str!("../templates/resume-pastel.sdesign")),
    ("Resume - Corporate Navy", include_str!("../templates/resume-corporate-navy.sdesign")),
    ("Resume - Earthy", include_str!("../templates/resume-earthy.sdesign")),
    ("Resume - Retro 70s", include_str!("../templates/resume-retro-70s.sdesign")),
    ("Resume - Dark Luxe Gold", include_str!("../templates/resume-dark-luxe.sdesign")),
    ("Resume - Brutalist", include_str!("../templates/resume-brutalist.sdesign")),
    ("Resume - Ocean", include_str!("../templates/resume-ocean.sdesign")),
    ("Resume - Sunset", include_str!("../templates/resume-sunset.sdesign")),
    ("Newsletter Header - Minimal Mono", include_str!("../templates/newsletter-header-minimal-mono.sdesign")),
    ("Newsletter Header - Neon Night", include_str!("../templates/newsletter-header-neon-night.sdesign")),
    ("Newsletter Header - Pastel", include_str!("../templates/newsletter-header-pastel.sdesign")),
    ("Newsletter Header - Corporate Navy", include_str!("../templates/newsletter-header-corporate-navy.sdesign")),
    ("Newsletter Header - Earthy", include_str!("../templates/newsletter-header-earthy.sdesign")),
    ("Newsletter Header - Retro 70s", include_str!("../templates/newsletter-header-retro-70s.sdesign")),
    ("Newsletter Header - Dark Luxe Gold", include_str!("../templates/newsletter-header-dark-luxe.sdesign")),
    ("Newsletter Header - Brutalist", include_str!("../templates/newsletter-header-brutalist.sdesign")),
    ("Newsletter Header - Ocean", include_str!("../templates/newsletter-header-ocean.sdesign")),
    ("Newsletter Header - Sunset", include_str!("../templates/newsletter-header-sunset.sdesign")),
    ("Login Screen - Minimal Mono", include_str!("../templates/login-minimal-mono.sdesign")),
    ("Login Screen - Neon Night", include_str!("../templates/login-neon-night.sdesign")),
    ("Login Screen - Pastel", include_str!("../templates/login-pastel.sdesign")),
    ("Login Screen - Corporate Navy", include_str!("../templates/login-corporate-navy.sdesign")),
    ("Login Screen - Earthy", include_str!("../templates/login-earthy.sdesign")),
    ("Login Screen - Retro 70s", include_str!("../templates/login-retro-70s.sdesign")),
    ("Login Screen - Dark Luxe Gold", include_str!("../templates/login-dark-luxe.sdesign")),
    ("Login Screen - Brutalist", include_str!("../templates/login-brutalist.sdesign")),
    ("Login Screen - Ocean", include_str!("../templates/login-ocean.sdesign")),
    ("Login Screen - Sunset", include_str!("../templates/login-sunset.sdesign")),
    ("404 Page - Minimal Mono", include_str!("../templates/not-found-minimal-mono.sdesign")),
    ("404 Page - Neon Night", include_str!("../templates/not-found-neon-night.sdesign")),
    ("404 Page - Pastel", include_str!("../templates/not-found-pastel.sdesign")),
    ("404 Page - Corporate Navy", include_str!("../templates/not-found-corporate-navy.sdesign")),
    ("404 Page - Earthy", include_str!("../templates/not-found-earthy.sdesign")),
    ("404 Page - Retro 70s", include_str!("../templates/not-found-retro-70s.sdesign")),
    ("404 Page - Dark Luxe Gold", include_str!("../templates/not-found-dark-luxe.sdesign")),
    ("404 Page - Brutalist", include_str!("../templates/not-found-brutalist.sdesign")),
    ("404 Page - Ocean", include_str!("../templates/not-found-ocean.sdesign")),
    ("404 Page - Sunset", include_str!("../templates/not-found-sunset.sdesign")),
    ("Product Card - Minimal Mono", include_str!("../templates/product-card-minimal-mono.sdesign")),
    ("Product Card - Neon Night", include_str!("../templates/product-card-neon-night.sdesign")),
    ("Product Card - Pastel", include_str!("../templates/product-card-pastel.sdesign")),
    ("Product Card - Corporate Navy", include_str!("../templates/product-card-corporate-navy.sdesign")),
    ("Product Card - Earthy", include_str!("../templates/product-card-earthy.sdesign")),
    ("Product Card - Retro 70s", include_str!("../templates/product-card-retro-70s.sdesign")),
    ("Product Card - Dark Luxe Gold", include_str!("../templates/product-card-dark-luxe.sdesign")),
    ("Product Card - Brutalist", include_str!("../templates/product-card-brutalist.sdesign")),
    ("Product Card - Ocean", include_str!("../templates/product-card-ocean.sdesign")),
    ("Product Card - Sunset", include_str!("../templates/product-card-sunset.sdesign")),
    ("Event Poster - Minimal Mono", include_str!("../templates/event-poster-minimal-mono.sdesign")),
    ("Event Poster - Neon Night", include_str!("../templates/event-poster-neon-night.sdesign")),
    ("Event Poster - Pastel", include_str!("../templates/event-poster-pastel.sdesign")),
    ("Event Poster - Corporate Navy", include_str!("../templates/event-poster-corporate-navy.sdesign")),
    ("Event Poster - Earthy", include_str!("../templates/event-poster-earthy.sdesign")),
    ("Event Poster - Retro 70s", include_str!("../templates/event-poster-retro-70s.sdesign")),
    ("Event Poster - Dark Luxe Gold", include_str!("../templates/event-poster-dark-luxe.sdesign")),
    ("Event Poster - Brutalist", include_str!("../templates/event-poster-brutalist.sdesign")),
    ("Event Poster - Ocean", include_str!("../templates/event-poster-ocean.sdesign")),
    ("Event Poster - Sunset", include_str!("../templates/event-poster-sunset.sdesign")),
    ("Title Slide - Minimal Mono", include_str!("../templates/title-slide-minimal-mono.sdesign")),
    ("Title Slide - Neon Night", include_str!("../templates/title-slide-neon-night.sdesign")),
    ("Title Slide - Pastel", include_str!("../templates/title-slide-pastel.sdesign")),
    ("Title Slide - Corporate Navy", include_str!("../templates/title-slide-corporate-navy.sdesign")),
    ("Title Slide - Earthy", include_str!("../templates/title-slide-earthy.sdesign")),
    ("Title Slide - Retro 70s", include_str!("../templates/title-slide-retro-70s.sdesign")),
    ("Title Slide - Dark Luxe Gold", include_str!("../templates/title-slide-dark-luxe.sdesign")),
    ("Title Slide - Brutalist", include_str!("../templates/title-slide-brutalist.sdesign")),
    ("Title Slide - Ocean", include_str!("../templates/title-slide-ocean.sdesign")),
    ("Title Slide - Sunset", include_str!("../templates/title-slide-sunset.sdesign")),
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

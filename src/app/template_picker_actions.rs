//! Applies the template picker's `TemplatePick`s (see `ui/template_picker/`) — same "screen
//! returns picks, `App` applies them" convention as `layer_actions`.

use crate::io;
use crate::system_launch;
use crate::templates::TemplateSource;
use crate::ui::template_picker::TemplatePick;

use super::App;

impl App {
    pub(super) fn apply_template_pick(&mut self, pick: TemplatePick) {
        match pick {
            TemplatePick::Open(index) => {
                let template = self.template_picker.template(index).cloned();
                self.template_picker.close();
                if let Some(template) = template {
                    self.new_from_template(&template);
                }
            }
            TemplatePick::Reveal(index) => {
                if let Some(TemplateSource::User(path)) = self.template_picker.template(index).map(|t| &t.source)
                    && let Err(err) = system_launch::show_in_file_manager(path)
                {
                    io::show_error("Show Template", &format!("Couldn't show {}: {err}", path.display()));
                }
            }
            TemplatePick::Delete(index) => self.delete_user_template(index),
            TemplatePick::Close => self.template_picker.close(),
        }
    }

    /// Deletes the user template's file after a yes/no confirmation, then relists the
    /// templates. Built-in templates have no file and are left alone.
    fn delete_user_template(&mut self, index: usize) {
        let Some(template) = self.template_picker.template(index) else { return };
        let TemplateSource::User(path) = &template.source else { return };
        let confirmed = rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title("Delete Template")
            .set_description(format!("Delete the template \"{}\"? Its file is removed from disk.", template.name))
            .set_buttons(rfd::MessageButtons::YesNo)
            .show();
        if confirmed != rfd::MessageDialogResult::Yes {
            return;
        }
        if let Err(err) = std::fs::remove_file(path) {
            io::show_error("Delete Template", &format!("Couldn't delete {}: {err}", path.display()));
        }
        self.template_picker.open();
    }
}

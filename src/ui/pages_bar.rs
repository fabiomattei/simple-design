use egui::Ui;

use crate::model::Page;

pub enum PagesAction {
    Select(usize),
    Add,
    Rename(usize, String),
    Delete(usize),
}

#[derive(Default)]
pub struct PagesBar {
    renaming: Option<(usize, String)>,
    /// Set alongside `renaming` when it's first entered; consumed (and
    /// cleared) the next time the rename field is drawn, so focus is
    /// requested exactly once. Requesting it every frame would re-grab focus
    /// right after `TextEdit`'s own Enter-triggered `surrender_focus`, which
    /// makes `lost_focus()` (checked immediately after, in the same frame)
    /// read as still-focused — silently swallowing both Enter and click-away.
    renaming_needs_focus: bool,
}

impl PagesBar {
    pub fn ui(&mut self, ui: &mut Ui, pages: &[Page], active: usize) -> Option<PagesAction> {
        let mut action = None;
        ui.horizontal(|ui| {
            for (i, page) in pages.iter().enumerate() {
                let renaming_this = matches!(&self.renaming, Some((idx, _)) if *idx == i);
                if renaming_this {
                    let (_, buf) = self.renaming.as_mut().unwrap();
                    let resp = ui.text_edit_singleline(buf);
                    if self.renaming_needs_focus {
                        resp.request_focus();
                        self.renaming_needs_focus = false;
                    }
                    if resp.lost_focus() {
                        let (idx, buf) = self.renaming.take().unwrap();
                        action = Some(PagesAction::Rename(idx, buf));
                    }
                } else {
                    let resp = ui.selectable_label(i == active, &page.name);
                    if resp.clicked() {
                        action = Some(PagesAction::Select(i));
                    }
                    if resp.double_clicked() {
                        self.renaming = Some((i, page.name.clone()));
                        self.renaming_needs_focus = true;
                    }
                    resp.context_menu(|ui| {
                        if ui.button("Rename").clicked() {
                            self.renaming = Some((i, page.name.clone()));
                            self.renaming_needs_focus = true;
                            ui.close();
                        }
                        if ui
                            .add_enabled(pages.len() > 1, egui::Button::new("Delete"))
                            .clicked()
                        {
                            action = Some(PagesAction::Delete(i));
                            ui.close();
                        }
                    });
                }
                ui.separator();
            }
            if ui.button("+").on_hover_text("Add page").clicked() {
                action = Some(PagesAction::Add);
            }
        });
        action
    }
}

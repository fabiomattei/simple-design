#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    Pan,
    Artboard,
    Rectangle,
    Oval,
    Line,
    Arrow,
    Star,
    Polygon,
    Pen,
    Text,
    Scissors,
}

impl Tool {
    pub const ALL: [Tool; 12] = [
        Tool::Select,
        Tool::Pan,
        Tool::Artboard,
        Tool::Rectangle,
        Tool::Oval,
        Tool::Line,
        Tool::Arrow,
        Tool::Star,
        Tool::Polygon,
        Tool::Pen,
        Tool::Text,
        Tool::Scissors,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            Tool::Select => "Select (V)",
            Tool::Pan => "Pan (H)",
            Tool::Artboard => "Artboard (A)",
            Tool::Rectangle => "Rectangle (R)",
            Tool::Oval => "Oval (O)",
            Tool::Line => "Line (L)",
            Tool::Arrow => "Arrow (W)",
            Tool::Star => "Star (S)",
            Tool::Polygon => "Polygon (G)",
            Tool::Pen => "Pen (P)",
            Tool::Text => "Text (T)",
            Tool::Scissors => "Scissors (C)",
        }
    }

    /// Keyboard shortcut key for this tool, if any.
    pub fn shortcut(key: &egui::Key) -> Option<Tool> {
        match key {
            egui::Key::V => Some(Tool::Select),
            egui::Key::H => Some(Tool::Pan),
            egui::Key::A => Some(Tool::Artboard),
            egui::Key::R => Some(Tool::Rectangle),
            egui::Key::O => Some(Tool::Oval),
            egui::Key::L => Some(Tool::Line),
            egui::Key::W => Some(Tool::Arrow),
            egui::Key::S => Some(Tool::Star),
            egui::Key::G => Some(Tool::Polygon),
            egui::Key::P => Some(Tool::Pen),
            egui::Key::T => Some(Tool::Text),
            egui::Key::C => Some(Tool::Scissors),
            _ => None,
        }
    }
}

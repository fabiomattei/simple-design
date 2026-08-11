use serde::{Deserialize, Serialize};

/// Whether a guide is a horizontal line (dragged out of the top ruler, fixed
/// at a `y` in doc/page space) or a vertical one (dragged out of the left
/// ruler, fixed at an `x`).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum GuideOrientation {
    Horizontal,
    Vertical,
}

/// A user-placed ruler guide, in page space (not relative to any layer).
/// `pos` is the guide's `y` for `Horizontal`, `x` for `Vertical`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct Guide {
    pub orientation: GuideOrientation,
    pub pos: f32,
}

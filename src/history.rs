use crate::model::Document;

/// Snapshot-based undo/redo. Simpler and less error-prone than a command
/// pattern; document sizes in a design tool are small enough that cloning
/// on each committed edit is cheap.
pub struct History {
    current: Document,
    past: Vec<Document>,
    future: Vec<Document>,
}

const MAX_HISTORY: usize = 200;

impl History {
    pub fn new(document: Document) -> Self {
        Self {
            current: document,
            past: Vec::new(),
            future: Vec::new(),
        }
    }

    pub fn get(&self) -> &Document {
        &self.current
    }

    /// Call before mutating `current` in a way that should be undoable.
    pub fn snapshot(&mut self) {
        self.past.push(self.current.clone());
        if self.past.len() > MAX_HISTORY {
            self.past.remove(0);
        }
        self.future.clear();
    }

    pub fn mutate(&mut self) -> &mut Document {
        &mut self.current
    }

    pub fn undo(&mut self) {
        if let Some(prev) = self.past.pop() {
            self.future.push(std::mem::replace(&mut self.current, prev));
        }
    }

    pub fn redo(&mut self) {
        if let Some(next) = self.future.pop() {
            self.past.push(std::mem::replace(&mut self.current, next));
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }

    pub fn replace(&mut self, document: Document) {
        self.current = document;
        self.past.clear();
        self.future.clear();
    }
}

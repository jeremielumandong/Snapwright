//! Undo and redo as whole-scene snapshots: scenes are a few kilobytes, so copies are cheaper
//! than getting inverse operations right for every edit.

use crate::scene::Scene;

/// Steps kept for undo; the oldest are dropped beyond this.
const LIMIT: usize = 200;

#[derive(Debug, Default)]
pub struct History {
    undo: Vec<Scene>,
    redo: Vec<Scene>,
}

impl History {
    /// Records `before`, the scene as it was just before an edit. Clears the redo stack.
    pub fn record(&mut self, before: Scene) {
        if self.undo.last() == Some(&before) {
            return;
        }
        self.undo.push(before);
        if self.undo.len() > LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Steps `current` back one edit. Returns whether there was one.
    pub fn undo(&mut self, current: &mut Scene) -> bool {
        match self.undo.pop() {
            Some(prev) => {
                self.redo.push(std::mem::replace(current, prev));
                true
            }
            None => false,
        }
    }

    /// Re-applies the last undone edit. Returns whether there was one.
    pub fn redo(&mut self, current: &mut Scene) -> bool {
        match self.redo.pop() {
            Some(next) => {
                self.undo.push(std::mem::replace(current, next));
                true
            }
            None => false,
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

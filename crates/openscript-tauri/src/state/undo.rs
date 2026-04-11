use serde_json::Value;
use std::collections::VecDeque;

const MAX_UNDO_DEPTH: usize = 50;

#[derive(Debug, Clone)]
pub struct UndoEntry {
    pub description: String,
    pub before: Value,
    pub after: Value,
}

pub struct UndoManager {
    undo_stack: VecDeque<UndoEntry>,
    redo_stack: VecDeque<UndoEntry>,
    max_depth: usize,
}

impl UndoManager {
    pub fn new() -> Self {
        Self {
            undo_stack: VecDeque::with_capacity(MAX_UNDO_DEPTH),
            redo_stack: VecDeque::with_capacity(MAX_UNDO_DEPTH),
            max_depth: MAX_UNDO_DEPTH,
        }
    }

    pub fn record(&mut self, description: String, before: Value, after: Value) {
        let entry = UndoEntry {
            description,
            before,
            after,
        };
        self.undo_stack.push_back(entry);
        self.redo_stack.clear();
        if self.undo_stack.len() > self.max_depth {
            self.undo_stack.pop_front();
        }
    }

    pub fn undo(&mut self) -> Option<(String, Value)> {
        self.undo_stack.pop_back().map(|entry| {
            self.redo_stack.push_back(UndoEntry {
                description: entry.description.clone(),
                before: entry.before.clone(),
                after: entry.after.clone(),
            });
            (entry.description, entry.before)
        })
    }

    pub fn redo(&mut self) -> Option<(String, Value)> {
        self.redo_stack.pop_back().map(|entry| {
            self.undo_stack.push_back(UndoEntry {
                description: entry.description.clone(),
                before: entry.before.clone(),
                after: entry.after.clone(),
            });
            (entry.description, entry.after)
        })
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub fn clear(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
    }

    pub fn undo_count(&self) -> usize {
        self.undo_stack.len()
    }

    pub fn redo_count(&self) -> usize {
        self.redo_stack.len()
    }
}

impl Default for UndoManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn val(n: i32) -> Value {
        Value::Number(n.into())
    }

    #[test]
    fn test_undo_redo_basic() {
        let mut mgr = UndoManager::new();
        mgr.record("op1".into(), val(1), val(2));
        mgr.record("op2".into(), val(2), val(3));
        assert!(mgr.can_undo());
        assert!(!mgr.can_redo());
        let (desc, state) = mgr.undo().unwrap();
        assert_eq!(desc, "op2");
        assert_eq!(state, val(2));
        assert!(mgr.can_redo());
        let (_, state) = mgr.redo().unwrap();
        assert_eq!(state, val(3));
    }

    #[test]
    fn test_new_operation_clears_redo() {
        let mut mgr = UndoManager::new();
        mgr.record("op1".into(), val(1), val(2));
        mgr.record("op2".into(), val(2), val(3));
        mgr.undo();
        assert!(mgr.can_redo());
        mgr.record("op3".into(), val(2), val(4));
        assert!(!mgr.can_redo());
    }

    #[test]
    fn test_max_depth() {
        let mut mgr = UndoManager::new();
        for i in 0..60 {
            mgr.record(format!("op {}", i), val(i), val(i + 1));
        }
        assert_eq!(mgr.undo_count(), 50);
    }
}

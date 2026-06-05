use std::collections::VecDeque;
use super::cell::Cell;

/// A fixed-capacity scrollback buffer storing rows pushed off the top of the grid.
pub struct ScrollbackBuffer {
    rows: VecDeque<Vec<Cell>>,
    capacity: usize,
}

impl ScrollbackBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            rows: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn push(&mut self, row: Vec<Cell>) {
        if self.rows.len() == self.capacity {
            self.rows.pop_front();
        }
        self.rows.push_back(row);
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Returns the row at `offset` lines above the bottom (0 = most recent).
    pub fn get_from_bottom(&self, offset: usize) -> Option<&Vec<Cell>> {
        let idx = self.rows.len().checked_sub(offset + 1)?;
        self.rows.get(idx)
    }
}

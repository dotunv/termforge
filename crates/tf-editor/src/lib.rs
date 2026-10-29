//! Framework-free editor buffers, transactions and durable file persistence.

mod position;
mod syntax;

pub use position::{byte_to_position, position_to_byte, PositionError, TextPosition};
pub use syntax::{highlight, HighlightKind, HighlightSpan, Language};

use std::collections::hash_map::DefaultHasher;
use std::fs::{self, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::{self, Read, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const MAX_BUFFER_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Snapshot {
    text: String,
    selection: Range<usize>,
    anchor: usize,
    cursor: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Movement {
    Left,
    Right,
    Up,
    Down,
    LineStart,
    LineEnd,
    DocumentStart,
    DocumentEnd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DiskStamp {
    fingerprint: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalChange {
    Unchanged,
    Changed,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    Io(String),
    TooLarge(u64),
    NotUtf8,
}

#[derive(Debug, Clone)]
pub struct Buffer {
    path: PathBuf,
    text: String,
    selection: Range<usize>,
    anchor: usize,
    cursor: usize,
    preferred_column: Option<usize>,
    saved_fingerprint: u64,
    disk_stamp: DiskStamp,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    revision: u64,
}

impl Buffer {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, OpenError> {
        let path = path.into();
        let metadata = fs::metadata(&path).map_err(|error| OpenError::Io(error.to_string()))?;
        if metadata.len() > MAX_BUFFER_BYTES {
            return Err(OpenError::TooLarge(metadata.len()));
        }
        let file = fs::File::open(&path).map_err(|error| OpenError::Io(error.to_string()))?;
        let mut bytes = Vec::new();
        file.take(MAX_BUFFER_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| OpenError::Io(error.to_string()))?;
        if bytes.len() as u64 > MAX_BUFFER_BYTES {
            return Err(OpenError::TooLarge(bytes.len() as u64));
        }
        let text = String::from_utf8(bytes).map_err(|_| OpenError::NotUtf8)?;
        let fingerprint = fingerprint(&text);
        Ok(Self {
            path,
            text,
            selection: 0..0,
            anchor: 0,
            cursor: 0,
            preferred_column: None,
            saved_fingerprint: fingerprint,
            disk_stamp: DiskStamp { fingerprint },
            undo: Vec::new(),
            redo: Vec::new(),
            revision: 0,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn selection(&self) -> Range<usize> {
        self.selection.clone()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn is_dirty(&self) -> bool {
        fingerprint(&self.text) != self.saved_fingerprint
    }

    /// Incorporate a successful save of an earlier snapshot without replacing
    /// edits or undo history made while that save was in flight.
    pub fn apply_saved_snapshot(&mut self, saved: &Self) -> bool {
        if self.path != saved.path || saved.is_dirty() {
            return false;
        }
        self.saved_fingerprint = saved.saved_fingerprint;
        self.disk_stamp = saved.disk_stamp;
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn set_selection(&mut self, selection: Range<usize>) -> bool {
        if selection.start > selection.end
            || selection.end > self.text.len()
            || !self.text.is_char_boundary(selection.start)
            || !self.text.is_char_boundary(selection.end)
        {
            return false;
        }
        self.anchor = selection.start;
        self.cursor = selection.end;
        self.selection = selection;
        self.preferred_column = None;
        true
    }

    /// Move by Unicode scalar values while keeping a stable anchor for Shift
    /// selections and a preferred column across shorter intermediate lines.
    pub fn move_cursor(&mut self, movement: Movement, extend: bool) {
        let text = &self.text;
        let cursor = self.cursor;
        let line_start = text[..cursor].rfind('\n').map_or(0, |index| index + 1);
        let line_end = text[cursor..]
            .find('\n')
            .map_or(text.len(), |index| cursor + index);
        let vertical = matches!(movement, Movement::Up | Movement::Down);
        let column = self
            .preferred_column
            .unwrap_or_else(|| text[line_start..cursor].chars().count());
        let target = match movement {
            Movement::Left if !extend && !self.selection.is_empty() => self.selection.start,
            Movement::Right if !extend && !self.selection.is_empty() => self.selection.end,
            Movement::Left => text[..cursor]
                .char_indices()
                .next_back()
                .map_or(0, |(index, _)| index),
            Movement::Right => text[cursor..]
                .chars()
                .next()
                .map_or(cursor, |character| cursor + character.len_utf8()),
            Movement::LineStart => line_start,
            Movement::LineEnd => line_end,
            Movement::DocumentStart => 0,
            Movement::DocumentEnd => text.len(),
            Movement::Up if line_start > 0 => {
                let end = line_start - 1;
                let start = text[..end].rfind('\n').map_or(0, |index| index + 1);
                start
                    + text[start..end]
                        .char_indices()
                        .nth(column)
                        .map_or(end - start, |(index, _)| index)
            }
            Movement::Down if line_end < text.len() => {
                let start = line_end + 1;
                let end = text[start..]
                    .find('\n')
                    .map_or(text.len(), |index| start + index);
                start
                    + text[start..end]
                        .char_indices()
                        .nth(column)
                        .map_or(end - start, |(index, _)| index)
            }
            Movement::Up | Movement::Down => cursor,
        };
        if !extend {
            self.anchor = target;
        }
        self.cursor = target;
        self.selection = self.anchor.min(target)..self.anchor.max(target);
        self.preferred_column = vertical.then_some(column);
    }

    pub fn replace_selection(&mut self, replacement: &str) {
        let before = self.snapshot();
        let range = self.selection.clone();
        self.text.replace_range(range.clone(), replacement);
        let cursor = range.start + replacement.len();
        self.selection = cursor..cursor;
        self.anchor = cursor;
        self.cursor = cursor;
        self.preferred_column = None;
        self.undo.push(before);
        self.redo.clear();
        self.revision = self.revision.saturating_add(1);
    }

    pub fn delete_backward(&mut self) -> bool {
        if self.selection.start != self.selection.end {
            self.replace_selection("");
            return true;
        }
        let cursor = self.selection.start;
        let Some(previous) = self.text[..cursor]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
        else {
            return false;
        };
        self.set_selection(previous..cursor);
        self.replace_selection("");
        true
    }

    pub fn delete_forward(&mut self) -> bool {
        if !self.selection.is_empty() {
            self.replace_selection("");
            return true;
        }
        let cursor = self.selection.start;
        let Some(character) = self.text[cursor..].chars().next() else {
            return false;
        };
        self.set_selection(cursor..cursor + character.len_utf8());
        self.replace_selection("");
        true
    }

    pub fn undo(&mut self) -> bool {
        let Some(snapshot) = self.undo.pop() else {
            return false;
        };
        self.redo.push(self.snapshot());
        self.restore(snapshot);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(snapshot) = self.redo.pop() else {
            return false;
        };
        self.undo.push(self.snapshot());
        self.restore(snapshot);
        true
    }

    pub fn find_all(&self, query: &str, case_sensitive: bool) -> Vec<Range<usize>> {
        if query.is_empty() {
            return Vec::new();
        }
        if case_sensitive {
            return self
                .text
                .match_indices(query)
                .map(|(start, value)| start..start + value.len())
                .collect();
        }
        // Lowercasing can expand a character or change its UTF-8 byte length.
        // Record only original character boundaries, so matches always select
        // complete characters in the original buffer.
        let mut folded = String::with_capacity(self.text.len());
        let mut boundaries = Vec::new();
        for (offset, character) in self.text.char_indices() {
            boundaries.push((folded.len(), offset));
            folded.extend(character.to_lowercase());
        }
        boundaries.push((folded.len(), self.text.len()));
        let needle: String = query.chars().flat_map(char::to_lowercase).collect();
        folded
            .match_indices(&needle)
            .filter_map(|(start, value)| {
                let end = start + value.len();
                let start_index = boundaries
                    .binary_search_by_key(&start, |&(folded, _)| folded)
                    .ok()?;
                let end_index = boundaries
                    .binary_search_by_key(&end, |&(folded, _)| folded)
                    .ok()?;
                Some(boundaries[start_index].1..boundaries[end_index].1)
            })
            .collect()
    }

    pub fn external_change(&self) -> ExternalChange {
        let file = match fs::File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return ExternalChange::Deleted
            }
            Err(_) => return ExternalChange::Changed,
        };
        let mut bytes = Vec::new();
        if file
            .take(MAX_BUFFER_BYTES + 1)
            .read_to_end(&mut bytes)
            .is_err()
            || bytes.len() as u64 > MAX_BUFFER_BYTES
        {
            return ExternalChange::Changed;
        }
        match String::from_utf8(bytes) {
            Ok(text) if fingerprint(&text) == self.disk_stamp.fingerprint => {
                ExternalChange::Unchanged
            }
            _ => ExternalChange::Changed,
        }
    }

    pub fn save_atomic(&mut self) -> io::Result<()> {
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        let name = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("buffer");
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let temporary = parent.join(format!(".{name}.termforge-{nonce}.tmp"));
        let result: io::Result<()> = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(self.text.as_bytes())?;
            file.sync_all()?;
            if let Ok(metadata) = fs::metadata(&self.path) {
                fs::set_permissions(&temporary, metadata.permissions())?;
            }
            replace_file(&temporary, &self.path)?;
            if let Ok(directory) = fs::File::open(parent) {
                let _ = directory.sync_all();
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result?;
        let fingerprint = fingerprint(&self.text);
        self.saved_fingerprint = fingerprint;
        self.disk_stamp = DiskStamp { fingerprint };
        Ok(())
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            text: self.text.clone(),
            selection: self.selection.clone(),
            anchor: self.anchor,
            cursor: self.cursor,
        }
    }

    fn restore(&mut self, snapshot: Snapshot) {
        self.text = snapshot.text;
        self.selection = snapshot.selection;
        self.anchor = snapshot.anchor;
        self.cursor = snapshot.cursor;
        self.preferred_column = None;
        self.revision = self.revision.saturating_add(1);
    }
}

fn fingerprint(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    let backup = destination.with_extension("termforge-save-backup");
    let _ = fs::remove_file(&backup);
    if destination.exists() {
        fs::rename(destination, &backup)?;
    }
    if let Err(error) = fs::rename(source, destination) {
        let _ = fs::rename(&backup, destination);
        return Err(error);
    }
    let _ = fs::remove_file(backup);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transactions_undo_redo_and_dirty_state_round_trip() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("file.rs");
        fs::write(&path, "hello").expect("fixture");
        let mut buffer = Buffer::open(&path).expect("open buffer");
        assert!(buffer.set_selection(5..5));
        buffer.replace_selection(" world");
        assert_eq!(buffer.text(), "hello world");
        assert!(buffer.is_dirty());
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "hello");
        assert!(!buffer.is_dirty());
        assert!(buffer.redo());
        assert_eq!(buffer.text(), "hello world");
        buffer.save_atomic().expect("atomic save");
        assert!(!buffer.is_dirty());
        assert_eq!(fs::read_to_string(path).unwrap(), "hello world");
    }

    #[test]
    fn applying_saved_snapshot_preserves_newer_edits_and_undo() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("file.txt");
        fs::write(&path, "one").unwrap();
        let mut buffer = Buffer::open(&path).unwrap();
        buffer.set_selection(3..3);
        buffer.replace_selection(" two");
        let mut saved = buffer.clone();
        buffer.replace_selection(" three");
        saved.save_atomic().unwrap();
        assert!(buffer.apply_saved_snapshot(&saved));
        assert_eq!(buffer.text(), "one two three");
        assert!(buffer.is_dirty());
        assert_eq!(buffer.external_change(), ExternalChange::Unchanged);
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "one two");
        assert!(!buffer.is_dirty());
    }

    #[test]
    fn selection_rejects_non_boundaries_and_delete_handles_unicode() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("unicode.txt");
        fs::write(&path, "a🙂").expect("fixture");
        let mut buffer = Buffer::open(path).expect("open buffer");
        assert!(!buffer.set_selection(2..2));
        assert!(buffer.set_selection(buffer.text().len()..buffer.text().len()));
        assert!(buffer.delete_backward());
        assert_eq!(buffer.text(), "a");
    }

    #[test]
    fn movement_preserves_anchor_and_column_across_unicode_lines() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("movement.txt");
        fs::write(&path, "a🙂bc\nx\nαβγδ\n").unwrap();
        let mut buffer = Buffer::open(path).unwrap();
        buffer.set_selection(6..6);
        buffer.move_cursor(Movement::Down, false);
        assert_eq!(buffer.cursor(), 9);
        buffer.move_cursor(Movement::Down, false);
        assert_eq!(&buffer.text()[buffer.cursor()..], "δ\n");
        buffer.move_cursor(Movement::Up, false);
        assert_eq!(buffer.cursor(), 9);
        buffer.move_cursor(Movement::Up, false);
        assert_eq!(buffer.cursor(), 6);
        buffer.move_cursor(Movement::Left, true);
        buffer.move_cursor(Movement::Left, true);
        assert_eq!(&buffer.text()[buffer.selection()], "🙂b");
        assert_eq!(buffer.cursor(), 1);
        buffer.move_cursor(Movement::Right, true);
        assert_eq!(&buffer.text()[buffer.selection()], "b");
        buffer.move_cursor(Movement::Right, false);
        assert_eq!(buffer.cursor(), 6);
        assert!(buffer.selection().is_empty());
        buffer.move_cursor(Movement::DocumentEnd, false);
        assert_eq!(buffer.cursor(), buffer.text().len());
        buffer.move_cursor(Movement::DocumentStart, false);
        assert_eq!(buffer.cursor(), 0);
    }

    #[test]
    fn forward_delete_handles_unicode_and_selections() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("delete.txt");
        fs::write(&path, "a🙂bc").unwrap();
        let mut buffer = Buffer::open(path).unwrap();
        buffer.set_selection(1..1);
        assert!(buffer.delete_forward());
        assert_eq!(buffer.text(), "abc");
        buffer.set_selection(1..3);
        assert!(buffer.delete_forward());
        assert_eq!(buffer.text(), "a");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "abc");
    }

    #[test]
    fn external_change_detects_edits_and_deletion() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("watched.txt");
        fs::write(&path, "one").expect("fixture");
        let buffer = Buffer::open(&path).expect("open buffer");
        fs::write(&path, "different length").expect("external edit");
        assert_eq!(buffer.external_change(), ExternalChange::Changed);
        fs::remove_file(path).expect("external delete");
        assert_eq!(buffer.external_change(), ExternalChange::Deleted);
    }

    #[test]
    fn search_returns_byte_ranges() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("search.txt");
        fs::write(&path, "One one none").expect("fixture");
        let buffer = Buffer::open(path).expect("open buffer");
        assert_eq!(buffer.find_all("one", false), vec![0..3, 4..7, 9..12]);
        assert_eq!(buffer.find_all("One", true), vec![0..3]);
    }

    #[test]
    fn case_insensitive_search_returns_original_unicode_boundaries() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("unicode.txt");
        fs::write(&path, "İx KELVIN 🙂 Kelvin").unwrap();
        let mut buffer = Buffer::open(path).unwrap();
        let matches = buffer.find_all("kelvin", false);
        assert_eq!(matches.len(), 2);
        for range in matches {
            assert!(buffer.set_selection(range.clone()));
            assert!(buffer.text()[range].to_lowercase() == "kelvin");
        }
        assert!(
            buffer
                .find_all("i", false)
                .iter()
                .all(|range| range.start != 0),
            "a match cannot split a lowercase expansion"
        );
        let matches = buffer.find_all("i\u{307}x", false);
        assert_eq!(&buffer.text()[matches[0].clone()], "İx");
    }

    #[test]
    fn external_edit_with_same_size_and_timestamp_is_detected() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("watched.txt");
        fs::write(&path, "one").unwrap();
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        let buffer = Buffer::open(&path).unwrap();
        fs::write(&path, "two").unwrap();
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        assert_eq!(buffer.external_change(), ExternalChange::Changed);
    }
}

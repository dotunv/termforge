use std::fmt;

/// A Language Server Protocol position: zero-based line and UTF-16 column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TextPosition {
    pub line: u32,
    pub utf16_column: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionError {
    ByteOutOfBounds,
    NotCharBoundary,
    LineOutOfBounds,
    ColumnOutOfBounds,
    InsideSurrogatePair,
}

impl fmt::Display for PositionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ByteOutOfBounds => "byte offset is outside the document",
            Self::NotCharBoundary => "byte offset is not a UTF-8 character boundary",
            Self::LineOutOfBounds => "line is outside the document",
            Self::ColumnOutOfBounds => "UTF-16 column is outside the line",
            Self::InsideSurrogatePair => "UTF-16 column splits a surrogate pair",
        })
    }
}

impl std::error::Error for PositionError {}

pub fn byte_to_position(text: &str, byte: usize) -> Result<TextPosition, PositionError> {
    if byte > text.len() {
        return Err(PositionError::ByteOutOfBounds);
    }
    if !text.is_char_boundary(byte) {
        return Err(PositionError::NotCharBoundary);
    }
    let before = &text[..byte];
    let line = before.bytes().filter(|byte| *byte == b'\n').count();
    let line_start = before.rfind('\n').map_or(0, |index| index + 1);
    Ok(TextPosition {
        line: u32::try_from(line).unwrap_or(u32::MAX),
        utf16_column: u32::try_from(text[line_start..byte].encode_utf16().count())
            .unwrap_or(u32::MAX),
    })
}

pub fn position_to_byte(text: &str, position: TextPosition) -> Result<usize, PositionError> {
    let mut line = 0_u32;
    let mut line_start = 0;
    while line < position.line {
        let Some(relative) = text[line_start..].find('\n') else {
            return Err(PositionError::LineOutOfBounds);
        };
        line_start += relative + 1;
        line += 1;
    }

    let line_end = text[line_start..]
        .find('\n')
        .map_or(text.len(), |relative| line_start + relative);
    let mut utf16_column = 0_u32;
    for (relative, character) in text[line_start..line_end].char_indices() {
        if utf16_column == position.utf16_column {
            return Ok(line_start + relative);
        }
        let width = character.len_utf16() as u32;
        if position.utf16_column < utf16_column + width {
            return Err(PositionError::InsideSurrogatePair);
        }
        utf16_column += width;
    }
    if utf16_column == position.utf16_column {
        Ok(line_end)
    } else {
        Err(PositionError::ColumnOutOfBounds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_and_lsp_positions_round_trip_unicode() {
        let text = "a🙂z\nβeta";
        for byte in text
            .char_indices()
            .map(|(byte, _)| byte)
            .chain(std::iter::once(text.len()))
        {
            let position = byte_to_position(text, byte).unwrap();
            assert_eq!(position_to_byte(text, position), Ok(byte));
        }
        assert_eq!(
            byte_to_position(text, 5),
            Ok(TextPosition {
                line: 0,
                utf16_column: 3
            })
        );
    }

    #[test]
    fn invalid_positions_are_explicit() {
        let text = "🙂\n";
        assert_eq!(
            position_to_byte(
                text,
                TextPosition {
                    line: 0,
                    utf16_column: 1
                }
            ),
            Err(PositionError::InsideSurrogatePair)
        );
        assert_eq!(
            position_to_byte(
                text,
                TextPosition {
                    line: 2,
                    utf16_column: 0
                }
            ),
            Err(PositionError::LineOutOfBounds)
        );
        assert_eq!(
            byte_to_position(text, 1),
            Err(PositionError::NotCharBoundary)
        );
    }
}

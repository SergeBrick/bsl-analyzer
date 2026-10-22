use anyhow::{anyhow, bail, Result};
use ide::TextRange;
use line_index::{LineCol, LineIndex, TextSize};
use lsp_types::{Position, Url};
use vfs::FileId;

use crate::global_state::{GlobalState, GlobalStateSnapshot};
use crate::lsp::PositionEncoding;

pub fn file_id(state: &GlobalState, url: &Url) -> Result<FileId> {
    if let Some(&file_id) = state.open_file_uri_ids.get(url) {
        return Ok(file_id);
    }
    let path = url.to_file_path().map_err(|_| anyhow!("Invalid file URL: {}", url))?;
    let path = if state.analysis_host.owner().is_some() {
        crate::workspace::canonicalize_shared_path(&path)
    } else {
        path
    };

    let vfs_path = vfs::VfsPath::new(path);
    let vfs = state.vfs.read();

    vfs.file_id(&vfs_path).ok_or_else(|| anyhow!("File not in VFS: {}", url))
}

pub fn file_id_snapshot(snapshot: &GlobalStateSnapshot, url: &Url) -> Result<FileId> {
    snapshot.file_id_for_url(url)
}

pub fn offset(line_index: &LineIndex, text: &str, position: Position) -> Result<TextSize> {
    offset_with_encoding(line_index, text, position, PositionEncoding::Utf16)
}

/// What to do with a column past the end of its line.
#[derive(Clone, Copy, PartialEq, Eq)]
enum OverlongColumn {
    /// Cursor positions: LSP 3.17 — "If the character value is greater than the
    /// line length it defaults back to the line length."
    Clamp,
    /// Edit ranges: a clamped range would silently rewrite the wrong span of the
    /// caller's buffer, so the position is refused instead.
    Refuse,
}

/// The byte offset of an LSP position, or an error when the position names no
/// place in `text`. A column past the end of its line is clamped to the line
/// end, as LSP 3.17 requires; only a position that cannot name a place at all
/// (a bad line, a UTF-8 column inside a character) is refused.
///
/// The bounds a caller cannot see for itself are proven here:
/// [`LineIndex::offset`] validates the line only and adds the column to its
/// start unchecked, so a column past the line end has to be cut back before the
/// offset is taken — otherwise it would silently resolve into a later line. The
/// UTF-16 path walks whole characters and cannot split one; the UTF-8 path
/// takes the column as the client counted it and stays refused there.
pub fn offset_with_encoding(
    line_index: &LineIndex,
    text: &str,
    position: Position,
    encoding: PositionEncoding,
) -> Result<TextSize> {
    offset_with_overlong_column(line_index, text, position, encoding, OverlongColumn::Clamp)
}

/// [`offset_with_encoding`] for an edit range (`didChange`): a column past the
/// line end is refused instead of clamped. Clamping an edit would move it to the
/// line end and rewrite a span the caller never named; a cursor that ran past
/// the line, by contrast, means the line end.
pub fn edit_offset_with_encoding(
    line_index: &LineIndex,
    text: &str,
    position: Position,
    encoding: PositionEncoding,
) -> Result<TextSize> {
    offset_with_overlong_column(line_index, text, position, encoding, OverlongColumn::Refuse)
}

fn offset_with_overlong_column(
    line_index: &LineIndex,
    text: &str,
    position: Position,
    encoding: PositionEncoding,
    overlong: OverlongColumn,
) -> Result<TextSize> {
    let line_len = line_index
        .line_len(position.line)
        .ok_or_else(|| anyhow!("line {} is out of bounds", position.line))?;
    // The LSP line length excludes the terminator, but the line index leaves a CRLF
    // line's `\r` inside it. A column past the `\r`
    // would land between `\r` and `\n` — and an edit there splits the terminator.
    let line_len = if line_index
        .safe_line_str(text, position.line)
        .is_some_and(|line| line.ends_with('\r') && position.line + 1 < line_index.len_lines())
    {
        line_len - 1
    } else {
        line_len
    };

    let byte_col = match encoding {
        PositionEncoding::Utf8 => {
            if overlong == OverlongColumn::Clamp {
                position.character.min(line_len)
            } else {
                position.character
            }
        }
        PositionEncoding::Utf16 => {
            let character = if overlong == OverlongColumn::Clamp {
                // The line end is the line's own length in UTF-16 units.
                let line_utf16_len = line_index.utf16_col(text, position.line, line_len);
                position.character.min(line_utf16_len)
            } else {
                position.character
            };
            line_index.utf16_col_to_byte_col(text, position.line, character).ok_or_else(|| {
                anyhow!("UTF-16 column {} is out of bounds on line {}", character, position.line)
            })?
        }
    };

    // The Refuse check: a clamped column is already the line end, so only the
    // edit path can overrun its line here.
    if byte_col > line_len {
        bail!(
            "column {} is out of bounds on line {} with length {} bytes",
            byte_col,
            position.line,
            line_len
        );
    }

    let offset = line_index
        .offset(LineCol { line: position.line, col: byte_col })
        .ok_or_else(|| anyhow!("position {:?} is out of bounds", position))?;

    if !text.is_char_boundary(usize::from(offset)) {
        bail!(
            "position {:?} resolves to non-character boundary byte offset {}",
            position,
            u32::from(offset)
        );
    }

    tracing::trace!(?position, ?encoding, byte_col, ?offset, "from_proto::offset");

    Ok(offset)
}

pub fn text_range(
    line_index: &LineIndex,
    text: &str,
    range: lsp_types::Range,
) -> Result<TextRange> {
    text_range_with_encoding(line_index, text, range, PositionEncoding::Utf16)
}

pub fn text_range_with_encoding(
    line_index: &LineIndex,
    text: &str,
    range: lsp_types::Range,
    encoding: PositionEncoding,
) -> Result<TextRange> {
    let start = offset_with_encoding(line_index, text, range.start, encoding)?;
    let end = offset_with_encoding(line_index, text, range.end, encoding)?;

    Ok(TextRange::new(start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_offset() {
        let text = "hello\nworld\nrust";
        let line_index = LineIndex::new(text);

        let pos = Position { line: 1, character: 0 };
        let result = offset(&line_index, text, pos).unwrap();
        assert_eq!(result, TextSize::from(6));

        let pos = Position { line: 2, character: 0 };
        let result = offset(&line_index, text, pos).unwrap();
        assert_eq!(result, TextSize::from(12));
    }

    #[test]
    fn test_offset_out_of_bounds() {
        let text = "hello";
        let line_index = LineIndex::new(text);

        let pos = Position { line: 10, character: 0 };
        assert!(offset(&line_index, text, pos).is_err());
    }

    #[test]
    fn column_past_a_crlf_line_end_clamps_before_the_carriage_return() {
        let text = "a\r\nb";
        let line_index = LineIndex::new(text);

        // The LSP line length excludes the terminator, but the line index keeps a
        // CRLF line's `\r` inside line 0.
        let pos = Position { line: 0, character: 40 };
        for encoding in [PositionEncoding::Utf8, PositionEncoding::Utf16] {
            let result = offset_with_encoding(&line_index, text, pos, encoding).unwrap();
            assert_eq!(result, TextSize::from(1), "{encoding:?}");
        }
    }

    #[test]
    fn edit_column_on_the_carriage_return_of_a_crlf_line_is_refused() {
        let text = "a\r\nb";
        let line_index = LineIndex::new(text);

        // Column 2 sits between `\r` and `\n`: an insert there would split the
        // line terminator.
        let past = Position { line: 0, character: 2 };
        let end = Position { line: 0, character: 1 };
        for encoding in [PositionEncoding::Utf8, PositionEncoding::Utf16] {
            assert!(edit_offset_with_encoding(&line_index, text, past, encoding).is_err());
            let result = edit_offset_with_encoding(&line_index, text, end, encoding).unwrap();
            assert_eq!(result, TextSize::from(1), "{encoding:?}");
        }
    }

    #[test]
    fn a_lone_carriage_return_at_the_end_of_text_ends_its_line() {
        let text = "a\r";
        let line_index = LineIndex::new(text);

        // A lone `\r` is a line terminator for an LSP client: past it starts an empty
        // line, and the `\r` itself is no column of line 0.
        let after = Position { line: 1, character: 0 };
        let past = Position { line: 0, character: 2 };
        for encoding in [PositionEncoding::Utf8, PositionEncoding::Utf16] {
            let result = edit_offset_with_encoding(&line_index, text, after, encoding).unwrap();
            assert_eq!(result, TextSize::from(2), "{encoding:?}");
            assert!(edit_offset_with_encoding(&line_index, text, past, encoding).is_err());
        }
    }

    #[test]
    fn utf8_column_inside_a_multibyte_char_is_rejected() {
        let text = "Процедура Тест";
        let line_index = LineIndex::new(text);

        // Byte column 1 sits inside 'П' (bytes 0..2). Resolving it would hand
        // every downstream slice an offset no `&str[..]` accepts.
        let pos = Position { line: 0, character: 1 };
        let result = offset_with_encoding(&line_index, text, pos, PositionEncoding::Utf8);

        assert!(result.is_err(), "got {result:?}");
    }

    #[test]
    fn utf8_column_past_the_line_end_clamps_to_the_line_end() {
        let text = "Процедура\nТест";
        let line_index = LineIndex::new(text);

        // Line 0 is 18 bytes long; LSP 3.17 says a character past the line
        // length defaults back to it, so the position lands on the line end
        // instead of spilling into the next line or answering -32603.
        let pos = Position { line: 0, character: 40 };
        let result = offset_with_encoding(&line_index, text, pos, PositionEncoding::Utf8).unwrap();

        assert_eq!(result, TextSize::from(18));
    }

    #[test]
    fn utf16_column_past_the_line_end_clamps_to_the_line_end() {
        let text = "Процедура\nТест";
        let line_index = LineIndex::new(text);

        // The same rule in UTF-16 units: nine characters on line 0, an
        // over-long column defaults back to the ninth.
        let pos = Position { line: 0, character: 40 };
        let result = offset_with_encoding(&line_index, text, pos, PositionEncoding::Utf16).unwrap();

        assert_eq!(result, TextSize::from(18));
    }

    #[test]
    fn edit_column_past_the_line_end_is_refused() {
        let text = "Процедура\nТест";
        let line_index = LineIndex::new(text);

        // An edit range refuses the over-long column: clamping it would rewrite
        // a span the client never named.
        let pos = Position { line: 0, character: 40 };
        assert!(edit_offset_with_encoding(&line_index, text, pos, PositionEncoding::Utf8).is_err());
        assert!(edit_offset_with_encoding(&line_index, text, pos, PositionEncoding::Utf16).is_err());
    }

    #[test]
    fn utf8_byte_column_on_a_boundary_resolves() {
        let text = "Процедура Тест";
        let line_index = LineIndex::new(text);

        let pos = Position { line: 0, character: 18 };
        let result = offset_with_encoding(&line_index, text, pos, PositionEncoding::Utf8).unwrap();

        assert_eq!(result, TextSize::from(18));
    }

    #[test]
    fn test_offset_with_cyrillic() {
        let text = "Процедура Тест";
        let line_index = LineIndex::new(text);

        let pos = Position { line: 0, character: 9 };
        let result = offset(&line_index, text, pos).unwrap();
        assert_eq!(result, TextSize::from(18));

        let pos = Position { line: 0, character: 14 };
        let result = offset(&line_index, text, pos).unwrap();
        assert_eq!(result, TextSize::from(27));
    }
}

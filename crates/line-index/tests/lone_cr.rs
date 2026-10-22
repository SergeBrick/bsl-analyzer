//! A lone `\r` is a line break, as the LSP specification and editors count it; `\r\n`
//! stays one break.

use line_index::{LineCol, LineColRange, LineIndex, TextRange, TextSize};

fn at(text: &str, needle: &str) -> TextSize {
    TextSize::from(text.find(needle).expect("needle in text") as u32)
}

#[test]
fn lone_cr_starts_a_new_line() {
    let text = "А = 1;\rБ = 2;\r\nВ = 3;";
    let index = LineIndex::new(text);

    assert_eq!(index.len_lines(), 3);
    assert_eq!(index.line_col(at(text, "Б")), LineCol { line: 1, col: 0 });
    assert_eq!(index.line_col(at(text, "В")), LineCol { line: 2, col: 0 });
    // The lone `\r` is the terminator of line 0 and is not part of its text.
    assert_eq!(index.safe_line_str(text, 0), Some("А = 1;"));
    assert_eq!(index.safe_line_str(text, 1), Some("Б = 2;\r"));
}

#[test]
fn crlf_is_one_line_break() {
    let text = "А\r\nБ\r\nВ";
    let index = LineIndex::new(text);

    assert_eq!(index.len_lines(), 3);
    assert_eq!(index.line_col(at(text, "Б")), LineCol { line: 1, col: 0 });
    assert_eq!(index.line_col(at(text, "В")), LineCol { line: 2, col: 0 });
}

#[test]
fn cr_before_crlf_is_two_line_breaks() {
    // `\r\r\n`: a lone `\r`, then `\r\n` - an empty line between, as an LSP client shows it.
    let text = "А;\r\r\nБ;\r\r\r\nВ;";
    let index = LineIndex::new(text);

    assert_eq!(index.len_lines(), 6);
    assert_eq!(index.line_col(at(text, "Б")), LineCol { line: 2, col: 0 });
    assert_eq!(index.line_col(at(text, "В")), LineCol { line: 5, col: 0 });
}

#[test]
fn utf16_range_after_lone_cr_counts_from_the_cr() {
    let text = "\tА = 1;\r\tИтог = СтрДлина();\r\n";
    let index = LineIndex::new(text);
    let start = at(text, "СтрДлина");
    let range = TextRange::at(start, TextSize::of("СтрДлина"));

    assert_eq!(
        index.utf16_line_col_range(text, range),
        Some(LineColRange { start_line: 1, start_character: 8, end_line: 1, end_character: 16 })
    );
}

#[test]
fn offsets_round_trip_through_lone_cr_lines() {
    let text = "а\rб\r\r\nв\nг\r";
    let index = LineIndex::new(text);

    for (offset, _) in text.char_indices().chain(std::iter::once((text.len(), ' '))) {
        let offset = TextSize::from(offset as u32);
        assert_eq!(index.offset(index.line_col(offset)), Some(offset), "offset {offset:?}");
    }
}

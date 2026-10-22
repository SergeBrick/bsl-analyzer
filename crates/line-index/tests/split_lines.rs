//! `split_lines` cuts a text where `LineIndex` starts its lines, so a line number from the
//! index picks the text of that line.

use line_index::{split_lines, LineIndex};

#[test]
fn split_lines_yields_the_text_of_each_index_line() {
    let text = "а\rб\r\r\nв\nг\r\nд";
    let index = LineIndex::new(text);
    let lines: Vec<_> = split_lines(text).collect();

    assert_eq!(lines, ["а", "б", "", "в", "г", "д"]);
    assert_eq!(lines.len() as u32, index.len_lines());
    for (line, expected) in lines.iter().enumerate() {
        let got = index.safe_line_str(text, line as u32).unwrap();
        assert_eq!(got.trim_end_matches('\r'), *expected, "line {line}");
    }
}

#[test]
fn split_lines_matches_str_lines_without_lone_cr() {
    for text in ["", "\n", "а", "а\n", "а\r\nб", "а\r\nб\r\n", "\n\nа\n\n", "а\r\n\r\nб"] {
        assert_eq!(
            split_lines(text).collect::<Vec<_>>(),
            text.lines().collect::<Vec<_>>(),
            "{text:?}"
        );
    }
}

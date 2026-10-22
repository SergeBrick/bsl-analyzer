//! Одиночный `\r` (не перед `\n`) заканчивает комментарий `//`: платформа
//! компилирует код, стоящий за ним в той же `\n`-строке. `\r` перед `\n` (CRLF,
//! `\r\r\n`) остаётся в лексеме комментария, как и раньше.

use lexer::{tokenize, TokenKind};

fn kinds_and_texts(input: &str) -> Vec<(TokenKind, String)> {
    tokenize(input).into_iter().map(|token| (token.kind, token.text.to_string())).collect()
}

#[test]
fn lone_cr_ends_line_comment() {
    assert_eq!(
        kinds_and_texts("// к\rА"),
        vec![
            (TokenKind::Comment, "// к".to_owned()),
            (TokenKind::Whitespace, "\r".to_owned()),
            (TokenKind::Ident, "А".to_owned()),
        ]
    );
}

#[test]
fn lone_cr_with_indent_ends_line_comment() {
    let tokens = kinds_and_texts("\t// к\r\tА = 1;");
    assert_eq!(tokens[1], (TokenKind::Comment, "// к".to_owned()));
    assert_eq!(tokens[2], (TokenKind::Whitespace, "\r\t".to_owned()));
    assert_eq!(tokens[3], (TokenKind::Ident, "А".to_owned()));
}

#[test]
fn crlf_stays_in_comment() {
    assert_eq!(
        kinds_and_texts("// к\r\nА"),
        vec![
            (TokenKind::Comment, "// к\r".to_owned()),
            (TokenKind::Newline, "\n".to_owned()),
            (TokenKind::Ident, "А".to_owned()),
        ]
    );
}

#[test]
fn cr_cr_lf_stays_in_comment() {
    assert_eq!(
        kinds_and_texts("// к\r\r\nА"),
        vec![
            (TokenKind::Comment, "// к\r\r".to_owned()),
            (TokenKind::Newline, "\n".to_owned()),
            (TokenKind::Ident, "А".to_owned()),
        ]
    );
}

#[test]
fn cr_at_end_of_text_stays_in_comment() {
    assert_eq!(kinds_and_texts("// к\r"), vec![(TokenKind::Comment, "// к\r".to_owned())]);
}

#[test]
fn comment_without_cr_runs_to_lf() {
    assert_eq!(
        kinds_and_texts("// к А = 1;\nБ"),
        vec![
            (TokenKind::Comment, "// к А = 1;".to_owned()),
            (TokenKind::Newline, "\n".to_owned()),
            (TokenKind::Ident, "Б".to_owned()),
        ]
    );
}

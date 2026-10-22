//! `comment_runs` над настоящим выводом лексера.
//!
//! Правила серии опираются на то, как лексер режет текст: `\r` уезжает внутрь
//! токена комментария, BOM приходит отдельным токеном, а строка литерала,
//! начинающаяся с `//`, приходит токеном комментария под узлом `LITERAL`.
//! Проверить это можно только полным разбором, поэтому тест интеграционный.

use syntax::{comment_runs, CommentRun};

/// Номер строки по числу переводов строки перед смещением.
fn line_at(code: &str, offset: usize) -> usize {
    code[..offset].matches('\n').count()
}

fn runs(code: &str) -> Vec<CommentRun> {
    comment_runs(&parser::parse(code).syntax_node())
}

/// Серии как пары «первая строка, последняя строка».
fn run_lines(code: &str) -> Vec<(usize, usize)> {
    runs(code)
        .iter()
        .map(|run| {
            let range = run.range();
            (line_at(code, range.start().into()), line_at(code, range.end().into()))
        })
        .collect()
}

/// Строки одной серии как пары «номер строки, владеет ли строкой».
fn owned_lines(code: &str, run: &CommentRun) -> Vec<(usize, bool)> {
    run.lines()
        .iter()
        .map(|line| (line_at(code, line.range.start().into()), line.owns_line))
        .collect()
}

const METHOD_HEADER: &str = "\
// шапка
//
// Параметры:
//  А - Число
//
// Возвращаемое значение:
//  Структура
Функция Ф(А) Экспорт
КонецФункции";

#[test]
fn blank_comment_line_does_not_break_the_run() {
    let runs = runs(METHOD_HEADER);

    assert_eq!(runs.len(), 1);
    assert_eq!(
        owned_lines(METHOD_HEADER, &runs[0]),
        vec![(0, true), (1, true), (2, true), (3, true), (4, true), (5, true), (6, true)]
    );
}

#[test]
fn run_ends_at_the_last_comment() {
    let runs = runs(METHOD_HEADER);

    assert_eq!(runs.len(), 1);
    let end: usize = runs[0].range().end().into();
    assert!(
        METHOD_HEADER[end..].starts_with("\nФункция"),
        "серия захватила объявление: {:?}",
        &METHOD_HEADER[end..end + 20]
    );
}

#[test]
fn blank_line_breaks_the_run() {
    assert_eq!(run_lines("// один\n\n// два\n"), vec![(0, 0), (2, 2)]);
}

#[test]
fn crlf_blank_line_breaks_the_run() {
    assert_eq!(run_lines("// а\r\n// б\r\n\r\n// в\r\n// г\r\n"), vec![(0, 1), (3, 4)]);
}

#[test]
fn code_line_breaks_the_run() {
    assert_eq!(run_lines("// а\n// б\nА = 1;\n// в\n// г\n"), vec![(0, 1), (3, 4)]);
}

#[test]
fn code_after_lone_cr_breaks_the_run() {
    // Одиночный `\r` заканчивает комментарий, и код за ним - отдельная строка
    // модуля для платформы: серия на нём кончается.
    assert_eq!(run_lines("// а\rА = 1;\n// б\n// в\n"), vec![(0, 0), (1, 2)]);
    // Контроль: без `\r` код - текст комментария, серия одна.
    assert_eq!(run_lines("// а А = 1;\n// б\n// в\n"), vec![(0, 2)]);
}

#[test]
fn lone_cr_between_comments_continues_the_run() {
    // Для платформы одиночный `\r` — перевод строки: `// б` стоит на своей
    // строке, соседней с `// а`.
    let code = "// а\r// б\n// в\n";
    let found = runs(code);
    assert_eq!(found.len(), 1);
    assert_eq!(owned_lines(code, &found[0]), vec![(0, true), (0, true), (1, true)]);
    // Два одиночных `\r` — пустая строка между комментариями.
    assert_eq!(runs("// а\r\r// б\n").len(), 2);
}

#[test]
fn comment_after_code_and_lone_cr_owns_its_line() {
    let code = "А = 1;\r// б\n";
    assert_eq!(owned_lines(code, &runs(code)[0]), vec![(0, true)]);
}

#[test]
fn code_line_ended_by_lone_cr_breaks_the_run() {
    assert_eq!(run_lines("// а\n// б\nА = 1;\r// в\n// г\n"), vec![(0, 1), (2, 3)]);
    // Контроль: `\r` перед `\n` — часть CRLF, а не отдельная строка.
    assert_eq!(run_lines("// а\r\n// б\r\nА = 1; \r\n// в\r\n"), vec![(0, 1), (3, 3)]);
}

#[test]
fn trailing_comment_is_in_the_run_without_owning_its_line() {
    let code = "А = 1; // хвост\n// своя\n// своя2\n";

    let runs = runs(code);

    assert_eq!(runs.len(), 1);
    assert_eq!(owned_lines(code, &runs[0]), vec![(0, false), (1, true), (2, true)]);
}

#[test]
fn comment_inside_multiline_literal_is_not_a_run() {
    let inside_literal = "Процедура П()\n Т = \"ВЫБРАТЬ *\n // Сообщить(1);\n // Возврат;\n |ИЗ Т\";\nКонецПроцедуры";
    // Тот же текст вне литерала: без него пустой ответ читался бы и как
    // «гард работает», и как «серии не собираются вовсе».
    let outside_literal = "Процедура П()\n // Сообщить(1);\n // Возврат;\nКонецПроцедуры";

    assert_eq!(run_lines(inside_literal), vec![]);
    assert_eq!(run_lines(outside_literal), vec![(1, 2)]);
}

#[test]
fn comments_between_adjacent_string_literals_are_a_run() {
    // Соседние строковые литералы парсер держит одним узлом LITERAL, и тривия
    // между ними достаётся тому же узлу — текстом строки она не становится.
    assert_eq!(run_lines("А = \"один\"\n// Возврат 1;\n// Возврат 2;\n\"два\";"), vec![(1, 2)]);
    // Тот же разряд, когда слева закрылся многострочный литерал.
    assert_eq!(
        run_lines("А = \"ВЫБРАТЬ\n|*\"\n// Возврат 1;\n// Возврат 2;\n\"два\";"),
        vec![(2, 3)]
    );
}

#[test]
fn bom_does_not_hide_the_first_comment() {
    let code = "\u{FEFF}// один\n// два\n";

    let runs = runs(code);

    assert_eq!(runs.len(), 1);
    assert_eq!(owned_lines(code, &runs[0]), vec![(0, true), (1, true)]);
}

#[test]
fn run_at_end_of_file_without_newline() {
    assert_eq!(run_lines("// один\n// два"), vec![(0, 1)]);
}

#[test]
fn indented_comments_own_their_lines() {
    let code = "\t// один\n\u{a0}// два\n";

    let runs = runs(code);

    assert_eq!(runs.len(), 1);
    assert_eq!(owned_lines(code, &runs[0]), vec![(0, true), (1, true)]);
}

/// Комментарий как текст строки решается лексически — между частями одного
/// литерала, — и одинаково по дереву и по токенам текста.
mod string_text {
    use syntax::{comment_runs, comment_runs_of};

    const MODULE: &str = include_str!("../../../parser/tests/fixtures/Module.bsl");

    fn line_at(code: &str, offset: usize) -> usize {
        code[..offset].matches('\n').count()
    }

    fn by_tree(code: &str) -> Vec<(usize, usize)> {
        comment_runs(&parser::parse(code).syntax_node())
            .iter()
            .map(|run| {
                (line_at(code, run.range().start().into()), line_at(code, run.range().end().into()))
            })
            .collect()
    }

    fn by_tokens(code: &str) -> Vec<(usize, usize)> {
        comment_runs_of(code, &parser::line_tokens(code))
            .iter()
            .map(|run| {
                (line_at(code, run.range().start().into()), line_at(code, run.range().end().into()))
            })
            .collect()
    }

    /// Вход и ожидаемые серии: между открытием и продолжением — текст строки;
    /// оборванная строка, дотянувшаяся до закрывающего слова тела, — тоже
    /// текст строки; между двумя закрытыми литералами, после хвоста, за
    /// оборванной строкой со значимым токеном впереди и перед не-строковым
    /// токеном — комментарий; голые продолжения без открытия продолжают друг
    /// друга.
    const CASES: &[(&str, &[(usize, usize)])] = &[
        ("Т = \"ВЫБРАТЬ *\n // Сообщить(1);\n // Возврат;\n |ИЗ Т\";\n", &[]),
        ("Т = \"а\"\n// между\n\"б\";\n", &[(1, 1)]),
        ("Т = \"а\n|б\"\n// после хвоста\nХ = 1;\n", &[(2, 2)]),
        ("Т = \"открыта\n// комментарий\nХ = 1;\n", &[(1, 1)]),
        ("Т = \"открыта\n// комментарий\n\"закрыта\";\n", &[(1, 1)]),
        ("|голая часть\n// между голыми\n|ещё\n", &[]),
        ("Т = \"а\n// первый\n// второй\n|б\";\n// после\n", &[(4, 4)]),
        ("Т = \"а\n|б\n// между частями\n|в\";\n", &[]),
        ("Т = \"открыта\n// в конце файла", &[(1, 1)]),
        // Оборванный литерал дотянулся до конца тела: строки — его текст.
        ("Процедура П()\n Т = \"ВЫБРАТЬ *\n // Сообщить(1);\n // Возврат;\nКонецПроцедуры", &[]),
        // Контроль: тот же хвост без оборванной строки — это серия.
        ("Процедура П()\n // Сообщить(1);\n // Возврат;\nКонецПроцедуры", &[(1, 2)]),
        // Контроль: закрытая строка перед комментарием — серия остаётся.
        ("Процедура П()\n Т = \"а\n|б\";\n // Б = 2;\nКонецПроцедуры", &[(3, 3)]),
        // Контроль: значимый токен до конца тела — строка оборвана, это комментарий.
        ("Т = \"открыта\n// комментарий\nХ = 1;\nКонецПроцедуры", &[(1, 1)]),
        // Контроль: закрывающее слово функции глушит так же, как процедуры.
        ("Функция Ф()\n Т = \"ВЫБРАТЬ *\n // Сообщить(1);\n // Возврат;\nКонецФункции", &[]),
        // Контроль: значимый токен между открытием и комментарием — строка оборвана.
        ("Т = \"открыта\n Х = 1;\n// комментарий\nКонецПроцедуры", &[(2, 2)]),
        // Контроль: блочный закрыватель телом не считается.
        ("Процедура П()\n Т = \"открыта\n // Б = 2;\n КонецЕсли;\nКонецПроцедуры", &[(2, 2)]),
        // Контроль: оборванная строка в чужом теле не глушит серию в следующем.
        (
            "Процедура П1()\n Т = \"открыта\nКонецПроцедуры\nПроцедура П2()\n // Б = 2;\nКонецПроцедуры",
            &[(4, 4)],
        ),
        ("// один\n\n// два\nА = 1; // хвост\n// своя\n", &[(0, 0), (2, 4)]),
    ];

    #[test]
    fn string_text_is_decided_between_literal_parts() {
        for (code, expected) in CASES {
            assert_eq!(by_tokens(code), *expected, "по токенам:\n{code}");
            assert_eq!(by_tree(code), *expected, "по дереву:\n{code}");
        }
    }

    #[test]
    fn tree_and_token_runs_agree_on_the_fixture() {
        let tree = by_tree(MODULE);
        assert_eq!(by_tokens(MODULE), tree);
        assert!(tree.len() > 100, "фикстура без серий ничего не сторожит: {}", tree.len());
    }
}

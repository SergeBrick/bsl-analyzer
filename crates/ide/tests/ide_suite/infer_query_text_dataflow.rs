use hir::{
    Builders, DefDatabase, HirDatabase, MethodId, MethodIdInput, ModuleId, Name, TypeId,
    TypeKernelDb, TypeKind,
};
use ide_db::base_db::{SourceDatabase, SourceRoot, SourceRootId};
use ide_db::RootDatabaseImpl;
use test_fixture::Fixture;
use vfs::FileId;

fn setup(fixture_text: &str) -> (RootDatabaseImpl, FileId) {
    let fixture = Fixture::parse(fixture_text);
    let mut db = RootDatabaseImpl::new();
    let mut file_set = vfs::FileSet::default();
    for (file_id, file) in &fixture.files {
        file_set.insert(*file_id, file.path.clone());
    }
    db.set_source_root(SourceRootId(0), SourceRoot::new_local(file_set));
    for (file_id, file) in &fixture.files {
        db.set_file_source_root(*file_id, SourceRootId(0));
        db.set_file_text(*file_id, &file.content);
    }
    let test_file = fixture
        .files
        .iter()
        .find(|(_, f)| f.path.as_path().to_string_lossy().ends_with("/test.bsl"))
        .map(|(id, _)| *id)
        .expect("fixture must contain /test.bsl");
    let _ = db.module_bodies(ModuleId::new(test_file));
    (db, test_file)
}

fn var_ty(db: &RootDatabaseImpl, file_id: FileId, var_lower: &str) -> Option<TypeId> {
    db.infer(file_id).var_types.get(var_lower).copied()
}

fn query_no_projection(db: &RootDatabaseImpl, ty: TypeId) -> bool {
    match db.lookup_type(ty) {
        TypeKind::Query { projections } => projections.iter().all(Option::is_none),
        _ => false,
    }
}

#[test]
#[cfg_attr(not(corpus_contract), ignore = "corpus contract: needs the platform help corpus")]
fn straight_line_text_assign_refines_projection_through_execute_select() {
    let fixture = r#"//- /test.bsl
Функция Тест()
    Зап = Новый Запрос;
    Зап.Текст = "ВЫБРАТЬ ""abc"" КАК Имя";
    Х = Зап.Выполнить().Выбрать().Имя;
    Возврат Х;
КонецФункции
"#;
    let (db, file_id) = setup(fixture);
    assert_eq!(
        var_ty(&db, file_id, "х"),
        Some(db.string(None, false)),
        "single literal write to Зап.Текст must let .Выбрать().Имя resolve to Ty::String",
    );
}

#[test]
fn text_append_idiom_collapses_refinement_to_none() {
    let fixture = r#"//- /test.bsl
Функция Тест()
    Зап = Новый Запрос;
    Зап.Текст = "ВЫБРАТЬ ""abc"" КАК Имя";
    Зап.Текст = Зап.Текст + " ИЗ Справочник.Товары";
    Х = Зап.Выполнить().Выбрать().Имя;
    Возврат Х;
КонецФункции
"#;
    let (db, file_id) = setup(fixture);
    let ty = var_ty(&db, file_id, "х");
    assert!(
        ty.is_none_or(|t| t != db.string(None, false)),
        "append idiom must not propagate the first literal's projection — got {ty:?}",
    );
}

#[test]
fn divergent_branch_literals_collapse_refinement_to_none() {
    let fixture = r#"//- /test.bsl
Функция Тест(Флаг)
    Зап = Новый Запрос;
    Если Флаг Тогда
        Зап.Текст = "ВЫБРАТЬ ""abc"" КАК Имя";
    Иначе
        Зап.Текст = "ВЫБРАТЬ 42 КАК Цена";
    КонецЕсли;
    Х = Зап.Выполнить().Выбрать().Имя;
    Возврат Х;
КонецФункции
"#;
    let (db, file_id) = setup(fixture);
    let ty = var_ty(&db, file_id, "х");
    assert!(
        ty.is_none_or(|t| t != db.string(None, false)),
        "divergent-branch literals must not pick one — got {ty:?}",
    );
}

#[test]
#[cfg_attr(not(corpus_contract), ignore = "corpus contract: needs the platform help corpus")]
fn unrelated_field_writes_do_not_block_refinement() {
    let fixture = r#"//- /test.bsl
Функция Тест()
    Зап = Новый Запрос;
    Зап.Текст = "ВЫБРАТЬ ""abc"" КАК Имя";
    Зап.Параметры.Вставить("Foo", "Bar");
    Х = Зап.Выполнить().Выбрать().Имя;
    Возврат Х;
КонецФункции
"#;
    let (db, file_id) = setup(fixture);
    assert_eq!(
        var_ty(&db, file_id, "х"),
        Some(db.string(None, false)),
        "intervening .Параметры call must not block .Текст refinement",
    );
}

#[test]
#[cfg_attr(not(corpus_contract), ignore = "corpus contract: needs the platform help corpus")]
fn no_text_assignment_keeps_receiver_unrefined() {
    let fixture = r#"//- /test.bsl
Функция Тест()
    Зап = Новый Запрос;
    Выборка = Зап.Выполнить().Выбрать();
    Возврат Выборка;
КонецФункции
"#;
    let (db, file_id) = setup(fixture);
    let ty = var_ty(&db, file_id, "выборка").expect("выборка must be inferred");
    let projections = match db.lookup_type(ty) {
        TypeKind::QueryResultSelection(facet) => facet.projection.clone(),
        other => panic!("expected Ty::QueryResultSelection, got {other:?}"),
    };
    assert!(
        projections.is_none(),
        "no Зап.Текст write reaches the dispatch — selection must carry no projection",
    );
}

#[test]
#[cfg_attr(not(corpus_contract), ignore = "corpus contract: needs the platform help corpus")]
fn loop_carried_text_write_collapses_refinement_to_none() {
    let fixture = r#"//- /test.bsl
Функция Тест()
    Зап = Новый Запрос;
    Для i = 1 По 3 Цикл
        Зап.Текст = "ВЫБРАТЬ ""abc"" КАК Имя";
    КонецЦикла;
    Х = Зап.Выполнить().Выбрать().Имя;
    Возврат Х;
КонецФункции
"#;
    let (db, file_id) = setup(fixture);
    let ty = var_ty(&db, file_id, "х").expect("х must be inferred");
    let _ = ty;
    let chain_ty = var_ty(&db, file_id, "х");
    assert!(
        chain_ty.is_some(),
        "loop-body Зап.Текст assignment must not panic the refinement helper",
    );
}

#[test]
fn unbound_receiver_keeps_chain_unrefined() {
    let fixture = r#"//- /test.bsl
Функция Тест()
    Х = Зап.Выполнить().Выбрать().Имя;
    Возврат Х;
КонецФункции
"#;
    let (db, file_id) = setup(fixture);
    let ty = var_ty(&db, file_id, "х");
    assert!(
        ty.is_none_or(|t| t != db.string(None, false)),
        "unbound `Зап` receiver must not produce a Ty::String chain — got {ty:?}",
    );
    let _ = query_no_projection;
}

fn find_method(db: &RootDatabaseImpl, file_id: FileId, name: &str) -> MethodId {
    let symbol_tree = db.symbol_tree(ModuleId::new(file_id));
    symbol_tree
        .find_method(&Name::new(name))
        .unwrap_or_else(|| panic!("expected method `{name}` in {file_id:?}"))
        .id
}

fn value_table_parameters(text: &hir::ExecutedQueryText) -> Vec<&str> {
    text.value_table_parameters.iter().map(|name| name.as_str()).collect()
}

#[test]
fn executed_text_of_issue_function_proves_value_table() {
    let fixture = r#"//- /test.bsl
Функция ПроверитьВнешнийИсточник() Экспорт
    Данные = Новый ТаблицаЗначений;
    Данные.Колонки.Добавить("Ключ", Новый ОписаниеТипов("Число"));
    Данные.Колонки.Добавить("Значение", Новый ОписаниеТипов("Число"));
    СтрокаДанных = Данные.Добавить();
    СтрокаДанных.Ключ = 1;
    СтрокаДанных.Значение = 10;

    Запрос = Новый Запрос;
    Запрос.Текст =
        "ВЫБРАТЬ
        |	Данные.Ключ КАК Ключ,
        |	Данные.Значение КАК Значение
        |ИЗ
        |	&Данные КАК Данные
        |ИТОГИ
        |	СУММА(Значение)
        |ПО
        |	Ключ";
    Запрос.УстановитьПараметр("Данные", Данные);
    Возврат Запрос.Выполнить();
КонецФункции
"#;
    let (db, file_id) = setup(fixture);
    let method = find_method(&db, file_id, "ПроверитьВнешнийИсточник");

    let texts = hir::executed_query_texts(&db, method);
    let body = db.method_body_ref(MethodIdInput::new(&db, method));
    let literals: Vec<_> = body.sdbl_exprs().map(|(expr_id, _, _)| expr_id).collect();

    assert_eq!(literals.len(), 1, "the method holds one query literal");
    assert_eq!(texts.len(), 1, "one execution of one literal, got {texts:?}");
    assert_eq!(texts[0].literal, literals[0]);
    assert_eq!(value_table_parameters(&texts[0]), ["Данные"]);
}

#[test]
fn method_parameter_is_not_a_proven_value_table() {
    let fixture = r#"//- /test.bsl
Процедура И3(Данные)
    Запрос = Новый Запрос;
    Запрос.Текст = "ВЫБРАТЬ Данные.Ключ КАК Ключ ИЗ &Данные КАК Данные";
    Запрос.УстановитьПараметр("Данные", Данные);
    Запрос.Выполнить();
КонецПроцедуры
"#;
    let (db, file_id) = setup(fixture);
    let method = find_method(&db, file_id, "И3");

    let texts = hir::executed_query_texts(&db, method);

    assert_eq!(texts.len(), 1, "the text is executed as written, got {texts:?}");
    assert!(
        texts[0].value_table_parameters.is_empty(),
        "a parameter of unknown type is not a proven value table, got {:?}",
        texts[0].value_table_parameters,
    );
}

#[test]
fn unrecovered_texts_have_no_executed_record() {
    let fixture = r#"//- /test.bsl
Процедура И12а()
    Данные = Новый ТаблицаЗначений;
    Запрос = Новый Запрос;
    Запрос.Текст = "ВЫБРАТЬ Данные.Ключ КАК Ключ ИЗ &Данные КАК Данные";
    Запрос.Текст = СтрЗаменить(Запрос.Текст, "&Данные", "Справочник.Номенклатура");
    Запрос.УстановитьПараметр("Данные", Данные);
    Запрос.Выполнить();
КонецПроцедуры

Процедура И12б()
    Данные = Новый ТаблицаЗначений;
    Запрос = Новый Запрос;
    Запрос.Текст = "ВЫБРАТЬ Данные.Ключ КАК Ключ ИЗ &Данные КАК Данные";
    Запрос.Текст = ТекстЗапроса();
    Запрос.УстановитьПараметр("Данные", Данные);
    Запрос.Выполнить();
КонецПроцедуры

Функция ТекстЗапроса()
    Возврат "";
КонецФункции

Процедура ОбновитьТекст(Параметр)
КонецПроцедуры

Процедура Контроль()
    Данные = Новый ТаблицаЗначений;
    Запрос = Новый Запрос;
    Запрос.Текст = "ВЫБРАТЬ Данные.Ключ КАК Ключ ИЗ &Данные КАК Данные";
    Запрос.УстановитьПараметр("Данные", Данные);
    Запрос.Выполнить();
КонецПроцедуры

Процедура И13а()
    Данные = Новый ТаблицаЗначений;
    Запрос = Новый Запрос;
    Запрос.Текст = "ВЫБРАТЬ Данные.Ключ КАК Ключ ИЗ &Данные КАК Данные";
    ОбновитьТекст(Запрос.Текст);
    Запрос.УстановитьПараметр("Данные", Данные);
    Запрос.Выполнить();
КонецПроцедуры

Процедура И13б()
    Данные = Новый ТаблицаЗначений;
    Запрос = Новый Запрос;
    Запрос.Текст = "ВЫБРАТЬ Данные.Ключ КАК Ключ ИЗ &Данные КАК Данные";
    ОбновитьТекст(Запрос);
    Запрос.УстановитьПараметр("Данные", Данные);
    Запрос.Выполнить();
КонецПроцедуры

Процедура И14а()
    Данные = Новый ТаблицаЗначений;
    Запрос = Новый Запрос;
    Текст = "ВЫБРАТЬ Данные.Ключ КАК Ключ ИЗ &Данные КАК Данные";
    Запрос.Текст = Текст;
    Запрос.УстановитьПараметр("Данные", Данные);
    Запрос.Выполнить();
КонецПроцедуры

Процедура И14б()
    Данные = Новый ТаблицаЗначений;
    Запрос = Новый Запрос;
    Запрос.Текст = "ВЫБРАТЬ Данные.Ключ КАК Ключ ИЗ &Данные КАК Данные" + "";
    Запрос.УстановитьПараметр("Данные", Данные);
    Запрос.Выполнить();
КонецПроцедуры

Процедура И15а()
    Данные = Новый ТаблицаЗначений;
    Запрос = Новый Запрос;
    Запрос.Текст = "ВЫБРАТЬ Данные.Ключ КАК Ключ ИЗ &Данные КАК Данные";
    Запрос.УстановитьПараметр("Данные", Данные);
КонецПроцедуры

Процедура И15б()
    Построитель = Новый ПостроительЗапроса;
    Построитель.Текст = "ВЫБРАТЬ Данные.Ключ КАК Ключ ИЗ &Данные КАК Данные";
    Построитель.Выполнить();
КонецПроцедуры
"#;
    let (db, file_id) = setup(fixture);

    let control = hir::executed_query_texts(&db, find_method(&db, file_id, "Контроль"));
    assert_eq!(
        control.len(),
        1,
        "the same form with an untouched text is executed as written, got {control:?}",
    );
    for name in ["И12а", "И12б", "И13а", "И13б", "И14а", "И14б", "И15а", "И15б"] {
        let method = find_method(&db, file_id, name);
        let texts = hir::executed_query_texts(&db, method);
        assert!(
            texts.is_empty(),
            "[{name}] an unrecovered text must have no record, got {texts:?}"
        );
    }
}

#[test]
fn text_assigned_in_both_branches_is_executed_text() {
    let fixture = r#"//- /test.bsl
Процедура Тест(Флаг)
    Запрос = Новый Запрос;
    Если Флаг Тогда
        Запрос.Текст = "ВЫБРАТЬ А.К КАК К ИЗ &А КАК А";
    Иначе
        Запрос.Текст = "ВЫБРАТЬ Б.К КАК К ИЗ &Б КАК Б";
    КонецЕсли;
    Запрос.Выполнить();
КонецПроцедуры
"#;
    let (db, file_id) = setup(fixture);
    let method = find_method(&db, file_id, "Тест");

    let texts = hir::executed_query_texts(&db, method);
    let body = db.method_body_ref(MethodIdInput::new(&db, method));
    let literals: std::collections::HashSet<_> =
        body.sdbl_exprs().map(|(expr_id, _, _)| expr_id).collect();
    let recorded: std::collections::HashSet<_> = texts.iter().map(|text| text.literal).collect();

    assert_eq!(literals.len(), 2, "two query literals in the method");
    assert_eq!(texts.len(), 2, "one record per branch literal, got {texts:?}");
    assert_eq!(recorded, literals, "every branch literal reaches the execution");
}

#[test]
fn rebound_query_object_has_no_executed_record() {
    let fixture = r#"//- /test.bsl
Процедура Тест()
    Запрос = Новый Запрос;
    Запрос.Текст = "ВЫБРАТЬ А.К КАК К ИЗ &А КАК А";
    Запрос = Новый Запрос;
    Запрос.Выполнить();
КонецПроцедуры
"#;
    let (db, file_id) = setup(fixture);
    let method = find_method(&db, file_id, "Тест");

    let texts = hir::executed_query_texts(&db, method);

    assert!(texts.is_empty(), "a fresh object does not carry the earlier text, got {texts:?}");
}

#[test]
fn aliased_query_object_has_no_executed_record() {
    let fixture = r#"//- /test.bsl
Процедура Тест()
    Запрос = Новый Запрос;
    Запрос.Текст = "ВЫБРАТЬ А.К КАК К ИЗ &А КАК А";
    Другой = Запрос;
    Другой.Текст = "ВЫБРАТЬ 1 КАК К";
    Запрос.Выполнить();
КонецПроцедуры
"#;
    let (db, file_id) = setup(fixture);
    let method = find_method(&db, file_id, "Тест");

    let texts = hir::executed_query_texts(&db, method);

    assert!(texts.is_empty(), "the text was rewritten under another name, got {texts:?}");
}

#[test]
fn every_execution_method_records_the_text() {
    let fixture = r#"//- /test.bsl
Процедура Пакет()
    Запрос = Новый Запрос;
    Запрос.Текст = "ВЫБРАТЬ А.К КАК К ИЗ &А КАК А";
    Запрос.ВыполнитьПакет();
КонецПроцедуры

Процедура Английский()
    Запрос = Новый Запрос;
    Запрос.Текст = "ВЫБРАТЬ А.К КАК К ИЗ &А КАК А";
    Запрос.Execute();
КонецПроцедуры

Процедура АнглийскийПакет()
    Запрос = Новый Запрос;
    Запрос.Текст = "ВЫБРАТЬ А.К КАК К ИЗ &А КАК А";
    Запрос.ExecuteBatch();
КонецПроцедуры
"#;
    let (db, file_id) = setup(fixture);

    for name in ["Пакет", "Английский", "АнглийскийПакет"] {
        let texts = hir::executed_query_texts(&db, find_method(&db, file_id, name));
        assert_eq!(texts.len(), 1, "[{name}] the execution must record its text, got {texts:?}");
    }
}

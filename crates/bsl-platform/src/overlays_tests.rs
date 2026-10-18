use super::*;
use serde_json::json;

fn base_data() -> Value {
    json!({
        "methods": [
            {
                "id": 1,
                "type_name": "DOMDocument",
                "name": "ДобавитьДочерний",
                "english_name": "AppendChild",
                "min_version": "8.1",
                "parameters": [{"name": "НовыйУзел", "param_type": "ЭлементDOM"}]
            }
        ]
    })
}

fn curated_overlay() -> Value {
    serde_json::from_str(include_str!("../data/platform_overlays.json")).unwrap()
}

/// The curated overlay narrowed to its DOM `AppendChild` override, the one
/// method [`base_data`] carries; the other sections stay as curated.
fn valid_overlay() -> String {
    let mut overlay = curated_overlay();
    overlay["method_parameter_overrides"].as_array_mut().unwrap().truncate(1);
    overlay.as_object_mut().unwrap().remove("global_function_parameter_overrides");
    serde_json::to_string_pretty(&overlay).unwrap()
}

fn overlay_with_versions(min_version: Option<&str>, max_version: Option<&str>) -> String {
    let mut overlay: Value = serde_json::from_str(&valid_overlay()).unwrap();
    let entry = overlay["method_parameter_overrides"][0].as_object_mut().unwrap();

    match min_version {
        Some(version) => entry.insert("min_version".to_owned(), Value::String(version.to_owned())),
        None => entry.remove("min_version"),
    };
    match max_version {
        Some(version) => entry.insert("max_version".to_owned(), Value::String(version.to_owned())),
        None => entry.remove("max_version"),
    };

    overlay.to_string()
}

fn base_data_with_version(version: Option<&str>) -> Value {
    let mut data = base_data();
    let method = data["methods"][0].as_object_mut().unwrap();

    match version {
        Some(version) => method.insert("min_version".to_owned(), Value::String(version.to_owned())),
        None => method.remove("min_version"),
    };

    data
}

#[test]
fn applies_curated_dom_append_child_override() {
    let mut data = base_data();

    apply_method_parameter_overlays(&mut data, &valid_overlay()).unwrap();

    assert_eq!(
        data["methods"][0]["parameters"][0]["param_type"],
        "АтрибутDOM, ДокументDOM, ЭлементDOM, ОпределениеТипаДокументаDOM, НотацияDOM, АтрибутHTML, ЭлементHTML, ЭлементКнопкаHTML, ЭлементВводаHTML, ЭлементЗаголовокHTML"
    );
}

#[test]
fn rejects_missing_target_deterministically() {
    let mut data = base_data();
    let overlay = valid_overlay().replace("DOMDocument", "MissingDocument");

    let error = apply_method_parameter_overlays(&mut data, &overlay).unwrap_err();

    assert_eq!(
        error.to_string(),
        "overlay target MissingDocument.AppendChild is missing or ambiguous"
    );
}

#[test]
fn rejects_duplicate_alias_override_deterministically() {
    let mut data = base_data();
    let mut overlay: Value = serde_json::from_str(&valid_overlay()).unwrap();
    let duplicate = overlay["method_parameter_overrides"][0].clone();
    overlay["method_parameter_overrides"].as_array_mut().unwrap().push(duplicate);

    let error = apply_method_parameter_overlays(&mut data, &overlay.to_string()).unwrap_err();

    assert_eq!(error.to_string(), "duplicate override for DOMDocument.AppendChild parameter 0");
}

#[test]
fn rejects_aliases_that_resolve_to_different_methods() {
    let mut data = base_data();
    data["methods"].as_array_mut().unwrap().push(json!({
        "id": 2,
        "type_name": "DOMDocument",
        "name": "ДругоеИмя",
        "english_name": "OtherName",
        "min_version": "8.1",
        "parameters": [{"name": "НовыйУзел", "param_type": "ЭлементDOM"}]
    }));
    let overlay = valid_overlay().replace("AppendChild", "OtherName");

    let error = apply_method_parameter_overlays(&mut data, &overlay).unwrap_err();

    assert_eq!(
        error.to_string(),
        "overlay aliases for DOMDocument.OtherName resolve to different methods (0 and 1)"
    );
}

#[test]
fn rejects_out_of_range_parameter_index_deterministically() {
    let mut data = base_data();
    let overlay = valid_overlay().replace("\"parameter_index\": 0", "\"parameter_index\": 1");

    let error = apply_method_parameter_overlays(&mut data, &overlay).unwrap_err();

    assert_eq!(
        error.to_string(),
        "overlay target DOMDocument.AppendChild has no parameter at index 1"
    );
}

#[test]
fn rejects_malformed_schema_deterministically() {
    let mut data = base_data();

    let error =
        apply_method_parameter_overlays(&mut data, "{\"schema_version\": \"1\"}").unwrap_err();

    assert_eq!(error.to_string(), "overlay schema_version must be the number 1");
}

#[test]
fn rejects_max_only_bound_above_target_version() {
    let mut data = base_data();

    let error =
        apply_method_parameter_overlays(&mut data, &overlay_with_versions(None, Some("8.0")))
            .unwrap_err();

    assert_eq!(
        error.to_string(),
        "overlay target DOMDocument.AppendChild version is outside its declared bounds"
    );
}

#[test]
fn accepts_max_only_bound_at_target_version() {
    let mut data = base_data();

    apply_method_parameter_overlays(&mut data, &overlay_with_versions(None, Some("8.1"))).unwrap();
}

#[test]
fn rejects_min_only_bound_below_target_version() {
    let mut data = base_data();

    let error =
        apply_method_parameter_overlays(&mut data, &overlay_with_versions(Some("8.2"), None))
            .unwrap_err();

    assert_eq!(
        error.to_string(),
        "overlay target DOMDocument.AppendChild version is outside its declared bounds"
    );
}

#[test]
fn accepts_min_only_bound_at_target_version() {
    let mut data = base_data();

    apply_method_parameter_overlays(&mut data, &overlay_with_versions(Some("8.1"), None)).unwrap();
}

#[test]
fn rejects_reversed_version_bounds() {
    let mut data = base_data();

    let error = apply_method_parameter_overlays(
        &mut data,
        &overlay_with_versions(Some("8.2"), Some("8.1")),
    )
    .unwrap_err();

    assert_eq!(error.to_string(), "overlay entry 0 has min_version after max_version");
}

#[test]
fn accepts_both_bounds_when_target_is_interior() {
    let mut data = base_data();

    apply_method_parameter_overlays(&mut data, &overlay_with_versions(Some("8.0"), Some("8.2")))
        .unwrap();
}

#[test]
fn accepts_both_bounds_at_inclusive_endpoint() {
    let mut data = base_data();

    apply_method_parameter_overlays(&mut data, &overlay_with_versions(Some("8.1"), Some("8.1")))
        .unwrap();
}

#[test]
fn rejects_missing_target_version_when_bounds_exist() {
    let mut data = base_data_with_version(None);

    let error =
        apply_method_parameter_overlays(&mut data, &overlay_with_versions(None, Some("8.1")))
            .unwrap_err();

    assert_eq!(
        error.to_string(),
        "overlay target DOMDocument.AppendChild has no valid minimum version"
    );
}

fn property_data() -> Value {
    json!({
        "types": [
            {"name": "ФормаКлиентскогоПриложения", "english_name": "ClientApplicationForm"}
        ],
        "properties": [
            {"id": 7, "type_name": "ClientApplicationForm", "name": "Заголовок", "english_name": "Title"}
        ]
    })
}

fn property_addition_overlay(entry: Value) -> String {
    json!({
        "schema_version": 1,
        "method_parameter_overrides": [],
        "type_property_additions": [entry]
    })
    .to_string()
}

fn window_mode_addition() -> Value {
    json!({
        "canonical_type": "ClientApplicationForm",
        "russian_name": "РежимОткрытияОкна",
        "english_name": "WindowOpeningMode",
        "property_types": ["РежимОткрытияОкнаФормы"],
        "is_readonly": false,
        "min_version": "8.2",
        "evidence_source": "v8std #std742",
        "rationale": "help archive files it under the enum type name"
    })
}

#[test]
fn appends_type_property_addition_with_synthetic_id() {
    let mut data = property_data();

    apply_type_property_additions(&mut data, &property_addition_overlay(window_mode_addition()))
        .unwrap();

    let properties = data["properties"].as_array().unwrap();
    assert_eq!(properties.len(), 2);
    let added = &properties[1];
    assert_eq!(added["type_name"], "ClientApplicationForm");
    assert_eq!(added["name"], "РежимОткрытияОкна");
    assert_eq!(added["english_name"], "WindowOpeningMode");
    assert_eq!(added["property_types"][0], "РежимОткрытияОкнаФормы");
    // max existing id (7) + 1
    assert_eq!(added["id"], 8);
}

#[test]
fn absent_additions_section_is_a_no_op() {
    let mut data = property_data();
    let overlay = json!({
        "schema_version": 1,
        "method_parameter_overrides": []
    })
    .to_string();

    apply_type_property_additions(&mut data, &overlay).unwrap();

    assert_eq!(data["properties"].as_array().unwrap().len(), 1);
}

#[test]
fn rejects_addition_for_unknown_type() {
    let mut data = property_data();
    let mut entry = window_mode_addition();
    entry["canonical_type"] = Value::String("NoSuchType".to_owned());

    let error =
        apply_type_property_additions(&mut data, &property_addition_overlay(entry)).unwrap_err();

    assert_eq!(
        error.to_string(),
        "type property addition target NoSuchType is not a known platform type (canonical_type must be the English type name)"
    );
}

#[test]
fn rejects_russian_canonical_type() {
    // The property index keys on the English type name, so a Russian alias would
    // produce a property `get_type_properties` can never resolve.
    let mut data = property_data();
    let mut entry = window_mode_addition();
    entry["canonical_type"] = Value::String("ФормаКлиентскогоПриложения".to_owned());

    let error =
        apply_type_property_additions(&mut data, &property_addition_overlay(entry)).unwrap_err();

    assert_eq!(
        error.to_string(),
        "type property addition target ФормаКлиентскогоПриложения is not a known platform type (canonical_type must be the English type name)"
    );
}

#[test]
fn skips_addition_the_extracted_data_already_carries() {
    // 8.5 archives file РежимОткрытияОкна on the form itself: the archive's
    // record, which has documentation, stays and the addition is dropped.
    let mut data = property_data();
    data["properties"][0]["documentation"] = json!({"description": "Заголовок формы."});
    let before = data["properties"].clone();
    let mut entry = window_mode_addition();
    entry["russian_name"] = Value::String("заголовок".to_owned());
    entry["english_name"] = Value::String("Title".to_owned());

    apply_type_property_additions(&mut data, &property_addition_overlay(entry)).unwrap();

    assert_eq!(data["properties"], before);
}

#[test]
fn rejects_full_match_alongside_a_property_sharing_one_alias() {
    // Заголовок/Title is a full match; Подпись/Title shares only the English
    // alias, so the name index would hold two entries for "Title". The verdict
    // must not depend on which comes first.
    let full = json!({"id": 7, "type_name": "ClientApplicationForm", "name": "Заголовок", "english_name": "Title"});
    let partial = json!({"id": 9, "type_name": "ClientApplicationForm", "name": "Подпись", "english_name": "Title"});
    let mut entry = window_mode_addition();
    entry["russian_name"] = Value::String("Заголовок".to_owned());
    entry["english_name"] = Value::String("Title".to_owned());

    for properties in [json!([full, partial]), json!([partial, full])] {
        let mut data = property_data();
        data["properties"] = properties;

        let error =
            apply_type_property_additions(&mut data, &property_addition_overlay(entry.clone()))
                .unwrap_err();

        assert_eq!(
            error.to_string(),
            "type property addition ClientApplicationForm.Title already exists in the extracted data"
        );
    }
}

#[test]
fn rejects_several_full_matches() {
    let mut data = property_data();
    let duplicate = json!({"id": 9, "type_name": "ClientApplicationForm", "name": "Заголовок", "english_name": "Title"});
    data["properties"].as_array_mut().unwrap().push(duplicate);
    let mut entry = window_mode_addition();
    entry["russian_name"] = Value::String("Заголовок".to_owned());
    entry["english_name"] = Value::String("Title".to_owned());

    let error =
        apply_type_property_additions(&mut data, &property_addition_overlay(entry)).unwrap_err();

    assert_eq!(
        error.to_string(),
        "type property addition ClientApplicationForm.Title already exists in the extracted data"
    );
}

#[test]
fn adds_after_skipping_a_present_addition_at_its_enumerate_offset() {
    // The skipped first addition still consumes offset 0, so the second takes
    // base_id (max existing id 7, plus 1) plus 1.
    let mut data = property_data();
    let mut present = window_mode_addition();
    present["russian_name"] = Value::String("Заголовок".to_owned());
    present["english_name"] = Value::String("Title".to_owned());
    let overlay = json!({
        "schema_version": 1,
        "method_parameter_overrides": [],
        "type_property_additions": [present, window_mode_addition()]
    })
    .to_string();

    apply_type_property_additions(&mut data, &overlay).unwrap();

    let properties = data["properties"].as_array().unwrap();
    assert_eq!(properties.len(), 2);
    assert_eq!(properties[1]["english_name"], "WindowOpeningMode");
    assert_eq!(properties[1]["id"], 9);
}

#[test]
fn rejects_addition_sharing_one_name_with_an_existing_property() {
    let mut data = property_data();
    let mut entry = window_mode_addition();
    entry["russian_name"] = Value::String("Заголовок".to_owned());
    entry["english_name"] = Value::String("Caption".to_owned());

    let error =
        apply_type_property_additions(&mut data, &property_addition_overlay(entry)).unwrap_err();

    assert_eq!(
        error.to_string(),
        "type property addition ClientApplicationForm.Caption already exists in the extracted data"
    );
}

#[test]
fn rejects_addition_colliding_with_existing_english_name_via_russian() {
    // property_data() has Заголовок/Title. An addition whose Russian name is
    // "Title" would shadow that property in the shared name index, so it must be
    // rejected even though no RU→RU or EN→EN pair matches directly.
    let mut data = property_data();
    let mut entry = window_mode_addition();
    entry["russian_name"] = Value::String("Title".to_owned());

    let error =
        apply_type_property_additions(&mut data, &property_addition_overlay(entry)).unwrap_err();

    assert_eq!(
        error.to_string(),
        "type property addition ClientApplicationForm.WindowOpeningMode already exists in the extracted data"
    );
}

#[test]
fn rejects_addition_with_empty_property_types() {
    let mut data = property_data();
    let mut entry = window_mode_addition();
    entry["property_types"] = json!([]);

    let error =
        apply_type_property_additions(&mut data, &property_addition_overlay(entry)).unwrap_err();

    assert_eq!(error.to_string(), "type property addition 0 property_types must not be empty");
}

#[test]
fn curated_overlay_adds_window_opening_mode() {
    let mut data = property_data();

    apply_type_property_additions(&mut data, &valid_overlay()).unwrap();

    let added = data["properties"].as_array().unwrap().iter().any(|prop| {
        prop["type_name"] == "ClientApplicationForm" && prop["name"] == "РежимОткрытияОкна"
    });
    assert!(added, "curated overlay must add ClientApplicationForm.РежимОткрытияОкна");
}

fn global_function_data() -> Value {
    json!({
        "types": [
            {"name": "ОбъектМетаданных: Поле", "english_name": "MetadataObject: Field"},
            {"name": "ОбъектМетаданных: Справочник", "english_name": "MetadataObject: Catalog"},
            {"name": "ОбъектМетаданныхКонфигурация", "english_name": "ConfigurationMetadataObject"},
            {"name": "ОбъектМетаданных: ТаблицаИзмерения", "english_name": "MetadataObject: DimensionTable"}
        ],
        "methods": [
            {
                "id": 1,
                "type_name": "MetadataObjectCollection",
                "name": "ПравоДоступа",
                "english_name": "AccessRight",
                "min_version": "8.0",
                "parameters": [{"name": "Право", "param_type": "Строка"}, {"name": "Объект", "param_type": "Строка"}]
            }
        ],
        "global_functions": [
            {
                "id": 459,
                "name": "ПравоДоступа",
                "english_name": "AccessRight",
                "min_version": "8.0",
                "parameters": [
                    {"name": "Право", "param_type": "Строка"},
                    {"name": "ОбъектМетаданных", "param_type": "ОбъектМетаданных: ТаблицаИзмерения, ОбъектМетаданных: Поле"}
                ]
            }
        ]
    })
}

fn global_function_overlay(entry: Value) -> String {
    json!({
        "schema_version": 1,
        "method_parameter_overrides": [],
        "global_function_parameter_overrides": [entry]
    })
    .to_string()
}

fn access_right_override() -> Value {
    json!({
        "russian_name": "ПравоДоступа",
        "english_name": "AccessRight",
        "min_version": "8.0",
        "parameter_index": 1,
        "replacement_type_list": ["ОбъектМетаданных: *"],
        "evidence_source": "the parameter description lists every metadata-object kind",
        "rationale": "the extract cut the list to five kinds"
    })
}

#[test]
fn applies_global_function_override_by_name_alone() {
    let mut data = global_function_data();

    apply_global_function_parameter_overlays(
        &mut data,
        &global_function_overlay(access_right_override()),
    )
    .unwrap();

    assert_eq!(
        data["global_functions"][0]["parameters"][1]["param_type"],
        "ОбъектМетаданных: Поле, ОбъектМетаданных: Справочник, ОбъектМетаданных: ТаблицаИзмерения"
    );
    assert_eq!(
        data["methods"][0]["parameters"][1]["param_type"], "Строка",
        "a type method of the same name is not a global function"
    );
}

#[test]
fn global_function_override_leaves_method_overrides_alone() {
    let mut data = global_function_data();

    apply_method_parameter_overlays(&mut data, &global_function_overlay(access_right_override()))
        .unwrap();

    assert_eq!(
        data["global_functions"][0]["parameters"][1]["param_type"],
        "ОбъектМетаданных: ТаблицаИзмерения, ОбъектМетаданных: Поле"
    );
}

#[test]
fn absent_global_function_section_is_a_no_op() {
    let mut data = global_function_data();
    data.as_object_mut().unwrap().remove("global_functions");

    apply_global_function_parameter_overlays(&mut data, &valid_overlay()).unwrap();
}

#[test]
fn rejects_missing_global_function_target() {
    let mut data = global_function_data();
    let mut entry = access_right_override();
    entry["english_name"] = Value::String("NoSuchFunction".to_owned());
    entry["russian_name"] = Value::String("НетТакойФункции".to_owned());

    let error =
        apply_global_function_parameter_overlays(&mut data, &global_function_overlay(entry))
            .unwrap_err();

    assert_eq!(error.to_string(), "overlay target NoSuchFunction is missing or ambiguous");
}

#[test]
fn rejects_global_function_aliases_that_resolve_to_different_functions() {
    let mut data = global_function_data();
    data["global_functions"].as_array_mut().unwrap().push(json!({
        "id": 460,
        "name": "ДругаяФункция",
        "english_name": "OtherFunction",
        "min_version": "8.0",
        "parameters": [{"name": "Право", "param_type": "Строка"}, {"name": "Объект", "param_type": "Строка"}]
    }));
    let mut entry = access_right_override();
    entry["english_name"] = Value::String("OtherFunction".to_owned());

    let error =
        apply_global_function_parameter_overlays(&mut data, &global_function_overlay(entry))
            .unwrap_err();

    assert_eq!(
        error.to_string(),
        "overlay aliases for OtherFunction resolve to different methods (0 and 1)"
    );
}

#[test]
fn rejects_canonical_type_on_a_global_function_override() {
    let mut data = global_function_data();
    let mut entry = access_right_override();
    entry["canonical_type"] = Value::String("MetadataObjectCollection".to_owned());

    let error =
        apply_global_function_parameter_overlays(&mut data, &global_function_overlay(entry))
            .unwrap_err();

    assert_eq!(error.to_string(), "overlay entry 0 has unknown field canonical_type");
}

#[test]
fn rejects_duplicate_global_function_override() {
    let mut data = global_function_data();
    let overlay = json!({
        "schema_version": 1,
        "method_parameter_overrides": [],
        "global_function_parameter_overrides": [access_right_override(), access_right_override()]
    })
    .to_string();

    let error = apply_global_function_parameter_overlays(&mut data, &overlay).unwrap_err();

    assert_eq!(error.to_string(), "duplicate override for AccessRight parameter 1");
}

#[test]
fn rejects_global_function_override_outside_its_version_bounds() {
    let mut data = global_function_data();
    let mut entry = access_right_override();
    entry["min_version"] = Value::String("8.2".to_owned());

    let error =
        apply_global_function_parameter_overlays(&mut data, &global_function_overlay(entry))
            .unwrap_err();

    assert_eq!(
        error.to_string(),
        "overlay target AccessRight version is outside its declared bounds"
    );
}

#[test]
fn family_wildcard_expands_to_every_extracted_member() {
    let mut data = global_function_data();
    let mut entry = access_right_override();
    entry["replacement_type_list"] =
        json!(["Массив", "ОбъектМетаданных: *", "ОбъектМетаданных: Поле"]);

    apply_global_function_parameter_overlays(&mut data, &global_function_overlay(entry)).unwrap();

    assert_eq!(
        data["global_functions"][0]["parameters"][1]["param_type"],
        "Массив, ОбъектМетаданных: Поле, ОбъектМетаданных: Справочник, ОбъектМетаданных: ТаблицаИзмерения",
        "members in extract order, once each; a name merely sharing the stem is no member"
    );
}

#[test]
fn rejects_family_wildcard_without_members() {
    let mut data = global_function_data();
    let mut entry = access_right_override();
    entry["replacement_type_list"] = json!(["ОбъектНетТакого: *"]);

    let error =
        apply_global_function_parameter_overlays(&mut data, &global_function_overlay(entry))
            .unwrap_err();

    assert_eq!(
        error.to_string(),
        "overlay target AccessRight replacement ОбъектНетТакого: * names no extracted type"
    );
}

#[test]
fn a_star_without_a_family_prefix_is_a_plain_name() {
    let mut data = global_function_data();
    let mut entry = access_right_override();
    entry["replacement_type_list"] = json!(["ОбъектМетаданных*"]);

    apply_global_function_parameter_overlays(&mut data, &global_function_overlay(entry)).unwrap();

    assert_eq!(data["global_functions"][0]["parameters"][1]["param_type"], "ОбъектМетаданных*");
}

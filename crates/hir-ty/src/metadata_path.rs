//! Metadata named by a string argument of a platform function.
//!
//! `Тип("СправочникСсылка.Товары")`, `ПредопределенноеЗначение("Перечисление.Цвета.Красный")`
//! and `ОткрытьФорму("Справочник.Товары.ФормаСписка")` name metadata by value. The
//! platform's extended module check resolves such a literal against the configuration
//! and reports a miss as «Возможно ошибочный параметр»; the call itself fails at run
//! time. The spelling written as code (`Справочники.Товары`, `Перечисления.Цвета.Красный`)
//! is judged by the manager lookup — this module asks the same questions of the string.
//!
//! Only what the configuration model answers exhaustively is judged: the object (for
//! every path), and the predefined value of an enum, a catalog or a chart. A form name
//! is never judged — the model does not list an object's forms, and the standard ones
//! (`ФормаСписка`, `ФормаВыбора`, …) exist without being declared.
//!
//! A type name is judged the same way where the constructor `Новый ОписаниеТипов` takes
//! it: a comma-separated list, each name checked on its own. One more miss is common to
//! `Тип` and `ОписаниеТипов`: the bare kind prefix (`ДокументСсылка`, `ЛюбаяСсылка`) —
//! a type set of the form XML (`cfg:DocumentRef`), but not a type. `Тип("ДокументСсылка")`
//! fails with «Тип не определен», `Новый ОписаниеТипов("ДокументСсылка")` with
//! «Недопустимое значение параметра» (8.3.17 and 8.3.27, server and thin client).

use std::str::FromStr;

use bsl_metadata::MdoType;
use stdx::case::CaseExt;

use crate::object_resolver::ObjectResolver;

/// A platform function whose first argument is a metadata path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MetadataPathFunction {
    /// `Тип` / `Type`: a type name, `<Вид><Фасет>.<Имя>`.
    Type,
    /// `ПредопределенноеЗначение` / `PredefinedValue`: `<Вид>.<Имя>.<Значение>`.
    PredefinedValue,
    /// `ОткрытьФорму`, `ОткрытьФормуМодально`, `ПолучитьФорму`: `<Вид>.<Имя>.<Форма…>`.
    Form,
    /// A string argument of the constructor `Новый ОписаниеТипов`: type names separated
    /// by commas. Not a global function, so [`Self::of`] never yields it.
    TypeDescription,
}

impl MetadataPathFunction {
    pub(crate) fn of(name: &str) -> Option<Self> {
        match name.fold_lower().as_str() {
            "тип" | "type" => Some(Self::Type),
            "предопределенноезначение" | "predefinedvalue" => {
                Some(Self::PredefinedValue)
            }
            "открытьформу"
            | "openform"
            | "открытьформумодально"
            | "openformmodal"
            | "получитьформу"
            | "getform" => Some(Self::Form),
            _ => None,
        }
    }
}

/// The part of a metadata path the configuration does not have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MissingMetadata {
    /// No object of this kind and name.
    Object { kind: MdoType, name: String },
    /// The object exists and lists its predefined values, but not this one.
    PredefinedValue { kind: MdoType, owner: String, value: String },
    /// A type name that is only a kind prefix (`ДокументСсылка`, `CatalogRef`) or
    /// `ЛюбаяСсылка`: a set of types, where one type is required.
    TypeSet { name: String },
}

/// What `text`, the literal argument of `function`, names that the configuration does
/// not have — one entry per missing part; a type list of `ОписаниеТипов` may miss
/// several. Empty when the path is fine or is not one this check can judge: another
/// shape, a kind outside the configuration (external processors and reports), a part
/// the model does not list.
pub(crate) fn missing_metadata(
    function: MetadataPathFunction,
    text: &str,
    resolver: &dyn ObjectResolver,
) -> Vec<MissingMetadata> {
    match function {
        // The platform trims the names and skips an empty one between commas; a
        // trailing comma or a blank string is accepted by one version and rejected by
        // another, so an empty name is not judged.
        MetadataPathFunction::TypeDescription => {
            let mut missing = Vec::new();
            for name in text.split(',').map(str::trim).filter(|name| !name.is_empty()) {
                if let Some(miss) = missing_type(name, resolver) {
                    if !missing.contains(&miss) {
                        missing.push(miss);
                    }
                }
            }
            missing
        }
        MetadataPathFunction::Type => missing_type(text, resolver).into_iter().collect(),
        MetadataPathFunction::Form | MetadataPathFunction::PredefinedValue => {
            missing_object_path(function, text, resolver).into_iter().collect()
        }
    }
}

/// A type name `<Вид><Фасет>.<Имя>`, or a bare `<Вид><Фасет>` that names a type set.
fn missing_type(text: &str, resolver: &dyn ObjectResolver) -> Option<MissingMetadata> {
    let segments: Vec<&str> = text.trim().split('.').collect();
    if !segments.iter().all(|segment| crate::lower::type_string::is_identifier_shaped(segment)) {
        return None;
    }
    match segments[..] {
        [name] => type_set(name),
        [prefix, name] => {
            let kind = crate::lower::metadata_kind_from_prefix(&kind_spelling(prefix))?;
            let mdo = crate::field_enum::mdo_type_for_kind(kind)
                .or_else(|| crate::field_enum::register_parent_for_kind(kind))
                .filter(|mdo| is_configuration_kind(*mdo))?;
            object_missing(resolver, mdo, name)
        }
        _ => None,
    }
}

/// A kind prefix written without the object name. `ОтчетОбъект` / `ReportObject` is the
/// exception: a type of its own on the server (`Тип("ОтчетОбъект")` and
/// `Новый ОписаниеТипов("ОтчетОбъект")` succeed there and fail on the client), so it is
/// not judged. Only the «е» spelling is exempt: the platform does not read «ё» as «е»
/// in a bare name (`ОтчётОбъект` fails where `ОтчетОбъект` succeeds), so any other kind
/// spelled with «ё» fails all the same.
fn type_set(name: &str) -> Option<MissingMetadata> {
    let folded = name.fold_lower();
    let is_set = matches!(folded.as_str(), "любаяссылка" | "anyref")
        || (crate::lower::metadata_kind_from_prefix(&kind_spelling(name)).is_some()
            && !matches!(folded.as_str(), "отчетобъект" | "reportobject"));
    is_set.then(|| MissingMetadata::TypeSet { name: name.to_string() })
}

/// A path `<Вид>.<Имя>…` of [`MetadataPathFunction::Form`] or
/// [`MetadataPathFunction::PredefinedValue`].
fn missing_object_path(
    function: MetadataPathFunction,
    text: &str,
    resolver: &dyn ObjectResolver,
) -> Option<MissingMetadata> {
    let segments: Vec<&str> = text.trim().split('.').collect();
    if !segments.iter().all(|segment| crate::lower::type_string::is_identifier_shaped(segment)) {
        return None;
    }
    match function {
        MetadataPathFunction::Type | MetadataPathFunction::TypeDescription => None,
        MetadataPathFunction::Form => {
            let (mdo, name) = configuration_object(&segments)?;
            object_missing(resolver, mdo, name)
        }
        MetadataPathFunction::PredefinedValue => {
            let (mdo, name) = configuration_object(&segments)?;
            if let Some(missing) = object_missing(resolver, mdo, name) {
                return Some(missing);
            }
            let [_, _, value] = segments[..] else {
                return None;
            };
            predefined_value_missing(resolver, mdo, name, value)
        }
    }
}

/// A kind name with «ё» read as «е» (`Отчёт`, `РегистрРасчёта`, `ПланСчётов`), the
/// way the platform takes it; the kind tables list the «е» spelling.
fn kind_spelling(kind: &str) -> String {
    kind.replace(['ё', 'Ё'], "е")
}

/// `<Вид>.<Имя>…` of a kind the configuration holds by name.
fn configuration_object<'a>(segments: &[&'a str]) -> Option<(MdoType, &'a str)> {
    let [kind, name, ..] = segments[..] else {
        return None;
    };
    let mdo =
        MdoType::from_str(&kind_spelling(kind)).ok().filter(|mdo| is_configuration_kind(*mdo))?;
    Some((mdo, name))
}

/// The kinds a metadata path names a configuration object of. External processors and
/// reports are not in the configuration at all; the rest of [`MdoType`] is not named
/// by a path these functions take.
fn is_configuration_kind(mdo: MdoType) -> bool {
    matches!(
        mdo,
        MdoType::Catalog
            | MdoType::Document
            | MdoType::InformationRegister
            | MdoType::AccumulationRegister
            | MdoType::AccountingRegister
            | MdoType::CalculationRegister
            | MdoType::ChartOfCharacteristicTypes
            | MdoType::ChartOfAccounts
            | MdoType::ChartOfCalculationTypes
            | MdoType::BusinessProcess
            | MdoType::Task
            | MdoType::Enum
            | MdoType::ExchangePlan
            | MdoType::Constant
            | MdoType::DataProcessor
            | MdoType::Report
    )
}

fn object_missing(
    resolver: &dyn ObjectResolver,
    kind: MdoType,
    name: &str,
) -> Option<MissingMetadata> {
    (!crate::manager_lookup::collection_member_exists(resolver, kind, name))
        .then(|| MissingMetadata::Object { kind, name: name.to_string() })
}

/// The same membership `Перечисления.Цвета.Красный` is judged by
/// ([`crate::manager_lookup::lookup_predefined`]) over the same closed set
/// ([`crate::manager_lookup::closed_member_owner`]), plus the empty reference every one
/// of these kinds has without declaring it.
fn predefined_value_missing(
    resolver: &dyn ObjectResolver,
    kind: MdoType,
    owner: &str,
    value: &str,
) -> Option<MissingMetadata> {
    if matches!(value.fold_lower().as_str(), "пустаяссылка" | "emptyref") {
        return None;
    }
    let mdo = crate::manager_lookup::closed_member_owner(resolver, kind, owner)?;
    let found = match kind {
        MdoType::Enum => mdo.find_enum_value(value).is_some(),
        _ => mdo.find_predefined_item(value).is_some(),
    };
    (!found).then(|| MissingMetadata::PredefinedValue {
        kind,
        owner: owner.to_string(),
        value: value.to_string(),
    })
}

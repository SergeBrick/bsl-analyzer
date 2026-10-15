use crate::enums::ObjectBelonging;
use crate::error::{MetadataError, Result};
use crate::metadata_object::{MdoType, StandardAttributeKind};
use crate::register::RegisterAttribute;
use uuid::Uuid;

pub(crate) fn parse_uuid(s: &str, context: &str) -> Result<Uuid> {
    s.parse::<Uuid>()
        .map_err(|e| MetadataError::InvalidFormat(format!("Invalid {} UUID: {}", context, e)))
}

pub(crate) fn create_register_standard_attribute(
    kind: &StandardAttributeKind,
    mdo_type: MdoType,
    object_name: &str,
) -> RegisterAttribute {
    let nil_uuid = Uuid::nil();
    let mut attr = RegisterAttribute::new(nil_uuid, kind.russian_name());
    attr.set_name_en(kind.english_name());
    attr.set_attr_type(kind.attribute_type(mdo_type, object_name));
    attr
}

pub(crate) fn find_child<'a>(
    node: roxmltree::Node<'a, 'a>,
    tag: &str,
) -> Option<roxmltree::Node<'a, 'a>> {
    node.children().find(|n| n.is_element() && n.tag_name().name() == tag)
}

pub(crate) fn child_text<'a>(node: roxmltree::Node<'a, 'a>, tag: &str) -> Option<&'a str> {
    find_child(node, tag).and_then(|n| n.text())
}

/// Reads whether an object is adopted from the base configuration and which base
/// object it extends. A malformed base reference is dropped rather than failing the
/// object, so the object still loads, only without being merged into its base.
pub(crate) fn parse_extension_ownership(
    props: roxmltree::Node<'_, '_>,
) -> (ObjectBelonging, Option<Uuid>) {
    let belonging = match child_text(props, "ObjectBelonging") {
        Some("Adopted") => ObjectBelonging::Adopted,
        Some("Own") => ObjectBelonging::Own,
        _ => ObjectBelonging::Unknown,
    };
    let extends = child_text(props, "ExtendedConfigurationObject").and_then(|raw| {
        Uuid::parse_str(raw)
            .inspect_err(|err| {
                tracing::warn!(uuid_raw = %raw, %err, "ignored malformed ExtendedConfigurationObject");
            })
            .ok()
    });
    (belonging, extends)
}

pub(crate) fn child_bool(node: roxmltree::Node<'_, '_>, tag: &str) -> bool {
    child_text(node, tag).is_some_and(|s| s.eq_ignore_ascii_case("true"))
}

pub(crate) fn child_u32(node: roxmltree::Node<'_, '_>, tag: &str) -> Option<u32> {
    // Configurator XML is tight, but a dumped or hand-edited element can carry a newline or a
    // space around the digits. Parsing that as absent would drop a count the file did state.
    child_text(node, tag).and_then(|s| s.trim().parse().ok())
}

pub(crate) fn find_mdo_element<'a>(
    doc: &'a roxmltree::Document<'a>,
) -> Option<roxmltree::Node<'a, 'a>> {
    doc.root_element().children().find(|n| n.is_element())
}

pub(crate) fn parse_xml(xml: &str) -> Result<roxmltree::Document<'_>> {
    roxmltree::Document::parse(xml)
        .map_err(|e| MetadataError::InvalidFormat(format!("XML parse error: {}", e)))
}

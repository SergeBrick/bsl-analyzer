use crate::command::Command;
use crate::error::{MetadataError, Result};

use super::helpers::{child_text, find_child, find_mdo_element, mdo_uuid, parse_xml};

pub fn parse_command_xml(xml: &str, name: &str) -> Result<Command> {
    let _span = tracing::debug_span!("parse_command_xml", name).entered();

    let doc = parse_xml(xml)?;
    let mdo = find_mdo_element(&doc)
        .ok_or_else(|| MetadataError::InvalidFormat("No Command element found".to_string()))?;

    // The file next to a command module's directory holds `<Command>` (an
    // object's command) or `<CommonCommand>`; any other element means the path
    // led somewhere else, and its uuid must not pass for the command's.
    let kind = mdo.tag_name().name();
    if kind != "Command" && kind != "CommonCommand" {
        return Err(MetadataError::InvalidFormat(format!(
            "Expected a Command or CommonCommand element, found {kind}"
        )));
    }

    let command_name =
        find_child(mdo, "Properties").and_then(|props| child_text(props, "Name")).unwrap_or(name);

    Ok(Command::new(command_name, mdo_uuid(mdo)))
}

/// The command owning a `CommandModule.bsl`, by the XML next to the command's
/// directory: `…/Commands/<Имя>/Ext/CommandModule.bsl` (an object command) and
/// `…/CommonCommands/<Имя>/Ext/CommandModule.bsl` (a common command) share the
/// shape `<collection>/<Имя>.xml`.
pub fn parse_command_from_bsl_path(bsl_path: &std::path::Path) -> Result<Command> {
    let command_dir = bsl_path.parent().and_then(|ext| ext.parent()).ok_or_else(|| {
        MetadataError::InvalidFormat(format!("Invalid command module path: {}", bsl_path.display()))
    })?;
    let name = command_dir.file_name().and_then(|n| n.to_str()).ok_or_else(|| {
        MetadataError::InvalidFormat(format!(
            "Cannot extract command name from: {}",
            bsl_path.display()
        ))
    })?;
    let collection_dir = command_dir.parent().ok_or_else(|| {
        MetadataError::InvalidFormat(format!(
            "Cannot locate the command collection of: {}",
            bsl_path.display()
        ))
    })?;

    let xml_path = bsl_conventions::find_child_ci(collection_dir, &format!("{name}.xml"))
        .unwrap_or_else(|| collection_dir.join(format!("{name}.xml")));
    let xml = std::fs::read_to_string(&xml_path).map_err(|error| {
        MetadataError::InvalidFormat(format!("cannot read {}: {error}", xml_path.display()))
    })?;

    parse_command_xml(&xml, name)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OBJECT_COMMAND_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<MetaDataObject xmlns="http://v8.1c.ru/8.3/MDClasses" version="2.10">
    <Command uuid="3c1a9e64-8a2b-4a2a-9f61-9b6f4b23e111">
        <Properties>
            <Name>ДобавитьРеквизит</Name>
            <Action>ДобавитьРеквизит</Action>
        </Properties>
    </Command>
</MetaDataObject>"#;

    const COMMON_COMMAND_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<MetaDataObject xmlns="http://v8.1c.ru/8.3/MDClasses" version="2.10">
    <CommonCommand uuid="5b9f7d02-c0da-4e02-b48a-1a5f2a6d7222">
        <Properties>
            <Name>АвтономнаяРабота</Name>
        </Properties>
    </CommonCommand>
</MetaDataObject>"#;

    #[test]
    fn parses_an_object_command_with_its_uuid() {
        let command = parse_command_xml(OBJECT_COMMAND_XML, "ДобавитьРеквизит").unwrap();
        assert_eq!(command.name(), "ДобавитьРеквизит");
        assert_eq!(command.uuid().to_string(), "3c1a9e64-8a2b-4a2a-9f61-9b6f4b23e111");
    }

    #[test]
    fn parses_a_common_command_with_its_uuid() {
        let command = parse_command_xml(COMMON_COMMAND_XML, "АвтономнаяРабота").unwrap();
        assert_eq!(command.name(), "АвтономнаяРабота");
        assert_eq!(command.uuid().to_string(), "5b9f7d02-c0da-4e02-b48a-1a5f2a6d7222");
    }

    /// A missing uuid degrades to nil: the command still loads, and a consumer
    /// of the uuid (the vendor-support map) sees an unidentified object.
    #[test]
    fn missing_uuid_degrades_to_nil() {
        let xml = OBJECT_COMMAND_XML.replace(" uuid=\"3c1a9e64-8a2b-4a2a-9f61-9b6f4b23e111\"", "");
        let command = parse_command_xml(&xml, "ДобавитьРеквизит").unwrap();
        assert!(command.uuid().is_nil());
    }

    /// A malformed uuid degrades the same way instead of losing the command.
    #[test]
    fn malformed_uuid_degrades_to_nil() {
        let xml = OBJECT_COMMAND_XML
            .replace("uuid=\"3c1a9e64-8a2b-4a2a-9f61-9b6f4b23e111\"", "uuid=\"не-uuid\"");
        let command = parse_command_xml(&xml, "ДобавитьРеквизит").unwrap();
        assert!(command.uuid().is_nil());
    }

    /// A foreign collection's file is rejected: its uuid must not pass for a
    /// command's owner identity.
    #[test]
    fn a_non_command_element_is_rejected() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<MetaDataObject xmlns="http://v8.1c.ru/8.3/MDClasses" version="2.10">
    <Catalog uuid="aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa">
        <Properties>
            <Name>Товары</Name>
        </Properties>
    </Catalog>
</MetaDataObject>"#;
        assert!(parse_command_xml(xml, "Товары").is_err());
    }

    /// The production shape: the XML found next to the command's directory,
    /// both for an object command (`Catalogs/…/Commands/…`) and a common one.
    #[test]
    fn reads_the_command_from_a_bsl_module_path() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        let write_command = |collection: &str, name: &str, xml: &str| {
            let module_dir = root.join(collection).join(name).join("Ext");
            std::fs::create_dir_all(&module_dir).unwrap();
            std::fs::write(module_dir.join("CommandModule.bsl"), "").unwrap();
            std::fs::write(root.join(collection).join(format!("{name}.xml")), xml).unwrap();
            root.join(collection).join(name).join("Ext").join("CommandModule.bsl")
        };

        let object_module = write_command("Commands", "ДобавитьРеквизит", OBJECT_COMMAND_XML);
        let command = parse_command_from_bsl_path(&object_module).unwrap();
        assert_eq!(command.name(), "ДобавитьРеквизит");
        assert_eq!(command.uuid().to_string(), "3c1a9e64-8a2b-4a2a-9f61-9b6f4b23e111");

        let common_module = write_command("CommonCommands", "АвтономнаяРабота", COMMON_COMMAND_XML);
        let command = parse_command_from_bsl_path(&common_module).unwrap();
        assert_eq!(command.name(), "АвтономнаяРабота");
        assert_eq!(command.uuid().to_string(), "5b9f7d02-c0da-4e02-b48a-1a5f2a6d7222");
    }
}

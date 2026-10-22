use crate::global_state::GlobalState;

/// Shared with the MCP resident and CLI `analyze`: one implementation, so every
/// host analyses under the same effective configuration.
pub use ide_host_core::{
    apply_compatibility_mode_to_db, apply_configuration_xml_settings_to_db, apply_features_to_db,
    apply_ordinary_app_support_to_db, apply_project_config_to_db, apply_project_to_db,
    apply_vendor_support_to_db,
};

impl GlobalState {
    pub fn update_features_config(&mut self) {
        let config =
            self.project.as_ref().map(|project| project.config.clone()).unwrap_or_default();
        apply_project_config_to_db(self.analysis_host.raw_database_mut(), &config);
        self.apply_configuration_xml_settings();
        apply_vendor_support_to_db(self.analysis_host.raw_database_mut(), self.project.as_ref());
        if let Some(project) = self.project.as_ref() {
            if let Some(warning) =
                platform_help::source_change_warning(Some(&project.config), &project.root)
            {
                tracing::warn!("{warning}");
                self.show_warning_message(warning);
            }
        }
    }

    /// Reads the settings the main configuration's `Configuration.xml` states into the
    /// database ([`apply_configuration_xml_settings_to_db`]).
    pub(crate) fn apply_configuration_xml_settings(&mut self) {
        apply_configuration_xml_settings_to_db(
            self.analysis_host.raw_database_mut(),
            self.project.as_ref(),
        );
    }
}

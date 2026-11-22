use serde::Serialize;
use std::path::Path;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpInfo {
    pub data_directory: String,
    pub vault_root: String,
}

pub fn connection_info(data_directory: &Path, vault_root: &Path) -> McpInfo {
    McpInfo {
        data_directory: data_directory.display().to_string(),
        vault_root: vault_root.display().to_string(),
    }
}

use serde::Serialize;

use super::ExtensionInfo;

pub const EXTENSION_DIAGNOSTICS_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Serialize)]
pub struct ExtensionListDocument {
    pub schema_version: u32,
    pub extensions: Vec<ExtensionInfo>,
}

impl ExtensionListDocument {
    pub(crate) fn new(extensions: Vec<ExtensionInfo>) -> Self {
        Self {
            schema_version: EXTENSION_DIAGNOSTICS_SCHEMA_VERSION,
            extensions,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ExtensionCheckDocument {
    pub schema_version: u32,
    pub extension: ExtensionInfo,
}

impl ExtensionCheckDocument {
    pub(crate) fn new(extension: ExtensionInfo) -> Self {
        Self {
            schema_version: EXTENSION_DIAGNOSTICS_SCHEMA_VERSION,
            extension,
        }
    }
}

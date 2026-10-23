use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::clients::ClientId;

/// User-controlled scanner settings loaded from a config file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct ScannerSettings {
    /// Authoritative OpenCode SQLite paths outside its fixed data directory.
    #[serde(default)]
    pub opencode_db_paths: Vec<PathBuf>,
    /// Additional per-client roots keyed by typed public client identity.
    #[serde(default)]
    pub extra_scan_paths: BTreeMap<ClientId, Vec<PathBuf>>,
    pub machine_labels: BTreeMap<PathBuf, String>,
    pub local_machine_label: String,
}

impl Default for ScannerSettings {
    fn default() -> Self {
        Self {
            opencode_db_paths: Vec::new(),
            extra_scan_paths: BTreeMap::new(),
            machine_labels: BTreeMap::new(),
            local_machine_label: "local".into(),
        }
    }
}

impl ScannerSettings {
    pub fn validate(&self) -> Result<(), ScannerSettingsError> {
        if self.local_machine_label.trim().is_empty()
            || self
                .machine_labels
                .values()
                .any(|label| label.trim().is_empty())
        {
            return Err(ScannerSettingsError::EmptyMachineLabel);
        }
        for path in self.machine_labels.keys() {
            if !path.is_absolute() {
                return Err(ScannerSettingsError::RelativePath {
                    setting: "machineLabels".into(),
                    path: path.clone(),
                });
            }
        }
        for path in &self.opencode_db_paths {
            if path.as_os_str().is_empty() {
                return Err(ScannerSettingsError::EmptyPath {
                    setting: "opencodeDbPaths".to_string(),
                });
            }
            if !path.is_absolute() {
                return Err(ScannerSettingsError::RelativePath {
                    setting: "opencodeDbPaths".to_string(),
                    path: path.clone(),
                });
            }
        }
        for (client, paths) in &self.extra_scan_paths {
            // OpenCode accepts explicit database files through opencodeDbPaths,
            // not directory roots that would weaken its discovery contract.
            if *client == ClientId::OpenCode {
                return Err(ScannerSettingsError::UnsupportedClient {
                    client: client.to_string(),
                });
            }
            for path in paths {
                let setting = format!("extraScanPaths.{client}");
                if path.as_os_str().is_empty() {
                    return Err(ScannerSettingsError::EmptyPath { setting });
                }
                if !path.is_absolute() {
                    return Err(ScannerSettingsError::RelativePath {
                        setting,
                        path: path.clone(),
                    });
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ScannerSettingsError {
    #[error("scanner machine labels must not be blank")]
    EmptyMachineLabel,
    #[error("cannot resolve machine root `{path}`: {source}")]
    MachinePath {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("machine root `{path}` has conflicting labels")]
    ConflictingMachineRoot { path: PathBuf },
    #[error("scanner.extraScanPaths client `{client}` does not support extra scan roots")]
    UnsupportedClient { client: String },
    #[error("scanner.{setting} contains an empty path")]
    EmptyPath { setting: String },
    #[error("scanner.{setting} path `{path}` must be absolute")]
    RelativePath { setting: String, path: PathBuf },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_deserialize_from_camel_case_json() {
        let parsed: ScannerSettings = serde_json::from_value(serde_json::json!({
            "opencodeDbPaths": ["/one/opencode.db"],
            "extraScanPaths": {
                "codex": ["/tmp/codex"],
                "gemini": ["/tmp/gemini"]
            }
        }))
        .unwrap();
        assert_eq!(
            parsed.opencode_db_paths,
            vec![PathBuf::from("/one/opencode.db")]
        );
        assert_eq!(
            parsed.extra_scan_paths[&ClientId::Codex],
            vec![PathBuf::from("/tmp/codex")]
        );
    }

    #[test]
    fn settings_reject_opencode_extra_roots() {
        let settings: ScannerSettings = serde_json::from_value(serde_json::json!({
            "extraScanPaths": {
                "opencode": ["/tmp/opencode"]
            }
        }))
        .unwrap();
        assert!(matches!(
            settings.validate(),
            Err(ScannerSettingsError::UnsupportedClient { client })
                if client == "opencode"
        ));
    }

    #[test]
    fn settings_reject_relative_paths_before_cache_identity_is_built() {
        let settings: ScannerSettings = serde_json::from_value(serde_json::json!({
            "opencodeDbPaths": ["relative/opencode.db"],
            "extraScanPaths": {
                "codex": ["relative/codex"]
            }
        }))
        .unwrap();

        assert!(matches!(
            settings.validate(),
            Err(ScannerSettingsError::RelativePath { setting, path })
                if setting == "opencodeDbPaths"
                    && path.as_path() == std::path::Path::new("relative/opencode.db")
        ));

        let settings: ScannerSettings = serde_json::from_value(serde_json::json!({
            "extraScanPaths": {
                "codex": ["relative/codex"]
            }
        }))
        .unwrap();
        assert!(matches!(
            settings.validate(),
            Err(ScannerSettingsError::RelativePath { setting, path })
                if setting == "extraScanPaths.codex"
                    && path.as_path() == std::path::Path::new("relative/codex")
        ));
    }

    #[test]
    fn settings_reject_misspelled_scanner_keys() {
        for value in [
            serde_json::json!({"extraScanPath": {"codex": ["/tmp/codex"]}}),
            serde_json::json!({"opencodeDbPath": ["/tmp/opencode.db"]}),
        ] {
            let error = serde_json::from_value::<ScannerSettings>(value)
                .expect_err("scanner key typos must not silently use defaults");
            assert!(error.to_string().contains("unknown field"));
        }
    }
}

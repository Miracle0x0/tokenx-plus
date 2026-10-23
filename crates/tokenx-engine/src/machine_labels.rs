//! Machine identity comes from configured input roots, never host discovery.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::scanner_settings::{ScannerSettings, ScannerSettingsError};

#[derive(Clone)]
pub(crate) struct MachineLabels {
    roots: Vec<(PathBuf, Arc<str>)>,
    local: Arc<str>,
}

impl Default for MachineLabels {
    fn default() -> Self {
        Self {
            roots: Vec::new(),
            local: crate::records::intern::intern("local"),
        }
    }
}

impl MachineLabels {
    pub(crate) fn resolve(settings: &ScannerSettings) -> Result<Self, ScannerSettingsError> {
        let mut roots = BTreeMap::<PathBuf, Arc<str>>::new();
        for (path, label) in &settings.machine_labels {
            let canonical = match std::fs::canonicalize(path) {
                Ok(path) => path,
                // A label does not discover inputs; an absent root has no matching usage.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => path.clone(),
                Err(source) => {
                    return Err(ScannerSettingsError::MachinePath {
                        path: path.clone(),
                        source,
                    })
                }
            };
            let label = crate::records::intern::intern(label.trim());
            for root in [path.clone(), canonical] {
                let root = comparison_path(&root);
                if roots.get(&root).is_some_and(|existing| existing != &label) {
                    return Err(ScannerSettingsError::ConflictingMachineRoot { path: root });
                }
                roots.insert(root, Arc::clone(&label));
            }
        }
        let mut roots: Vec<_> = roots.into_iter().collect();
        roots.sort_by(|(a, _), (b, _)| {
            b.components()
                .count()
                .cmp(&a.components().count())
                .then_with(|| a.cmp(b))
        });
        Ok(Self {
            roots,
            local: crate::records::intern::intern(settings.local_machine_label.trim()),
        })
    }

    pub(crate) fn label(&self, path: &Path) -> Arc<str> {
        let path = comparison_path(path);
        self.roots
            .iter()
            .find(|(root, _)| path.starts_with(root))
            .map_or_else(|| Arc::clone(&self.local), |(_, label)| Arc::clone(label))
    }
}

#[cfg(not(windows))]
fn comparison_path(path: &Path) -> PathBuf {
    path.to_path_buf()
}

#[cfg(windows)]
fn comparison_path(path: &Path) -> PathBuf {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    let units: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .map(|unit| {
            if (b'A' as u16..=b'Z' as u16).contains(&unit) {
                unit + 32
            } else {
                unit
            }
        })
        .collect();
    std::ffi::OsString::from_wide(&units).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn longest_whole_root_wins_and_sibling_prefix_does_not_match() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("server");
        let nested = root.join("gpu");
        std::fs::create_dir_all(&nested).unwrap();
        let settings = ScannerSettings {
            machine_labels: BTreeMap::from([
                (root.clone(), "server".into()),
                (nested.clone(), "gpu".into()),
            ]),
            local_machine_label: "laptop".into(),
            ..Default::default()
        };
        let labels = MachineLabels::resolve(&settings).unwrap();
        assert_eq!(labels.label(&nested.join("file")).as_ref(), "gpu");
        assert_eq!(labels.label(&root.join("file")).as_ref(), "server");
        assert_eq!(
            labels.label(&dir.path().join("server-copy/file")).as_ref(),
            "laptop"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_roots_match_canonical_inputs_and_conflicting_aliases_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        let link = dir.path().join("alias");
        std::fs::create_dir(&target).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let mut settings = ScannerSettings {
            machine_labels: BTreeMap::from([(link, "remote".into())]),
            ..Default::default()
        };
        assert_eq!(
            MachineLabels::resolve(&settings)
                .unwrap()
                .label(&target.join("session"))
                .as_ref(),
            "remote"
        );
        settings.machine_labels.insert(target, "another".into());
        assert!(matches!(
            MachineLabels::resolve(&settings),
            Err(ScannerSettingsError::ConflictingMachineRoot { .. })
        ));
    }
}

//! Concrete execution boundary for TUI side effects.
//!
//! Transitions enqueue fully prepared effects. This module performs the I/O
//! and returns a typed outcome to the model; no service trait or callback
//! protocol is needed for these process-local operations.

use std::path::PathBuf;

use crate::product_paths::ProductPaths;
use crate::settings::Settings;
use crate::subscription::SubscriptionOutput;

use super::model::TuiModel;

#[derive(Debug)]
pub(crate) enum TuiEffect {
    PersistSettings {
        settings: Settings,
        paths: ProductPaths,
        success_message: String,
    },
    CopyText {
        text: String,
    },
    WriteExport {
        directory: PathBuf,
        path: PathBuf,
        json: String,
    },
    PersistSubscriptionCache {
        path: PathBuf,
        outputs: Vec<SubscriptionOutput>,
    },
}

#[derive(Debug)]
pub(crate) enum EffectOutcome {
    SettingsPersisted {
        success_message: String,
        result: Result<(), String>,
    },
    TextCopied {
        result: Result<(), String>,
    },
    ExportWritten {
        path: PathBuf,
        result: Result<(), String>,
    },
    SubscriptionCachePersisted {
        result: anyhow::Result<()>,
    },
}

pub(crate) fn execute_pending(model: &mut TuiModel) {
    for effect in model.take_effects() {
        let outcome = execute(effect);
        model.apply_effect_outcome(outcome);
    }
}

fn execute(effect: TuiEffect) -> EffectOutcome {
    match effect {
        TuiEffect::PersistSettings {
            settings,
            paths,
            success_message,
        } => EffectOutcome::SettingsPersisted {
            success_message,
            result: settings.save(&paths).map_err(|error| format!("{error:#}")),
        },
        TuiEffect::CopyText { text } => EffectOutcome::TextCopied {
            result: arboard::Clipboard::new()
                .and_then(|mut clipboard| clipboard.set_text(text))
                .map_err(|error| error.to_string()),
        },
        TuiEffect::WriteExport {
            directory,
            path,
            json,
        } => {
            let result = std::fs::create_dir_all(directory)
                .and_then(|_| std::fs::write(&path, json))
                .map_err(|error| error.to_string());
            EffectOutcome::ExportWritten { path, result }
        }
        TuiEffect::PersistSubscriptionCache { path, outputs } => {
            EffectOutcome::SubscriptionCachePersisted {
                result: crate::subscription::cache::save(&path, &outputs),
            }
        }
    }
}

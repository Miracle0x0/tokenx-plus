use std::fs::{self, OpenOptions};
use std::io::Write;

use anyhow::{Context, Result};

use crate::product_paths::ProductPaths;

pub(crate) fn init_model_mappings(paths: &ProductPaths, no_spinner: bool) -> Result<()> {
    let path = paths.model_mappings_file();
    let create = || -> std::io::Result<()> {
        fs::create_dir_all(path.parent().expect("model mappings have a product root"))?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        file.write_all(tokenx_engine::ModelMappings::example_toml().as_bytes())
    };
    let spinner = (!no_spinner)
        .then(|| super::render::LightSpinner::start(rust_i18n::t!("commands.config.creating")));
    let result = create();
    if let Some(spinner) = spinner {
        spinner.stop();
    }
    result.with_context(|| {
        rust_i18n::t!(
            "commands.config.create_error",
            path = path.display().to_string()
        )
    })?;
    println!(
        "{}",
        rust_i18n::t!("commands.config.created", path = path.display().to_string())
    );
    Ok(())
}

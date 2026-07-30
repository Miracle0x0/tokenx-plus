use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Debug)]
struct Catalog {
    path: PathBuf,
    leaves: BTreeMap<String, BTreeSet<String>>,
}

fn main() {
    let locales = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo sets CARGO_MANIFEST_DIR"),
    )
    .join("locales");
    println!("cargo:rerun-if-changed={}", locales.display());

    if let Err(errors) = verify_catalogs(&locales) {
        panic!(
            "locale catalog verification failed:\n{}",
            errors
                .into_iter()
                .map(|error| format!("- {error}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}

fn verify_catalogs(locales: &Path) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    let mut catalogs = BTreeMap::<String, BTreeMap<String, Catalog>>::new();

    let entries = match std::fs::read_dir(locales) {
        Ok(entries) => entries,
        Err(error) => {
            return Err(vec![format!(
                "cannot read locale directory `{}`: {error}",
                locales.display()
            )])
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                errors.push(format!("cannot inspect locale directory entry: {error}"));
                continue;
            }
        };
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("yml") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            errors.push(format!(
                "locale filename is not valid UTF-8: {}",
                path.display()
            ));
            continue;
        };
        let Some((scope, locale)) = stem.rsplit_once('.') else {
            errors.push(format!(
                "locale filename must end in `.<locale>.yml`: {}",
                path.display()
            ));
            continue;
        };
        if scope.is_empty() || locale.is_empty() {
            errors.push(format!("invalid locale filename: {}", path.display()));
            continue;
        }

        match parse_catalog(&path) {
            Ok(catalog) => {
                if let Some(previous) = catalogs
                    .entry(scope.to_string())
                    .or_default()
                    .insert(locale.to_string(), catalog)
                {
                    errors.push(format!(
                        "duplicate `{scope}` catalog for locale `{locale}`: `{}` and `{}`",
                        previous.path.display(),
                        path.display()
                    ));
                }
            }
            Err(mut parse_errors) => errors.append(&mut parse_errors),
        }
    }

    if catalogs.is_empty() {
        errors.push(format!(
            "no locale catalogs found under `{}`",
            locales.display()
        ));
        return Err(errors);
    }

    let supported_locales = catalogs
        .values()
        .flat_map(|by_locale| by_locale.keys().cloned())
        .collect::<BTreeSet<_>>();
    for (scope, by_locale) in &catalogs {
        for locale in &supported_locales {
            if !by_locale.contains_key(locale) {
                errors.push(format!(
                    "catalog scope `{scope}` is missing locale `{locale}`"
                ));
            }
        }
        let Some(source) = by_locale.get("en") else {
            errors.push(format!(
                "catalog scope `{scope}` is missing source locale `en`"
            ));
            continue;
        };
        verify_owner(scope, source, &mut errors);

        for (locale, translated) in by_locale {
            if locale == "en" {
                continue;
            }
            verify_parity(scope, locale, source, translated, &mut errors);
            verify_owner(scope, translated, &mut errors);
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn parse_catalog(path: &Path) -> Result<Catalog, Vec<String>> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) => {
            return Err(vec![format!(
                "cannot read locale catalog `{}`: {error}",
                path.display()
            )])
        }
    };
    let mut errors = Vec::new();
    let mut parents = Vec::<(usize, String)>::new();
    let mut leaves = BTreeMap::new();

    for (index, line) in content.lines().enumerate() {
        let line_number = index + 1;
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        if line.contains('\t') {
            errors.push(format!(
                "{}:{line_number}: tabs are not allowed in locale indentation",
                path.display()
            ));
            continue;
        }
        let indent = line.len() - line.trim_start_matches(' ').len();
        if indent % 2 != 0 {
            errors.push(format!(
                "{}:{line_number}: indentation must use two-space levels",
                path.display()
            ));
        }
        let trimmed = line.trim_start();
        let Some((raw_key, raw_value)) = trimmed.split_once(':') else {
            errors.push(format!(
                "{}:{line_number}: expected a YAML key followed by `:`",
                path.display()
            ));
            continue;
        };
        let key = raw_key.trim();
        if key.is_empty()
            || !key
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
        {
            errors.push(format!(
                "{}:{line_number}: unsupported locale key `{key}`",
                path.display()
            ));
            continue;
        }

        while parents
            .last()
            .is_some_and(|(parent_indent, _)| *parent_indent >= indent)
        {
            parents.pop();
        }
        let full_key = parents
            .iter()
            .map(|(_, key)| key.as_str())
            .chain(std::iter::once(key))
            .collect::<Vec<_>>()
            .join(".");

        if raw_value.trim().is_empty() {
            parents.push((indent, key.to_string()));
            continue;
        }
        let placeholders = extract_placeholders(raw_value, path, line_number, &mut errors);
        if leaves.insert(full_key.clone(), placeholders).is_some() {
            errors.push(format!(
                "{}:{line_number}: duplicate flattened key `{full_key}`",
                path.display()
            ));
        }
    }

    if leaves.is_empty() {
        errors.push(format!(
            "locale catalog `{}` has no leaf keys",
            path.display()
        ));
    }
    if errors.is_empty() {
        Ok(Catalog {
            path: path.to_path_buf(),
            leaves,
        })
    } else {
        Err(errors)
    }
}

fn extract_placeholders(
    value: &str,
    path: &Path,
    line_number: usize,
    errors: &mut Vec<String>,
) -> BTreeSet<String> {
    let mut placeholders = BTreeSet::new();
    let mut remainder = value;
    while let Some(start) = remainder.find("%{") {
        let after_start = &remainder[start + 2..];
        let Some(end) = after_start.find('}') else {
            errors.push(format!(
                "{}:{line_number}: unterminated `%{{...}}` interpolation",
                path.display()
            ));
            break;
        };
        let name = &after_start[..end];
        if name.is_empty()
            || !name
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        {
            errors.push(format!(
                "{}:{line_number}: invalid interpolation name `%{{{name}}}`",
                path.display()
            ));
        } else {
            placeholders.insert(name.to_string());
        }
        remainder = &after_start[end + 1..];
    }
    placeholders
}

fn verify_parity(
    scope: &str,
    locale: &str,
    source: &Catalog,
    translated: &Catalog,
    errors: &mut Vec<String>,
) {
    let source_keys = source.leaves.keys().cloned().collect::<BTreeSet<_>>();
    let translated_keys = translated.leaves.keys().cloned().collect::<BTreeSet<_>>();
    for key in source_keys.difference(&translated_keys) {
        errors.push(format!("`{scope}.{locale}` is missing source key `{key}`"));
    }
    for key in translated_keys.difference(&source_keys) {
        errors.push(format!(
            "`{scope}.{locale}` has key `{key}` absent from source locale `en`"
        ));
    }
    for key in source_keys.intersection(&translated_keys) {
        let source_fields = &source.leaves[key];
        let translated_fields = &translated.leaves[key];
        if source_fields != translated_fields {
            errors.push(format!(
                "`{scope}.{locale}` key `{key}` interpolations differ: en={source_fields:?}, {locale}={translated_fields:?}"
            ));
        }
    }
}

fn verify_owner(scope: &str, catalog: &Catalog, errors: &mut Vec<String>) {
    let expected = owner_prefixes(scope);
    for key in catalog.leaves.keys() {
        if !expected.iter().any(|prefix| key.starts_with(prefix)) {
            errors.push(format!(
                "catalog `{}` owns `{key}`, outside expected prefixes [{}]",
                catalog.path.display(),
                expected.join(", ")
            ));
        }
    }
}

fn owner_prefixes(scope: &str) -> Vec<String> {
    match scope {
        // These consolidated scopes are stable product surfaces with explicit
        // ownership. Extending one requires updating this list deliberately.
        "app-core" => [
            "cache.",
            "main.",
            "paths.",
            "settings.",
            "shared.",
            "theme.",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
        "cli-surface" => ["cli.", "claude_diagnostics."]
            .into_iter()
            .map(str::to_string)
            .collect(),
        "tui-ui-chrome" => ["tui.ui.header.", "tui.ui.footer.", "tui.ui.loading."]
            .into_iter()
            .map(str::to_string)
            .collect(),
        "tui-ui-insights" => [
            "tui.ui.achievements.",
            "tui.ui.bar_chart.",
            "tui.ui.portraits.",
            "tui.ui.profile.",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
        scope if scope.starts_with("tui-ui-") => vec![format!(
            "tui.ui.{}.",
            scope.trim_start_matches("tui-ui-").replace('-', "_")
        )],
        scope if scope.starts_with("tui-") => vec![format!(
            "tui.{}.",
            scope.trim_start_matches("tui-").replace('-', "_")
        )],
        scope => vec![format!("{}.", scope.replace('-', "_"))],
    }
}

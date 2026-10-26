#[path = "../build_support/decoder_contracts.rs"]
mod decoder_contracts;

use std::fs;
use std::path::Path;

use decoder_contracts::{
    contract_fingerprint, generate_decoder_contracts, integration_sources, production_sources,
    write_if_changed, DECODER_CONTRACTS,
};

fn write(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn fingerprint(root: &Path, integration: &str) -> [u8; 32] {
    let shared = vec![root.join("shared.rs")];
    let integration = vec![root.join(integration).join("decode.rs")];
    contract_fingerprint(root, &shared, &integration)
}

fn source_fixture(root: &Path) {
    for file in [
        "lib.rs",
        "acquisition.rs",
        "finalization.rs",
        "model_aliases.rs",
        "provider_identity.rs",
        "token_imputation.rs",
        "records/mod.rs",
        "integrations/mod.rs",
    ] {
        write(&root.join("src").join(file), "// shared source\n");
    }
    for module in ["acquisition", "finalization"] {
        write(
            &root.join(format!("src/{module}/test_support.rs")),
            "#![cfg(test)]\nfn helper() {}\n",
        );
    }
    for spec in DECODER_CONTRACTS {
        write(
            &root.join(format!("src/integrations/{}/decode.rs", spec.integration)),
            "fn decode() {}\n",
        );
    }
}

fn discovered_fingerprint(root: &Path, integration: &str) -> [u8; 32] {
    contract_fingerprint(
        root,
        &production_sources(root),
        &integration_sources(root, integration),
    )
}

#[test]
fn contract_fingerprint_is_deterministic_and_content_addressed() {
    let temp = tempfile::TempDir::new().unwrap();
    let root = temp.path();
    write(&root.join("shared.rs"), "shared-v1");
    write(&root.join("amp/decode.rs"), "amp-v1");

    let first = fingerprint(root, "amp");
    assert_eq!(first, fingerprint(root, "amp"));

    write(&root.join("amp/decode.rs"), "amp-v2");
    assert_ne!(first, fingerprint(root, "amp"));
}

#[test]
fn shared_sources_invalidate_all_contracts_and_integrations_stay_isolated() {
    let temp = tempfile::TempDir::new().unwrap();
    let root = temp.path();
    write(&root.join("shared.rs"), "shared-v1");
    write(&root.join("amp/decode.rs"), "amp-v1");
    write(&root.join("claude/decode.rs"), "claude-v1");

    let amp = fingerprint(root, "amp");
    let claude = fingerprint(root, "claude");

    write(&root.join("amp/decode.rs"), "amp-v2");
    let amp_after_integration_change = fingerprint(root, "amp");
    assert_ne!(amp, amp_after_integration_change);
    assert_eq!(claude, fingerprint(root, "claude"));

    write(&root.join("shared.rs"), "shared-v2");
    assert_ne!(amp_after_integration_change, fingerprint(root, "amp"));
    assert_ne!(claude, fingerprint(root, "claude"));
}

#[test]
fn source_path_is_part_of_the_contract() {
    let temp = tempfile::TempDir::new().unwrap();
    let root = temp.path();
    write(&root.join("shared.rs"), "shared");
    write(&root.join("amp/decode.rs"), "same");
    write(&root.join("claude/decode.rs"), "same");

    let shared = vec![root.join("shared.rs")];
    let amp = vec![root.join("amp/decode.rs")];
    let claude = vec![root.join("claude/decode.rs")];
    assert_ne!(
        contract_fingerprint(root, &shared, &amp),
        contract_fingerprint(root, &shared, &claude)
    );
}

#[test]
fn integration_source_discovery_is_recursive_and_rust_only() {
    let temp = tempfile::TempDir::new().unwrap();
    let root = temp.path();
    write(
        &root.join("src/integrations/amp/decode.rs"),
        "fn decode() {}",
    );
    write(
        &root.join("src/integrations/amp/nested/filter.rs"),
        "fn filter() {}",
    );
    write(&root.join("src/integrations/amp/nested/fixture.json"), "{}");

    let sources = integration_sources(root, "amp");
    assert_eq!(sources.len(), 2);
    assert!(sources[0].ends_with("src/integrations/amp/decode.rs"));
    assert!(sources[1].ends_with("src/integrations/amp/nested/filter.rs"));
}

#[test]
fn only_explicit_file_level_test_markers_are_excluded() {
    let temp = tempfile::TempDir::new().unwrap();
    let root = temp.path();
    source_fixture(root);
    let integration = root.join("src/integrations/amp");
    let marked = integration.join("tests/marked.rs");
    write(&marked, "#![cfg(test)]\nfn test_case() {}\n");
    let unmarked = integration.join("tests.rs");
    write(&unmarked, "fn production_helper() {}\n");
    let nested = integration.join("tests/production.rs");
    write(&nested, "fn production_helper() {}\n");
    let conservative = integration.join("test_support.rs");
    write(&conservative, "// comment\n#![cfg(test)]\nfn helper() {}\n");

    let sources = integration_sources(root, "amp");
    assert!(!sources.contains(&marked));
    assert!(sources.contains(&unmarked));
    assert!(sources.contains(&nested));
    assert!(sources.contains(&conservative));
    let before = discovered_fingerprint(root, "amp");

    write(&marked, "#![cfg(test)]\nfn changed_test_case() {}\n");
    assert_eq!(before, discovered_fingerprint(root, "amp"));

    write(&unmarked, "fn changed_production_helper() {}\n");
    assert_ne!(before, discovered_fingerprint(root, "amp"));
}

#[test]
fn removing_a_test_marker_restores_production_invalidation() {
    let temp = tempfile::TempDir::new().unwrap();
    let root = temp.path();
    source_fixture(root);
    let path = root.join("src/integrations/amp/tests/converted.rs");
    write(&path, "#![cfg(test)]\nfn helper() {}\n");
    let before = discovered_fingerprint(root, "amp");

    write(&path, "fn helper() {}\n");

    assert!(integration_sources(root, "amp").contains(&path));
    assert_ne!(before, discovered_fingerprint(root, "amp"));
}

#[test]
fn shared_test_helpers_do_not_invalidate_but_production_additions_do() {
    let temp = tempfile::TempDir::new().unwrap();
    let root = temp.path();
    source_fixture(root);
    let amp = discovered_fingerprint(root, "amp");
    let claude = discovered_fingerprint(root, "claude");
    let helper = root.join("src/acquisition/test_support.rs");
    assert!(!production_sources(root).contains(&helper));

    write(&helper, "#![cfg(test)]\nfn changed_test_helper() {}\n");
    assert_eq!(amp, discovered_fingerprint(root, "amp"));
    assert_eq!(claude, discovered_fingerprint(root, "claude"));

    let production = root.join("src/acquisition/prepare.rs");
    write(&production, "fn prepare() {}\n");
    assert!(production_sources(root).contains(&production));
    assert_ne!(amp, discovered_fingerprint(root, "amp"));
    assert_ne!(claude, discovered_fingerprint(root, "claude"));
}

#[test]
fn generated_contracts_watch_source_roots_and_include_new_local_sources() {
    let temp = tempfile::TempDir::new().unwrap();
    let root = temp.path();
    source_fixture(root);
    let (before, watched) = generate_decoder_contracts(root);
    for directory in ["integrations", "records", "acquisition", "finalization"] {
        assert!(watched.contains(&root.join("src").join(directory)));
    }
    let claude = discovered_fingerprint(root, "claude");
    let added = root.join("src/integrations/amp/nested/usage.rs");
    write(&added, "fn usage() {}\n");

    let (after, watched) = generate_decoder_contracts(root);
    assert_ne!(before, after);
    assert!(watched.contains(&added));
    assert_eq!(claude, discovered_fingerprint(root, "claude"));
}

#[test]
fn unchanged_generated_files_keep_their_modification_time() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("generated.rs");
    assert!(write_if_changed(&path, b"first contents").unwrap());
    let previous = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(previous)
        .unwrap();

    assert!(!write_if_changed(&path, b"first contents").unwrap());
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), previous);

    assert!(write_if_changed(&path, b"changed contents").unwrap());
    assert_eq!(fs::read(&path).unwrap(), b"changed contents");
    assert_ne!(fs::metadata(&path).unwrap().modified().unwrap(), previous);
}

#[test]
fn generated_file_read_errors_remain_errors() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("generated.rs");
    fs::create_dir(&path).unwrap();

    assert!(write_if_changed(&path, b"contents").is_err());
    assert!(path.is_dir());
}

#[test]
fn declarative_specs_generate_every_decoder_identity_and_watch_every_source() {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let (generated, watched) = generate_decoder_contracts(crate_dir);

    assert_eq!(
        generated
            .matches(" => DecoderContractFingerprint([")
            .count(),
        DECODER_CONTRACTS.len()
    );
    for spec in DECODER_CONTRACTS {
        assert!(generated.contains(&format!("    {},", spec.variant)));
        assert!(generated.contains(&format!(
            "{:?} => Some(Self::{})",
            spec.stable_name, spec.variant
        )));
        assert!(watched.iter().any(|path| {
            path.components()
                .any(|component| component.as_os_str() == spec.integration)
        }));
    }
    assert!(watched
        .iter()
        .any(|path| path.ends_with("src/integrations/decoder.rs")));
    assert!(watched
        .iter()
        .any(|path| path.ends_with("src/records/utils.rs")));
}

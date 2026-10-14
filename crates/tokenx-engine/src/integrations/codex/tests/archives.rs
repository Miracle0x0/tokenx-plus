use super::*;

fn write_archive(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, zstd::encode_all(content.as_bytes(), 0).unwrap()).unwrap();
}

fn discover(home: &Path) -> Vec<DiscoveredInput> {
    DRIVER
        .discover_inputs(&scan_context(home, &Default::default()))
        .unwrap()
}

fn assert_no_source_reads(path: &Path) {
    assert_eq!(
        input_record_cache::get_input_read_stats(path),
        input_record_cache::InputReadStats::default(),
        "a warm archive must not be opened or decompressed"
    );
}

fn scan_with_health(
    home: &Path,
    cache: &mut input_record_cache::InputRecordShardStore,
) -> (
    Vec<AttributedUsageRecord>,
    crate::input_health::HealthSummary,
) {
    let parsed = plan_and_parse(discover(home), cache, None);
    let mut ctx = codex_fold_context(cache, None);
    let mut messages = Vec::new();
    fold_codex_into(parsed, &mut ctx, &mut messages).unwrap();
    (messages, ctx.take_health().summarize())
}

#[test]
fn codex_archives_are_discovered_in_all_roots_with_a_distinct_decoder() {
    let home = tempfile::TempDir::new().unwrap();
    let extra = home.path().join("imports");
    for root in [
        home.path().join(".codex/sessions/2026/10/01"),
        home.path().join(".codex/archived_sessions"),
        extra.clone(),
    ] {
        write_file(&root.join("plain.jsonl"), FIRST_CODEX_ENTRY);
        write_archive(&root.join("archive.jsonl.zst"), FIRST_CODEX_ENTRY);
        for name in ["other.zst", "archive.jsonl.zst.tmp", "archive.jsonl.zstd"] {
            write_file(&root.join(name), "not an accepted Codex input");
        }
    }
    let settings = crate::scanner::ScannerSettings {
        extra_scan_paths: BTreeMap::from([(ClientId::Codex, vec![extra])]),
        ..Default::default()
    };
    let units = DRIVER
        .discover_inputs(&scan_context(home.path(), &settings))
        .unwrap();
    assert_eq!(units.len(), 6);
    assert_eq!(
        units
            .iter()
            .filter(|unit| unit.decoder == DecoderKind::CodexArchive)
            .count(),
        3
    );
    for unit in &units {
        assert_eq!(unit.decoder, codex_decoder(&unit.path));
    }
    assert_ne!(
        DecoderKind::CodexArchive.version(),
        DecoderKind::codex().version()
    );
    assert_ne!(
        DecoderKind::CodexArchive.fingerprint_identity(),
        DecoderKind::codex().fingerprint_identity()
    );
}

#[test]
fn codex_archives_cache_usage_and_empty_results_without_reading_source_on_warm_hits() {
    let home = tempfile::TempDir::new().unwrap();
    let cache_home = tempfile::TempDir::new().unwrap();
    for (name, content) in [
        ("usage", FIRST_CODEX_ENTRY),
        ("empty", EMPTY_CODEX_ENTRY),
        ("no-newline", FIRST_CODEX_ENTRY.trim_end()),
        ("no-records", ""),
    ] {
        let plain = home.path().join(format!("{name}.jsonl"));
        let archive = home.path().join(format!("{name}.jsonl.zst"));
        write_file(&plain, content);
        write_archive(&archive, content);
        let expected = parser_messages(&plain);
        let mut cache =
            input_record_cache::InputRecordShardStore::with_cache_dir(cache_home.path());

        input_record_cache::reset_input_read_stats(&archive);
        let cold = parse_and_fold(vec![codex_unit(&archive)], &mut cache);
        assert_output_matches_parser(&cold, &expected);
        assert_eq!(
            input_record_cache::get_input_read_stats(&archive).bytes,
            std::fs::metadata(&archive).unwrap().len()
        );
        let meta = cache
            .get_meta(&archive, DecoderKind::CodexArchive.version())
            .unwrap()
            .unwrap();
        assert!(meta.codex_incremental.is_none());
        assert!(meta.fingerprint.primary_digest().is_none());

        let mut warm_cache =
            input_record_cache::InputRecordShardStore::with_cache_dir(cache_home.path());
        input_record_cache::reset_input_read_stats(&archive);
        let warm = parse_and_fold(vec![codex_unit(&archive)], &mut warm_cache);
        assert_eq!(warm, cold);
        assert_no_source_reads(&archive);

        let mut repricing_cache =
            input_record_cache::InputRecordShardStore::with_cache_dir(cache_home.path());
        let repriced = parse_and_fold_with_pricing(
            vec![codex_unit(&archive)],
            &mut repricing_cache,
            &pricing_service(0.5),
        );
        assert_eq!(repriced.len(), expected.len());
        if let Some(message) = repriced.first() {
            assert!(message.cost > 0.0);
        }
        assert_no_source_reads(&archive);
    }
}

#[test]
fn codex_archive_changes_reparse_all_frames_without_reusing_append_offsets() {
    let home = tempfile::TempDir::new().unwrap();
    let cache_home = tempfile::TempDir::new().unwrap();
    let archive = home.path().join("session.jsonl.zst");
    write_archive(&archive, FIRST_CODEX_ENTRY);
    let mut cache = input_record_cache::InputRecordShardStore::with_cache_dir(cache_home.path());
    assert_eq!(
        parse_and_fold(vec![codex_unit(&archive)], &mut cache).len(),
        1
    );

    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&archive)
        .unwrap();
    file.write_all(&zstd::encode_all(APPENDED_CODEX_ENTRY.as_bytes(), 0).unwrap())
        .unwrap();
    file.flush().unwrap();
    input_record_cache::reset_input_read_stats(&archive);
    let updated = parse_and_fold(vec![codex_unit(&archive)], &mut cache);
    assert_eq!(updated.len(), 2);
    assert_eq!(
        input_record_cache::get_input_read_stats(&archive).bytes,
        std::fs::metadata(&archive).unwrap().len()
    );
    let plain = home.path().join("session.jsonl");
    write_file(
        &plain,
        &format!("{FIRST_CODEX_ENTRY}{APPENDED_CODEX_ENTRY}"),
    );
    assert_output_matches_parser(&updated, &parser_messages(&plain));

    // Replacing an archive with a shorter log must discard the old records.
    write_archive(&archive, FIRST_CODEX_ENTRY);
    let replaced = parse_and_fold(vec![codex_unit(&archive)], &mut cache);
    assert_eq!(replaced.len(), 1);
    input_record_cache::reset_input_read_stats(&archive);
    assert_eq!(
        parse_and_fold(vec![codex_unit(&archive)], &mut cache),
        replaced
    );
    assert_no_source_reads(&archive);
}

#[test]
fn codex_plain_to_archive_migration_keeps_session_identity_and_deduplicates_copies() {
    let home = tempfile::TempDir::new().unwrap();
    let cache_home = tempfile::TempDir::new().unwrap();
    let plain = home.path().join(".codex/sessions/session.jsonl");
    let archive = home
        .path()
        .join(".codex/archived_sessions/session.jsonl.zst");
    write_file(&plain, FIRST_CODEX_ENTRY);
    let mut cache = input_record_cache::InputRecordShardStore::with_cache_dir(cache_home.path());
    let before = parse_and_fold_batched(discover(home.path()), &mut cache);

    write_archive(&archive, FIRST_CODEX_ENTRY);
    assert_eq!(
        parse_and_fold_batched(discover(home.path()), &mut cache),
        before
    );
    assert_eq!(before[0].session_id.as_ref(), "session");
    // The archive's raw shard must contain its records even when its copy was
    // deduplicated from the first report that scanned both physical inputs.
    std::fs::remove_file(&plain).unwrap();
    input_record_cache::reset_input_read_stats(&archive);
    assert_eq!(
        parse_and_fold_batched(discover(home.path()), &mut cache),
        before
    );
    assert_no_source_reads(&archive);
}

#[test]
fn damaged_codex_archives_report_health_keep_confirmed_usage_and_are_retried() {
    let home = tempfile::TempDir::new().unwrap();
    let cache_home = tempfile::TempDir::new().unwrap();
    let healthy = home.path().join(".codex/sessions/healthy.jsonl");
    let partial = home.path().join(".codex/sessions/partial.jsonl.zst");
    let broken = home.path().join(".codex/sessions/broken.jsonl.zst");
    write_file(&healthy, FIRST_CODEX_ENTRY);
    write_archive(&partial, FIRST_CODEX_ENTRY);
    let mut tail = zstd::encode_all(APPENDED_CODEX_ENTRY.as_bytes(), 0).unwrap();
    tail.truncate(tail.len() / 2);
    std::fs::OpenOptions::new()
        .append(true)
        .open(&partial)
        .unwrap()
        .write_all(&tail)
        .unwrap();
    write_file(&broken, "not a Zstandard frame");
    let mut cache = input_record_cache::InputRecordShardStore::with_cache_dir(cache_home.path());

    for _ in 0..2 {
        input_record_cache::reset_input_read_stats(&partial);
        let (messages, health) = scan_with_health(home.path(), &mut cache);
        assert_eq!(messages.len(), 2);
        assert_eq!(health.partial_inputs(), 1);
        assert_eq!(health.failed_inputs(), 1);
        for path in [&partial, &broken] {
            assert!(cache
                .get_meta(path, DecoderKind::CodexArchive.version())
                .unwrap()
                .is_none());
        }
        assert!(input_record_cache::get_input_read_stats(&partial).bytes > 0);
    }
    write_archive(&partial, FIRST_CODEX_ENTRY);
    write_archive(&broken, FIRST_CODEX_ENTRY);
    let (messages, health) = scan_with_health(home.path(), &mut cache);
    assert_eq!(messages.len(), 3);
    assert_eq!(health.partial_inputs(), 0);
    assert_eq!(health.failed_inputs(), 0);
}

#[test]
fn codex_archive_cache_damage_reparses_source_and_exposes_the_cache_failure() {
    let home = tempfile::TempDir::new().unwrap();
    let cache_home = tempfile::TempDir::new().unwrap();
    let archive = home.path().join(".codex/sessions/session.jsonl.zst");
    write_archive(&archive, FIRST_CODEX_ENTRY);
    let version = DecoderKind::CodexArchive.version();
    let mut cache = input_record_cache::InputRecordShardStore::with_cache_dir(cache_home.path());
    let original = parse_and_fold(vec![codex_unit(&archive)], &mut cache);
    input_record_cache::truncate_shard_after_header_for_test(cache_home.path(), &archive, version);

    let mut repair_cache =
        input_record_cache::InputRecordShardStore::with_cache_dir(cache_home.path());
    input_record_cache::reset_input_read_stats(&archive);
    let (repaired, health) = scan_with_health(home.path(), &mut repair_cache);
    assert_eq!(repaired, original);
    assert!(health
        .issues
        .iter()
        .any(|issue| issue.issue == "input-cache-read-failed"));
    assert!(input_record_cache::get_input_read_stats(&archive).bytes > 0);

    let mut warm_cache =
        input_record_cache::InputRecordShardStore::with_cache_dir(cache_home.path());
    input_record_cache::reset_input_read_stats(&archive);
    let (warm, health) = scan_with_health(home.path(), &mut warm_cache);
    assert_eq!(warm, original);
    assert!(health.issues.is_empty());
    assert_no_source_reads(&archive);
}

#[test]
fn codex_archive_mutation_after_preparation_is_partial_and_not_cached() {
    let home = tempfile::TempDir::new().unwrap();
    let cache_home = tempfile::TempDir::new().unwrap();
    let archive = home.path().join("session.jsonl.zst");
    write_archive(&archive, FIRST_CODEX_ENTRY);
    let mut cache = input_record_cache::InputRecordShardStore::with_cache_dir(cache_home.path());
    let miss = expect_codex_miss(
        DRIVER.plan_cache_hit(prepared_codex_unit(&archive), &cache),
        "new archive must be decoded",
    );
    write_archive(
        &archive,
        &format!("{FIRST_CODEX_ENTRY}{APPENDED_CODEX_ENTRY}"),
    );
    let parsed = DRIVER.parse_inputs(vec![miss], &ParseContext::uncancelled(None));
    assert!(matches!(
        parsed[0].health.status,
        InputStatus::Partial { .. }
    ));
    assert!(parsed[0].cache_write.is_none());
    assert_eq!(fold_parsed(parsed, &mut cache).len(), 2);
    assert!(cache
        .get_meta(&archive, DecoderKind::CodexArchive.version())
        .unwrap()
        .is_none());
}

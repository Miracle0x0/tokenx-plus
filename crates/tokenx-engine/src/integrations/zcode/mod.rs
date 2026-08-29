pub(crate) mod decode;
pub(crate) mod sqlite;

use std::collections::HashSet;
use std::path::Path;

use rayon::prelude::*;

use crate::input_record_cache::DecoderId;
use crate::integrations::cache as pipeline_cache;
use crate::integrations::discover as source_discovery;
use crate::integrations::{
    BoundUsageSink, DecoderKind, DiscoveredInput, DiscoveryContext, FingerprintPolicy, FoldContext,
    InputDiscoveryError, InputPipelineError, IntegrationDriver, ParseContext, ParsedBatchInput,
    ParsedUnit, SourceSpec,
};

const JSONL_SOURCE: SourceSpec = SourceSpec::home(
    ".zcode/projects",
    crate::integrations::SourceMatcher::new(crate::integrations::source_matchers::jsonl),
);
const SQLITE_RELATIVE_PATH: &str = ".zcode/cli/db/db.sqlite";

pub(crate) struct Driver;

impl IntegrationDriver for Driver {
    fn discover_inputs(
        &self,
        ctx: &DiscoveryContext<'_>,
    ) -> Result<Vec<DiscoveredInput>, InputDiscoveryError> {
        let client = ctx.client;
        let extra_roots = source_discovery::extra_roots_for_client(client, ctx)?;

        let mut jsonl_paths = source_discovery::scan_roots(
            ctx,
            [JSONL_SOURCE.resolve(ctx.home_dir)],
            JSONL_SOURCE.matcher(),
        )?;
        jsonl_paths.extend(source_discovery::scan_roots(
            ctx,
            extra_roots.iter().cloned(),
            JSONL_SOURCE.matcher(),
        )?);

        let mut sqlite_paths = Vec::new();
        source_discovery::push_existing_file(
            client,
            ctx.home_dir.join(SQLITE_RELATIVE_PATH),
            &mut sqlite_paths,
        )?;
        sqlite_paths.extend(source_discovery::scan_roots(
            ctx,
            extra_roots,
            crate::integrations::SourceMatcher::new(is_zcode_sqlite),
        )?);

        let mut units = source_discovery::input_units_from_paths(
            client,
            jsonl_paths,
            FingerprintPolicy::PlainFile,
            DecoderKind::plain(DecoderId::Zcode),
        )?;
        units.extend(source_discovery::input_units_from_paths(
            client,
            sqlite_paths,
            FingerprintPolicy::SqliteWithWal,
            DecoderKind::zcode_sqlite(),
        )?);
        units.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(units)
    }

    fn parse_inputs(
        &self,
        units: Vec<crate::integrations::ExecutionInput>,
        ctx: &ParseContext<'_>,
    ) -> Vec<ParsedUnit> {
        units
            .into_par_iter()
            .map(|unit| match unit.decoder {
                DecoderKind::Plain {
                    decoder_id: DecoderId::Zcode,
                } => pipeline_cache::load_or_scan_unit_with(unit, ctx, decode::parse_zcode_file),
                DecoderKind::ZcodeSqlite => {
                    pipeline_cache::load_or_scan_unit_with(unit, ctx, sqlite::parse_zcode_sqlite)
                }
                _ => unreachable!("unexpected ZCode decoder"),
            })
            .collect()
    }

    fn plan_cache_hit(
        &self,
        unit: crate::integrations::PreparedInput,
        input_cache: &crate::input_record_cache::InputRecordShardStore,
    ) -> Result<crate::integrations::CacheHitPlan, crate::integrations::InputPlanningError> {
        pipeline_cache::plan_cache_hit(unit, input_cache)
    }

    fn fold(
        &self,
        parsed: Vec<ParsedUnit>,
        ctx: &mut FoldContext<'_>,
        sink: &mut BoundUsageSink<'_>,
    ) -> Result<(), InputPipelineError> {
        let mut seen = HashSet::new();
        fold_zcode_units(parsed, ctx, sink, &mut seen)
    }

    fn fold_batches(
        &self,
        batches: &mut ParsedBatchInput,
        ctx: &mut FoldContext<'_>,
        sink: &mut BoundUsageSink<'_>,
    ) -> Result<(), InputPipelineError> {
        let mut seen = HashSet::new();
        while let Some(parsed) = batches.next(ctx)? {
            fold_zcode_units(parsed, ctx, sink, &mut seen)?;
        }
        Ok(())
    }
}

fn is_zcode_sqlite(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == "db.sqlite")
}

fn fold_zcode_units(
    parsed: Vec<ParsedUnit>,
    ctx: &mut FoldContext<'_>,
    sink: &mut BoundUsageSink<'_>,
    seen: &mut HashSet<u64>,
) -> Result<(), InputPipelineError> {
    pipeline_cache::fold_units_with_filter(parsed, ctx, sink, |_, messages| {
        messages
            .into_iter()
            .filter(|message| crate::should_keep_deduped_message(seen, message))
            .collect()
    })
}

pub(crate) static DRIVER: Driver = Driver;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clients::ClientId;
    use crate::input_record_cache;
    use crate::input_record_cache::DecoderVersion;
    use rusqlite::Connection;

    fn write_file(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
    }

    fn write_usage_database(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE model_usage (
                id TEXT PRIMARY KEY,
                session_id TEXT,
                turn_id TEXT,
                model_id TEXT,
                started_at INTEGER,
                completed_at INTEGER,
                duration_ms INTEGER,
                input_tokens INTEGER,
                output_tokens INTEGER,
                reasoning_tokens INTEGER,
                cache_read_input_tokens INTEGER,
                cache_creation_input_tokens INTEGER,
                computed_total_tokens INTEGER,
                agent TEXT,
                mode TEXT
            );
            INSERT INTO model_usage (
                id, session_id, turn_id, model_id, started_at, completed_at,
                input_tokens, output_tokens
            ) VALUES (
                'usage-1', 'session-1', 'turn-1', 'GLM-5.2', 1000, 1100, 10, 2
            );",
        )
        .unwrap();
    }

    #[test]
    fn discovers_v2_sqlite_and_legacy_jsonl_inputs() {
        let home = tempfile::TempDir::new().unwrap();
        let default_db = home.path().join(SQLITE_RELATIVE_PATH);
        let default_jsonl = home.path().join(".zcode/projects/project-a/session.jsonl");
        let extra_root = home.path().join("imports/zcode");
        let extra_db = extra_root.join("profile/db.sqlite");
        let extra_jsonl = extra_root.join("project-b/session.jsonl");
        let ignored_db = extra_root.join("profile/other.sqlite");
        for path in [
            &default_db,
            &default_jsonl,
            &extra_db,
            &extra_jsonl,
            &ignored_db,
        ] {
            write_file(path);
        }

        let mut extra_scan_paths = std::collections::BTreeMap::new();
        extra_scan_paths.insert(ClientId::Zcode, vec![extra_root]);
        let settings = crate::scanner::ScannerSettings {
            extra_scan_paths,
            ..Default::default()
        };
        let ctx = DiscoveryContext {
            client: ClientId::Zcode,
            home_dir: home.path(),
            scanner_settings: &settings,
            cancellation: crate::engine::AcquisitionCancellation::default(),
        };

        let units = DRIVER.discover_inputs(&ctx).unwrap();

        assert_eq!(
            units
                .iter()
                .map(|unit| unit.path.clone())
                .collect::<Vec<_>>(),
            vec![default_db, default_jsonl, extra_db, extra_jsonl]
        );
        for unit in units {
            if is_zcode_sqlite(&unit.path) {
                assert!(matches!(unit.decoder, DecoderKind::ZcodeSqlite));
                assert_eq!(
                    unit.decoder.version(),
                    DecoderVersion::current(DecoderId::ZcodeSqlite)
                );
                assert_eq!(unit.fingerprint_policy, FingerprintPolicy::SqliteWithWal);
            } else {
                assert!(matches!(
                    unit.decoder,
                    DecoderKind::Plain {
                        decoder_id: DecoderId::Zcode
                    }
                ));
                assert_eq!(
                    unit.decoder.version(),
                    DecoderVersion::current(DecoderId::Zcode)
                );
                assert_eq!(unit.fingerprint_policy, FingerprintPolicy::PlainFile);
            }
        }
    }

    #[test]
    fn sqlite_driver_finalizes_identity_and_deduplicates_copied_rows() {
        let dir = tempfile::TempDir::new().unwrap();
        let first = dir.path().join("first/db.sqlite");
        let second = dir.path().join("second/db.sqlite");
        write_usage_database(&first);
        write_usage_database(&second);
        let units = [first, second]
            .into_iter()
            .map(|path| DiscoveredInput::sqlite_with_wal(path, DecoderKind::zcode_sqlite()))
            .collect();
        let parsed = DRIVER.parse_inputs(
            crate::integrations::test_execute_all(units),
            &ParseContext::uncancelled(None),
        );
        let mut cache = input_record_cache::InputRecordShardStore::default();
        let binding = crate::integrations::integration_for(ClientId::Zcode);
        let mut fold_ctx = FoldContext::new(binding, &mut cache, None);
        let mut messages = Vec::new();
        let mut sink = BoundUsageSink::new(binding, &mut messages);

        DRIVER.fold(parsed, &mut fold_ctx, &mut sink).unwrap();

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].client, ClientId::Zcode);
        assert_eq!(messages[0].model_id.as_ref(), "glm-5.2");
        assert_eq!(messages[0].provider_id.as_ref(), "zai");
        assert_eq!(messages[0].tokens.total(), 12);
        assert_eq!(fold_ctx.health().failed_inputs(), 0);
        assert_eq!(fold_ctx.health().partial_inputs(), 0);
    }
}

pub(crate) mod decode;
mod merge;

use std::collections::HashSet;

use rayon::prelude::*;

use crate::integrations::cache;
use crate::integrations::discover;
use crate::integrations::{
    BoundUsageSink, CopilotWorkspaceScope, DecoderKind, DiscoveredInput, DiscoveryContext,
    FingerprintPolicy, FoldContext, InputDiscoveryError, InputPipelineError, IntegrationDriver,
    ParseContext, ParsedBatchInput, ParsedUnit, SourceSpec,
};

const SOURCE: SourceSpec = SourceSpec::home(
    ".copilot/otel",
    crate::integrations::SourceMatcher::new(crate::integrations::source_matchers::jsonl),
);
pub(crate) struct Driver;

pub(crate) static DRIVER: Driver = Driver;

impl IntegrationDriver for Driver {
    fn discover_inputs(
        &self,
        ctx: &DiscoveryContext<'_>,
    ) -> Result<Vec<DiscoveredInput>, InputDiscoveryError> {
        let client = ctx.client;
        let default_root = SOURCE.resolve(ctx.home_dir);
        let default_paths = discover::scan_roots(ctx, [default_root], SOURCE.matcher())?;
        let extra_paths = discover::scan_roots(
            ctx,
            discover::extra_roots_for_client(client, ctx)?,
            SOURCE.matcher(),
        )?;

        let mut units = discover::input_units_from_paths(
            client,
            default_paths,
            FingerprintPolicy::PlainFile,
            DecoderKind::copilot(CopilotWorkspaceScope::BuiltInPlatform),
        )?;
        units.extend(discover::input_units_from_paths(
            client,
            extra_paths,
            FingerprintPolicy::PlainFile,
            DecoderKind::copilot(CopilotWorkspaceScope::ExplicitRoot),
        )?);
        dedup_units_by_canonical_path(&mut units)?;
        units.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(units)
    }

    fn parse_inputs(
        &self,
        units: Vec<crate::integrations::ExecutionInput>,
        ctx: &ParseContext<'_>,
    ) -> Vec<ParsedUnit> {
        let workspace_index = decode::CopilotWorkspaceIndex::discover(units.iter().map(|unit| {
            let DecoderKind::Copilot {
                workspace_scope, ..
            } = unit.decoder
            else {
                unreachable!("unexpected Copilot decoder");
            };
            (unit.path.as_path(), workspace_scope)
        }));
        units
            .into_par_iter()
            .map(|unit| {
                cache::load_or_scan_unit_with(unit, ctx, |path| {
                    decode::parse_copilot_file_with_workspace_index(path, &workspace_index)
                })
            })
            .collect()
    }

    fn plan_cache_hit(
        &self,
        unit: crate::integrations::PreparedInput,
        input_cache: &crate::input_record_cache::InputRecordShardStore,
    ) -> Result<crate::integrations::CacheHitPlan, crate::integrations::InputPlanningError> {
        cache::plan_cache_hit(unit, input_cache)
    }

    fn fold(
        &self,
        parsed: Vec<ParsedUnit>,
        ctx: &mut FoldContext<'_>,
        sink: &mut BoundUsageSink<'_>,
    ) -> Result<(), InputPipelineError> {
        if parsed.is_empty() {
            return Ok(());
        }
        let mut fold = CopilotFold::new()?;
        fold.consume(parsed, ctx)?;
        fold.finish(ctx, sink)
    }

    fn fold_batches(
        &self,
        batches: &mut ParsedBatchInput,
        ctx: &mut FoldContext<'_>,
        sink: &mut BoundUsageSink<'_>,
    ) -> Result<(), InputPipelineError> {
        let Some(parsed) = batches.next(ctx)? else {
            return Ok(());
        };
        let mut fold = CopilotFold::new()?;
        fold.consume(parsed, ctx)?;
        while let Some(parsed) = batches.next(ctx)? {
            fold.consume(parsed, ctx)?;
        }
        fold.finish(ctx, sink)
    }
}

struct CopilotFold {
    merged: merge::CopilotMerge,
    health: Vec<(
        std::path::PathBuf,
        crate::input_health::InputStatus,
        crate::input_health::RejectionSummary,
    )>,
}

impl CopilotFold {
    fn new() -> Result<Self, InputPipelineError> {
        Ok(Self {
            merged: merge::CopilotMerge::new()
                .map_err(InputPipelineError::CopilotReconciliation)?,
            health: Vec::new(),
        })
    }

    fn consume(
        &mut self,
        parsed: Vec<ParsedUnit>,
        ctx: &mut FoldContext<'_>,
    ) -> Result<(), InputPipelineError> {
        for parsed in parsed {
            let cache::ResolvedUnit {
                unit,
                mut messages,
                cache_write,
                invalidate_cache,
                status,
                mut rejections,
            } = cache::resolve_unit(parsed, ctx)?;
            rejections.merge(&crate::retain_source_eligible_messages(&mut messages));
            let cache_write =
                cache_write.map(|plan| Box::new(plan.with_rejections(rejections.clone())));
            let outcome = cache::write_cache(cache_write, ctx, &messages);
            for message in messages {
                ctx.cancellation()
                    .check(crate::engine::AcquisitionPhase::Folding)?;
                self.merged
                    .push(self.health.len(), message)
                    .map_err(InputPipelineError::CopilotReconciliation)?;
            }
            if outcome == cache::CacheWriteOutcome::NotPlanned && invalidate_cache {
                ctx.input_cache.remove(&unit.path, unit.decoder.version());
            }
            self.health.push((unit.path, status, rejections));
        }
        Ok(())
    }

    fn finish(
        mut self,
        ctx: &mut FoldContext<'_>,
        sink: &mut BoundUsageSink<'_>,
    ) -> Result<(), InputPipelineError> {
        let groups = self
            .merged
            .into_groups()
            .map_err(InputPipelineError::CopilotReconciliation)?;
        for group in groups {
            ctx.cancellation()
                .check(crate::engine::AcquisitionPhase::Folding)?;
            for (owner, message) in group.map_err(InputPipelineError::CopilotReconciliation)? {
                let mut messages = vec![message];
                let rejections = &mut self.health[owner].2;
                rejections.merge(&crate::retain_source_eligible_messages(&mut messages));
                rejections.merge(&crate::price_source_eligible_messages(
                    &mut messages,
                    ctx.pricing,
                    &ctx.model_mappings,
                ));
                rejections.merge(&cache::emit_messages(messages, sink));
            }
        }
        for (path, status, rejections) in self.health {
            ctx.record_health(path, status, rejections);
        }
        Ok(())
    }
}

fn dedup_units_by_canonical_path(
    units: &mut Vec<DiscoveredInput>,
) -> Result<(), InputDiscoveryError> {
    let mut seen = HashSet::new();
    let mut keys = Vec::with_capacity(units.len());
    for unit in units.iter() {
        keys.push(std::fs::canonicalize(&unit.path).map_err(|source| {
            InputDiscoveryError::new(&unit.path, "canonicalize discovered input", source)
        })?);
    }
    let mut index = 0;
    units.retain(|_| {
        let keep = seen.insert(keys[index].clone());
        index += 1;
        keep
    });
    Ok(())
}

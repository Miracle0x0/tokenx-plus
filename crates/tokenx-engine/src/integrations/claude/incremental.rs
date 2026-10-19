//! Verified append parsing with raw, single-copy record shards.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::decode::{
    self, ClaudeParseState, ClaudeProjectDependency, ClaudeProjectResolver, RawClaudeScan,
};
use crate::input_health::{InputDiagnosticKind, InputFailure, InputStatus};
use crate::input_record_cache::{
    self, CacheReadPlan, CacheWritePlan, InputFileIdentity, InputSnapshot,
};
use crate::integrations::{
    cache, CacheHitPlan, DiscoveredInput, ExecutionInput, FingerprintPolicy, FoldContext,
    InputPipelineError, InputPlanningError, ParseContext, ParsedUnit, PreparedInput,
    UnitRecordPayload,
};
use crate::records::{
    error::{SessionParseError, SessionParseResult},
    UsageRecord,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ClaudeCheckpoint {
    state: ClaudeParseState,
    consumed_offset: u64,
    prefix_hash: [u8; 32],
    project: decode::ClaudeProjectContext,
}

#[derive(Debug)]
pub(crate) enum ClaudeInput {
    Fresh {
        scan: RawClaudeScan,
        snapshot: Option<InputSnapshot>,
        identity: InputFileIdentity,
        cache_enabled: bool,
    },
    Cached {
        read_plan: CacheReadPlan,
        checkpoint: ClaudeCheckpoint,
        append: Option<InputSnapshot>,
    },
}

pub(super) fn plan_cache_hit(
    unit: PreparedInput,
    store: &input_record_cache::InputRecordShardStore,
) -> Result<CacheHitPlan, InputPlanningError> {
    if matches!(unit.fingerprint_policy, FingerprintPolicy::NoRecordCache) || store.is_disabled() {
        return Ok(CacheHitPlan::Miss(unit.into_bypass_execution()));
    }
    let cached = match store.get_meta(&unit.path, unit.decoder.version()) {
        Ok(Some(cached)) => cached,
        Ok(None) | Err(_) => return Ok(CacheHitPlan::Miss(unit.into_lookup_miss())),
    };
    let stamp = match unit.input_policy().stamp_from_snapshot(unit.snapshot()) {
        Ok(stamp) => stamp,
        Err(_) if unit.preserves_primary_on_related_failure() => {
            return Ok(CacheHitPlan::Miss(unit.into_lookup_miss()))
        }
        Err(error) => return Err(error.into()),
    };
    if stamp != cached.fingerprint.stamp {
        if cached
            .claude_checkpoint
            .as_ref()
            .is_some_and(|c| cached.fingerprint.stamp.primary_size() == Some(c.consumed_offset))
            && stamp.is_append_of(&cached.fingerprint.stamp)
        {
            return Ok(CacheHitPlan::Miss(unit.into_candidate(Box::new(cached))));
        }
        return Ok(CacheHitPlan::Miss(unit.into_lookup_miss()));
    }
    let read_plan = CacheReadPlan::new(&unit.path, unit.decoder.version(), cached.fingerprint);
    let payload = match cached.claude_checkpoint {
        Some(checkpoint) => UnitRecordPayload::Claude(Box::new(ClaudeInput::Cached {
            read_plan,
            checkpoint: *checkpoint,
            append: None,
        })),
        None => UnitRecordPayload::CacheHit(read_plan),
    };
    let mut parsed = ParsedUnit::healthy(unit.into_discovered(), payload, None, false);
    parsed.health.rejections = cached.rejections;
    Ok(CacheHitPlan::Hit(parsed))
}

pub(super) fn parse_unit(
    mut unit: ExecutionInput,
    resolver: &ClaudeProjectResolver,
    ctx: &ParseContext<'_>,
) -> ParsedUnit {
    if decode::is_workflow_journal(&unit.path) {
        return cache::load_or_scan_unit_with(unit, ctx, |path| {
            decode::parse_claude_file_with_project_resolver_and_cancellation(
                path,
                resolver,
                Some(ctx.cancellation()),
            )
            .map(|(scan, _)| scan)
        });
    }
    let snapshot = unit.snapshot().cloned();
    if let Some(cached) = unit.take_cache_candidate() {
        if let Some(checkpoint) = cached.claude_checkpoint {
            let read_plan =
                CacheReadPlan::new(&unit.path, unit.decoder.version(), cached.fingerprint);
            return ParsedUnit::healthy(
                unit,
                UnitRecordPayload::Claude(Box::new(ClaudeInput::Cached {
                    read_plan,
                    checkpoint: *checkpoint,
                    append: snapshot,
                })),
                None,
                false,
            );
        }
    }
    let cache_enabled = !unit.bypasses_cache();
    match read_full(&unit.path, resolver, cache_enabled, ctx.cancellation()) {
        Ok((scan, identity)) => {
            let health = Box::new(scan.health());
            let mut parsed = ParsedUnit::healthy(
                unit,
                UnitRecordPayload::Claude(Box::new(ClaudeInput::Fresh {
                    scan,
                    snapshot,
                    identity,
                    cache_enabled,
                })),
                None,
                false,
            );
            parsed.health = health;
            parsed
        }
        Err(error) => ParsedUnit::unavailable(unit, InputFailure::from(&error)),
    }
}

fn read_full(
    path: &Path,
    resolver: &ClaudeProjectResolver,
    hash: bool,
    cancellation: &crate::AcquisitionCancellation,
) -> SessionParseResult<(RawClaudeScan, InputFileIdentity)> {
    let file = File::open(path)
        .map_err(|error| SessionParseError::at_path(path, "open Claude session", error))?;
    let identity =
        input_record_cache::input_file_identity_from_open_file(&file).map_err(|error| {
            SessionParseError::at_path(path, "inspect Claude input identity", error)
        })?;
    let mut reader = BufReader::new(file);
    let scan = decode::parse_claude_reader(
        path,
        &mut reader,
        resolver,
        ClaudeParseState::new(path)?,
        Vec::new(),
        0,
        hash.then(Sha256::new),
        Some(cancellation),
    )?;
    Ok((scan, identity))
}

fn read_append(
    path: &Path,
    checkpoint: ClaudeCheckpoint,
    records: Vec<UsageRecord>,
    resolver: &ClaudeProjectResolver,
    cancellation: &crate::AcquisitionCancellation,
) -> SessionParseResult<Option<(RawClaudeScan, InputFileIdentity)>> {
    let mut file = File::open(path)
        .map_err(|error| SessionParseError::at_path(path, "open Claude append input", error))?;
    let identity =
        input_record_cache::input_file_identity_from_open_file(&file).map_err(|error| {
            SessionParseError::at_path(path, "inspect Claude append identity", error)
        })?;
    let mut hasher = Sha256::new();
    let mut remaining = checkpoint.consumed_offset;
    let mut buffer = [0; 64 * 1024];
    while remaining > 0 {
        if cancellation.is_cancelled() {
            return Err(SessionParseError::invalid(
                "verify Claude append prefix",
                "acquisition cancelled",
            ));
        }
        let length = remaining.min(buffer.len() as u64) as usize;
        let read = file.read(&mut buffer[..length]).map_err(|error| {
            SessionParseError::at_path(path, "verify Claude append prefix", error)
        })?;
        if read == 0 {
            return Ok(None);
        }
        hasher.update(&buffer[..read]);
        remaining -= read as u64;
    }
    if <[u8; 32]>::from(hasher.clone().finalize()) != checkpoint.prefix_hash {
        return Ok(None);
    }
    let mut reader = BufReader::new(file);
    let scan = decode::parse_claude_reader(
        path,
        &mut reader,
        resolver,
        checkpoint.state,
        records,
        checkpoint.consumed_offset,
        Some(hasher),
        Some(cancellation),
    )?;
    Ok(Some((scan, identity)))
}

pub(super) fn resolve_unit(
    parsed: ParsedUnit,
    ctx: &mut FoldContext<'_>,
) -> Result<cache::ResolvedUnit, InputPipelineError> {
    let ParsedUnit {
        unit,
        messages,
        cache_write,
        invalidate_cache,
        health,
    } = parsed;
    let UnitRecordPayload::Claude(input) = messages else {
        return cache::resolve_unit(
            ParsedUnit {
                unit,
                messages,
                cache_write,
                invalidate_cache,
                health,
            },
            ctx,
        );
    };
    let resolver = unit
        .claude_project_resolver()
        .cloned()
        .expect("Claude inputs carry their project resolver");
    match *input {
        ClaudeInput::Fresh {
            scan,
            snapshot,
            identity,
            cache_enabled,
        } => finish_scan(
            unit,
            scan,
            snapshot,
            identity,
            cache_enabled,
            &resolver,
            ctx,
        ),
        ClaudeInput::Cached {
            read_plan,
            checkpoint,
            append,
        } => {
            let records = match ctx.input_cache.take_records(&read_plan) {
                Ok(records) => records,
                Err(failure) => {
                    if !failure.can_reparse_input() {
                        return Err(failure.into());
                    }
                    ctx.record_cache_diagnostic(
                        unit.path.clone(),
                        InputDiagnosticKind::CacheReadFailed,
                        "read Claude checkpoint records",
                        failure.to_string(),
                    );
                    ctx.input_cache
                        .invalidate_read(&unit.path, unit.decoder.version());
                    return reparse(unit, ctx);
                }
            };
            if read_plan.primary_size() != Some(checkpoint.consumed_offset)
                || !checkpoint.state.matches_records(&records)
            {
                ctx.record_cache_diagnostic(
                    unit.path.clone(),
                    InputDiagnosticKind::CacheReadFailed,
                    "validate Claude checkpoint",
                    "input size, record count, or snapshot shape disagrees with parser state",
                );
                ctx.input_cache
                    .invalidate_read(&unit.path, unit.decoder.version());
                return reparse(unit, ctx);
            }
            if let Some(snapshot) = append {
                match read_append(
                    &unit.path,
                    checkpoint,
                    records,
                    &resolver,
                    ctx.cancellation(),
                ) {
                    Ok(Some((scan, identity))) => {
                        finish_scan(unit, scan, Some(snapshot), identity, true, &resolver, ctx)
                    }
                    Ok(None) => reparse(unit, ctx),
                    Err(error) => cache::resolve_unit(
                        ParsedUnit::unavailable(unit, InputFailure::from(&error)),
                        ctx,
                    ),
                }
            } else {
                let scan = RawClaudeScan {
                    records,
                    state: checkpoint.state,
                    interrupted: None,
                    consumed_offset: checkpoint.consumed_offset,
                    ends_with_newline: true,
                    content_hash: Some(checkpoint.prefix_hash),
                };
                let scanned = scan.finish_with_project(&checkpoint.project);
                Ok(cache::ResolvedUnit {
                    unit,
                    messages: scanned.messages,
                    cache_write: None,
                    invalidate_cache: false,
                    status: InputStatus::Complete,
                    rejections: scanned.rejections,
                })
            }
        }
    }
}

fn reparse(
    unit: DiscoveredInput,
    ctx: &mut FoldContext<'_>,
) -> Result<cache::ResolvedUnit, InputPipelineError> {
    let reparsed = ctx.reparse_one(unit)?;
    resolve_unit(reparsed, ctx)
}

fn finish_scan(
    unit: DiscoveredInput,
    mut scan: RawClaudeScan,
    snapshot: Option<InputSnapshot>,
    identity: InputFileIdentity,
    cache_enabled: bool,
    resolver: &ClaudeProjectResolver,
    ctx: &mut FoldContext<'_>,
) -> Result<cache::ResolvedUnit, InputPipelineError> {
    let mut fingerprint = None;
    if let Some(snapshot) = snapshot {
        let policy = unit.input_policy();
        match policy.snapshot() {
            Ok(current)
                if current == snapshot
                    && snapshot.primary_identity() == Some(identity)
                    && (scan.interrupted.is_some()
                        || snapshot.primary_size() == Some(scan.consumed_offset)) =>
            {
                match policy.fingerprint_from_snapshot(&snapshot) {
                    Ok(value) => fingerprint = Some(value),
                    Err(error) => {
                        scan.interrupted = Some(InputFailure::new(
                            "fingerprint Claude input",
                            error.to_string(),
                        ))
                    }
                }
            }
            Ok(_) => {
                scan.interrupted = Some(InputFailure::new(
                    "validate Claude input after parsing",
                    "input changed during parsing",
                ))
            }
            Err(error) => {
                scan.interrupted = Some(InputFailure::new(
                    "validate Claude input after parsing",
                    error.to_string(),
                ))
            }
        }
    }
    let project = scan.project_context(&unit.path, resolver);
    let dependent_input_captured = match project.dependency {
        ClaudeProjectDependency::None => true,
        ClaudeProjectDependency::ParentSession => matches!(
            &unit.fingerprint_policy,
            FingerprintPolicy::ClaudeCodeWithHome {
                parent_session_path: Some(_),
                ..
            }
        ),
        ClaudeProjectDependency::ExternalMetadata => false,
    };
    let cacheable = cache_enabled && dependent_input_captured && scan.interrupted.is_none();
    let mut plan = fingerprint.filter(|_| cacheable).map(|fingerprint| {
        CacheWritePlan::new(&unit.path, unit.decoder.version(), fingerprint, None)
    });
    if scan.ends_with_newline {
        if let Some(write) = plan.take() {
            let prefix_hash = scan.content_hash.ok_or_else(|| {
                InputPipelineError::contract(
                    "cacheable Claude parse did not produce a content hash",
                )
            })?;
            let checkpoint = ClaudeCheckpoint {
                state: scan.state.clone(),
                consumed_offset: scan.consumed_offset,
                prefix_hash,
                project: project.clone(),
            };
            cache::write_cache(
                Some(Box::new(
                    write
                        .with_claude_checkpoint(checkpoint)
                        .with_rejections(scan.health().rejections),
                )),
                ctx,
                &scan.records,
            );
        }
    }
    let scanned = scan.finish_with_project(&project);
    let status = match scanned.interrupted {
        None => InputStatus::Complete,
        Some(failure) => InputStatus::Partial { failure },
    };
    Ok(cache::ResolvedUnit {
        unit,
        messages: scanned.messages,
        cache_write: plan.map(Box::new),
        invalidate_cache: !cacheable,
        status,
        rejections: scanned.rejections,
    })
}

#[cfg(test)]
static DECODED_LINES: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, usize>>,
> = std::sync::OnceLock::new();

#[cfg(test)]
pub(super) fn record_decoded_line(path: &Path) {
    *DECODED_LINES
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .entry(path.to_owned())
        .or_default() += 1;
}

#[cfg(test)]
#[path = "tests/incremental.rs"]
mod tests;

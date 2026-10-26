//! Reconcile timestamped calls against authoritative database token totals.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::input_health::{InputFailure, RecordRejectionReason, ScannedInput};
use crate::records::UsageRecord;
use crate::{CalendarContext, TokenBreakdown};
use chrono::TimeZone;

pub(super) type GroupKey = (Arc<str>, Arc<str>, Option<Arc<str>>);

pub(super) fn group_key(record: &UsageRecord) -> GroupKey {
    (
        record.session_id.clone(),
        record.raw_model_id.clone(),
        record
            .billing_provider
            .as_deref()
            .map(crate::records::intern::intern),
    )
}

pub(super) fn group_hash(record: &UsageRecord) -> u64 {
    crate::records::dedup_hash_str(
        &serde_json::to_string(&group_key(record)).expect("string tuple serializes"),
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LogInputs {
    pub paths: Vec<PathBuf>,
    pub failure: Option<String>,
}

pub(super) fn discover(db: &Path) -> LogInputs {
    let root = db.parent().expect("database has a parent").join("logs");
    let mut inputs = LogInputs {
        paths: vec![root.join("agent.log")],
        failure: None,
    };
    let entries = match std::fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return inputs,
        Err(error) => {
            inputs.failure = Some(error.to_string());
            return inputs;
        }
    };
    for entry in entries {
        match entry {
            Ok(entry)
                if entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with("agent.log")) =>
            {
                inputs.paths.push(entry.path())
            }
            Ok(_) => {}
            Err(error) => {
                inputs.failure = Some(error.to_string());
            }
        }
    }
    inputs.paths.sort();
    inputs.paths.dedup();
    inputs
}

#[derive(Clone)]
struct Call {
    key: GroupKey,
    number: i64,
    timestamp: i64,
    tokens: TokenBreakdown,
}

const MARKER: &str = " agent.conversation_loop: API call #";

fn parse_call(line: &str, calendar: CalendarContext) -> Result<Option<Call>, ()> {
    let Some((prefix, rest)) = line.split_once(MARKER) else {
        return Ok(None);
    };
    if rest
        .split_whitespace()
        .any(|word| word == "usage=unavailable")
    {
        return Ok(None);
    }
    let (number, fields) = rest.split_once(':').ok_or(())?;
    let number = number.trim().parse::<i64>().map_err(|_| ())?;
    let (stamp, session) = prefix.split_once(" INFO [").ok_or(())?;
    let session = session
        .strip_suffix(']')
        .filter(|value| !value.is_empty())
        .ok_or(())?;
    let stamp =
        chrono::NaiveDateTime::parse_from_str(&stamp.replace(',', "."), "%Y-%m-%d %H:%M:%S%.f")
            .map_err(|_| ())?;
    let timestamp = calendar
        .timezone()
        .from_local_datetime(&stamp)
        .single()
        .ok_or(())?
        .timestamp_millis();
    let mut model = None;
    let mut provider = None;
    let mut input = None;
    let mut output = None;
    let mut cache_read = 0;
    let mut cache_write = 0;
    for field in fields.split_whitespace() {
        let Some((key, value)) = field.split_once('=') else {
            continue;
        };
        match key {
            "model" => model = Some(value),
            "provider" => provider = crate::provider_identity::billing_provider_id(value),
            "in" => input = Some(value.parse::<i64>().map_err(|_| ())?),
            "out" => output = Some(value.parse::<i64>().map_err(|_| ())?),
            "cache" => {
                cache_read = value
                    .split('/')
                    .next()
                    .ok_or(())?
                    .parse::<i64>()
                    .map_err(|_| ())?
            }
            "write" => cache_write = value.parse::<i64>().map_err(|_| ())?,
            _ => {}
        }
    }
    let input = input
        .ok_or(())?
        .checked_sub(cache_read)
        .and_then(|n| n.checked_sub(cache_write))
        .ok_or(())?;
    let tokens = TokenBreakdown {
        input,
        output: output.ok_or(())?,
        cache_read,
        cache_write,
        ..Default::default()
    };
    if number < 0 || timestamp <= 0 || crate::finalization::positive_token_total(&tokens).is_none()
    {
        return Err(());
    }
    let model = model.filter(|model| !model.is_empty()).ok_or(())?;
    Ok(Some(Call {
        key: (
            crate::records::intern::intern(session),
            crate::records::intern::intern(model),
            provider.as_deref().map(crate::records::intern::intern),
        ),
        number,
        timestamp,
        tokens,
    }))
}

pub(super) fn allocate(scanned: &mut ScannedInput, inputs: &LogInputs, calendar: CalendarContext) {
    if let Some(failure) = &inputs.failure {
        scanned.interrupted = Some(InputFailure::new("discover Hermes call logs", failure));
        return;
    }
    let keys: BTreeSet<_> = scanned.messages.iter().map(group_key).collect();
    let mut calls: BTreeMap<GroupKey, Vec<Call>> = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for path in &inputs.paths {
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                scanned.interrupted =
                    Some(InputFailure::new("read Hermes call log", error.to_string()));
                continue;
            }
        };
        let length = match file.metadata() {
            Ok(metadata) if metadata.is_file() => metadata.len(),
            Ok(_) => {
                scanned.interrupted = Some(InputFailure::new(
                    "inspect Hermes call log",
                    "call log is not a regular file",
                ));
                continue;
            }
            Err(error) => {
                scanned.interrupted = Some(InputFailure::new(
                    "inspect Hermes call log",
                    error.to_string(),
                ));
                continue;
            }
        };
        for line in BufReader::new(file.take(length)).lines() {
            let line = match line {
                Ok(line) => line,
                Err(error) => {
                    scanned.interrupted =
                        Some(InputFailure::new("read Hermes call log", error.to_string()));
                    break;
                }
            };
            match parse_call(&line, calendar) {
                Ok(Some(call)) if keys.contains(&call.key) => {
                    let fingerprint = (
                        call.key.clone(),
                        call.number,
                        call.timestamp,
                        call.tokens.input,
                        call.tokens.output,
                        call.tokens.cache_read,
                        call.tokens.cache_write,
                    );
                    if seen.insert(fingerprint) {
                        calls.entry(call.key.clone()).or_default().push(call);
                    }
                }
                Ok(_) => {}
                Err(()) => scanned
                    .rejections
                    .record(RecordRejectionReason::MalformedRecord),
            }
        }
    }
    let records = std::mem::take(&mut scanned.messages);
    for record in records {
        let Some(mut calls) = calls.remove(&group_key(&record)) else {
            scanned.messages.push(record);
            continue;
        };
        calls.sort_by_key(|call| (call.timestamp, call.number));
        match split(&record, &calls) {
            Some(parts) => scanned.messages.extend(parts),
            None => {
                scanned
                    .rejections
                    .record(RecordRejectionReason::MalformedRecord);
                scanned.messages.push(record);
            }
        }
    }
}

fn split(record: &UsageRecord, calls: &[Call]) -> Option<Vec<UsageRecord>> {
    let summed = calls
        .iter()
        .try_fold(TokenBreakdown::default(), |total, call| {
            total.checked_add(&call.tokens)
        })?;
    let output = record.tokens.output.checked_add(record.tokens.reasoning)?;
    let residual = TokenBreakdown {
        input: record.tokens.input.checked_sub(summed.input)?,
        output: output.checked_sub(summed.output)?,
        cache_read: record.tokens.cache_read.checked_sub(summed.cache_read)?,
        cache_write: record.tokens.cache_write.checked_sub(summed.cache_write)?,
        ..Default::default()
    };
    crate::finalization::positive_token_total(&residual)?;
    let mut parts = Vec::new();
    for call in calls {
        if call.tokens.checked_total()? == 0 {
            continue;
        }
        let mut part = record.clone();
        part.tokens = call.tokens.clone();
        part.timestamp = call.timestamp;
        part.is_aggregate = false;
        part.message_count = 0;
        parts.push(part);
    }
    if residual.checked_total()? > 0 {
        let mut part = record.clone();
        part.tokens = residual;
        part.message_count = 0;
        parts.push(part);
    }
    if output > 0 {
        let mut assigned = 0;
        let mut remainders = Vec::with_capacity(parts.len());
        for (index, part) in parts.iter_mut().enumerate() {
            let product = i128::from(record.tokens.reasoning) * i128::from(part.tokens.output);
            let reasoning = (product / i128::from(output)) as i64;
            remainders.push((product % i128::from(output), index));
            part.tokens.reasoning = reasoning;
            part.tokens.output -= reasoning;
            assigned += reasoning;
        }
        remainders.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        for (_, index) in remainders
            .into_iter()
            .take((record.tokens.reasoning - assigned) as usize)
        {
            parts[index].tokens.reasoning += 1;
            parts[index].tokens.output -= 1;
        }
    }
    for (index, part) in parts.iter_mut().enumerate() {
        part.dedup_key = Some(crate::records::dedup_hash_str(&format!(
            "hermes:{}:{index}",
            group_hash(record)
        )));
    }
    if let Some(first) = parts.first_mut() {
        first.message_count = record.message_count;
    }
    Some(parts)
}

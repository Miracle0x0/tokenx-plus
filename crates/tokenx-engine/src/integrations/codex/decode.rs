//! Codex session decoder.
//!
//! Parses plain and Zstandard-compressed JSONL from `~/.codex/sessions/` and
//! `~/.codex/archived_sessions/`. Scan-root discovery lives in the Codex
//! driver; content-derived dedup keys prevent a session present in both roots
//! from being counted twice.
//! Note: This parser has stateful logic to track model and delta calculations.

use crate::input_health::{InputFailure, RecordRejectionReason, RejectionSummary};
use crate::records::error::{SessionParseError, SessionParseResult};
use crate::records::{normalize_workspace_key, workspace_label_from_key, UsageRecord};
use crate::TokenBreakdown;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;

/// Codex entry structure (from JSONL files)
#[derive(Debug, Deserialize)]
pub struct CodexEntry {
    #[serde(rename = "type")]
    pub entry_type: String,
    pub timestamp: Option<String>,
    pub payload: Option<CodexPayload>,
}

#[derive(Debug, Deserialize)]
pub struct CodexPayload {
    pub id: Option<String>,
    pub forked_from_id: Option<String>,
    #[serde(rename = "type")]
    pub payload_type: Option<String>,
    pub model: Option<String>,
    pub model_name: Option<String>,
    pub model_info: Option<CodexModelInfo>,
    pub info: Option<CodexInfo>,
    pub usage: Option<CodexTokenUsage>,
    pub response_id: Option<String>,
    pub thread_id: Option<String>,
    pub service_tier: Option<String>,
    pub turn_id: Option<String>,
    pub thread_settings: Option<CodexThreadSettings>,
    pub source: Option<Value>,
    /// Current working directory from session_meta.
    pub cwd: Option<String>,
    /// Provider identity from session_meta (e.g. "openai", "azure")
    pub model_provider: Option<String>,
    /// Agent name from session_meta
    pub agent_nickname: Option<String>,
    /// Stable agent role from session metadata or subagent thread_spawn.
    pub agent_role: Option<String>,
    /// Legacy user-input body; known injected context prefixes are excluded.
    pub message: Option<String>,
    /// Structured item carried by modern `item_completed` events.
    pub item: Option<CodexTurnItem>,
}

#[derive(Debug, Deserialize)]
pub struct CodexThreadSettings {
    pub service_tier: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum CodexTurnItem {
    UserMessage {
        content: Vec<CodexUserInput>,
    },
    // Assistant messages, tools, and reasoning do not start human turns.
    #[serde(other)]
    Other,
}

impl CodexTurnItem {
    fn is_human_turn(&self) -> bool {
        match self {
            Self::UserMessage { content } => content.iter().any(|input| match input {
                CodexUserInput::Text { text } => codex_message_is_human_turn(Some(text)),
                CodexUserInput::Image
                | CodexUserInput::LocalImage
                | CodexUserInput::Skill
                | CodexUserInput::Mention => true,
            }),
            Self::Other => false,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CodexUserInput {
    Text { text: String },
    Image,
    LocalImage,
    Skill,
    Mention,
}

#[derive(Debug, Deserialize)]
pub struct CodexModelInfo {
    pub slug: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CodexInfo {
    pub model: Option<String>,
    pub model_name: Option<String>,
    pub last_token_usage: Option<CodexTokenUsage>,
    pub total_token_usage: Option<CodexTokenUsage>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CodexTokenUsage {
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cached_input_tokens: Option<i64>,
    pub cache_read_input_tokens: Option<i64>,
    pub cache_write_input_tokens: Option<i64>,
    pub reasoning_output_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct CodexTotals {
    input: i64,
    output: i64,
    cached: i64,
    cache_write: i64,
    reasoning: i64,
}

impl CodexTotals {
    fn from_usage(usage: &CodexTokenUsage) -> Self {
        Self {
            input: usage.input_tokens.unwrap_or(0),
            output: usage.output_tokens.unwrap_or(0),
            cached: usage
                .cached_input_tokens
                .unwrap_or(0)
                .max(usage.cache_read_input_tokens.unwrap_or(0)),
            cache_write: usage.cache_write_input_tokens.unwrap_or(0),
            reasoning: usage.reasoning_output_tokens.unwrap_or(0),
        }
    }

    fn delta_from(self, previous: Self) -> Option<Self> {
        if self.input < previous.input
            || self.output < previous.output
            || self.cached < previous.cached
            || self.cache_write < previous.cache_write
            || self.reasoning < previous.reasoning
        {
            return None;
        }

        Some(Self {
            input: self.input - previous.input,
            output: self.output - previous.output,
            cached: self.cached - previous.cached,
            cache_write: self.cache_write - previous.cache_write,
            reasoning: self.reasoning - previous.reasoning,
        })
    }

    fn checked_total(self) -> Option<i64> {
        [
            self.input,
            self.output,
            self.cached,
            self.cache_write,
            self.reasoning,
        ]
        .into_iter()
        .try_fold(0_i64, i64::checked_add)
    }

    fn is_within(self, baseline: Self) -> bool {
        self.input <= baseline.input
            && self.output <= baseline.output
            && self.cached <= baseline.cached
            && self.cache_write <= baseline.cache_write
            && self.reasoning <= baseline.reasoning
    }

    fn looks_like_stale_regression(self, previous: Self, last: Self) -> bool {
        let (Some(previous_total), Some(current_total), Some(last_total)) = (
            previous.checked_total(),
            self.checked_total(),
            last.checked_total(),
        ) else {
            return false;
        };

        if previous_total <= 0 || current_total <= 0 || last_total <= 0 {
            return false;
        }

        // Some Codex token_count snapshots arrive slightly out of order: the cumulative
        // total regresses by roughly one recent increment, then resumes from the true
        // higher watermark on the next row. Treat those as stale snapshots rather than
        // hard resets so we do not count `last_token_usage` twice.
        i128::from(current_total) * 100 >= i128::from(previous_total) * 98
            || i128::from(current_total) + i128::from(last_total) * 2 >= i128::from(previous_total)
    }

    fn into_tokens(self) -> TokenBreakdown {
        TokenBreakdown {
            input: self.input - self.cached - self.cache_write,
            output: self.output,
            cache_read: self.cached,
            cache_write: self.cache_write,
            cache_write_1h: 0,
            reasoning: self.reasoning,
        }
    }
}

fn validate_codex_token_usage(usage: &CodexTokenUsage) -> SessionParseResult<CodexTotals> {
    if [
        usage.input_tokens,
        usage.output_tokens,
        usage.cached_input_tokens,
        usage.cache_read_input_tokens,
        usage.cache_write_input_tokens,
        usage.reasoning_output_tokens,
        usage.total_tokens,
    ]
    .into_iter()
    .flatten()
    .any(|value| value < 0)
    {
        return Err(SessionParseError::invalid(
            "validate Codex token-count usage",
            "token bucket is negative",
        ));
    }

    let totals = CodexTotals::from_usage(usage);
    // Subtract only after checking reads, so even overflowing read + write
    // detail is rejected without overflowing the validation itself.
    if totals.cached > totals.input || totals.cache_write > totals.input - totals.cached {
        return Err(SessionParseError::invalid(
            "validate Codex token-count usage",
            "cache read and cache write tokens exceed input tokens",
        ));
    }
    if totals.checked_total().is_none() {
        return Err(SessionParseError::invalid(
            "validate Codex token-count usage",
            "token total exceeds i64::MAX",
        ));
    }
    Ok(totals)
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct CodexParseState {
    pub current_model: Option<String>,
    pub service_tier: Option<String>,
    pub previous_totals: Option<CodexTotals>,
    pub session_is_exec: bool,
    pub session_id_from_meta: Option<String>,
    /// Only an explicit `source.subagent.thread_spawn` marks delegated usage.
    #[serde(default)]
    pub session_is_child: bool,
    pub session_forked_from_id: Option<String>,
    pub forked_child_session_id: Option<String>,
    pub forked_child_replay_session_id: Option<String>,
    pub session_provider: Option<String>,
    pub session_agent: Option<String>,
    pub session_agent_instance: Option<String>,
    pub session_workspace_key: Option<String>,
    pub session_workspace_label: Option<String>,
    pub forked_child_waiting_for_turn_context: bool,
    pub forked_child_inherited_baseline: Option<CodexTotals>,
    pub forked_child_inherited_reported_total: Option<i64>,
    /// Human input from a legacy event or completed user item marks the next
    /// accepted token_count as a turn start, including across appended chunks.
    #[serde(default)]
    pub pending_turn_start: bool,
    pub current_turn_id: Option<String>,
    pending_response: Option<PendingResponse>,
    settled_responses: Vec<ResponseSignature>,
    seen_responses: std::sync::Arc<std::collections::BTreeSet<u64>>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct ResponseSignature {
    usage: CodexTotals,
    turn_id: Option<String>,
    model: String,
}

impl ResponseSignature {
    fn same_context(&self, turn: Option<&str>, model: Option<&str>) -> bool {
        turn.is_none_or(|turn| self.turn_id.as_deref().is_none_or(|id| id == turn))
            && model.is_none_or(|model| model == self.model)
    }

    fn matches(&self, usage: CodexTotals, turn: Option<&str>, model: Option<&str>) -> bool {
        self.usage.input == usage.input
            && self.usage.output == usage.output
            && self.same_context(turn, model)
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PendingResponse {
    record: UsageRecord,
    signature: ResponseSignature,
    after_compacted: bool,
}

fn settle_response(state: &mut CodexParseState, messages: &mut Vec<UsageRecord>) {
    let Some(mut pending) = state.pending_response.take() else {
        return;
    };
    pending.record.is_turn_start &= state.pending_turn_start;
    if pending.record.is_turn_start {
        state.pending_turn_start = false;
    }
    state.settled_responses.push(pending.signature);
    messages.push(pending.record);
}

fn response_record(
    payload: &CodexPayload,
    timestamp: Option<&str>,
    session_id: &str,
    state: &CodexParseState,
) -> Result<PendingResponse, RecordRejectionReason> {
    let usage = payload
        .usage
        .as_ref()
        .ok_or(RecordRejectionReason::MalformedRecord)?;
    if usage.input_tokens.is_none() && usage.output_tokens.is_none() {
        return Err(RecordRejectionReason::MalformedRecord);
    }
    let usage =
        validate_codex_token_usage(usage).map_err(|_| RecordRejectionReason::MalformedRecord)?;
    let model = extract_model(payload)
        .or_else(|| state.current_model.clone())
        .filter(|m| !m.trim().is_empty())
        .ok_or(RecordRejectionReason::MissingModel)?;
    let timestamp = parse_codex_entry_timestamp(timestamp)
        .map_err(|_| RecordRejectionReason::MissingTimestamp)?
        .ok_or(RecordRejectionReason::MissingTimestamp)?;
    let scope = state
        .session_forked_from_id
        .as_deref()
        .or(state.session_id_from_meta.as_deref())
        .unwrap_or(session_id);
    let key = match payload
        .response_id
        .as_deref()
        .filter(|id| !id.trim().is_empty())
    {
        Some(id) => format!("codex:usage-record:{scope}:{id}"),
        None => format!(
            "codex:usage-record:{scope}:{timestamp}:{}:{}:{}:{}:{}",
            usage.input, usage.output, usage.cached, usage.cache_write, usage.reasoning
        ),
    };
    let mut record = UsageRecord::new_with_dedup(
        &model,
        state.session_provider.as_deref().unwrap_or("openai"),
        session_id,
        timestamp,
        usage.into_tokens(),
        0.0,
        Some(crate::records::dedup_hash_str(&key)),
    );
    record.set_billing_provider(state.session_provider.as_deref());
    record.service_tier = payload
        .service_tier
        .as_deref()
        .or(state.service_tier.as_deref())
        .map(crate::records::intern::intern);
    record.agent = state
        .session_agent
        .as_deref()
        .map(crate::records::intern::intern);
    record.set_agent_instance(
        state
            .session_agent_instance
            .clone()
            .or_else(|| state.session_id_from_meta.clone()),
    );
    record.set_workspace(
        state.session_workspace_key.clone(),
        state.session_workspace_label.clone(),
    );
    record.is_main_session = !state.session_is_child;
    record.is_turn_start = state.pending_turn_start;
    Ok(PendingResponse {
        record,
        signature: ResponseSignature {
            usage,
            turn_id: payload
                .turn_id
                .clone()
                .or_else(|| state.current_turn_id.clone()),
            model,
        },
        after_compacted: false,
    })
}

#[derive(Debug, Clone)]
pub(crate) struct ParsedCodexFile {
    pub messages: Vec<UsageRecord>,
    pub rejections: RejectionSummary,
    pub interrupted: Option<InputFailure>,
    pub consumed_offset: u64,
    pub state: CodexParseState,
    pub content_hash: Option<[u8; 32]>,
    pub ends_with_newline: bool,
    pub input_identity: Option<crate::input_record_cache::InputFileIdentity>,
}

#[derive(Clone)]
struct PendingCodexMessage {
    billing_provider: crate::BillingProvider,
    service_tier: Option<String>,
    provider: String,
    session_id: String,
    timestamp: i64,
    tokens: TokenBreakdown,
    agent: Option<String>,
    agent_instance: Option<String>,
    is_turn_start: bool,
    dedup_scope_id: String,
    total_usage: CodexTotals,
    workspace_key: Option<String>,
    workspace_label: Option<String>,
    is_main_session: bool,
}

impl PendingCodexMessage {
    fn into_message(self, model: &str) -> UsageRecord {
        let mut message = UsageRecord::new_with_agent(
            model,
            self.provider,
            self.session_id,
            self.timestamp,
            self.tokens,
            0.0,
            self.agent,
        );
        message.billing_provider = self.billing_provider;
        message.set_agent_instance(self.agent_instance);
        message.service_tier = self
            .service_tier
            .as_deref()
            .map(crate::records::intern::intern);
        message.is_turn_start = self.is_turn_start;
        message.is_main_session = self.is_main_session;
        set_codex_dedup_key(&mut message, model, &self.dedup_scope_id, self.total_usage);
        message.set_workspace(self.workspace_key, self.workspace_label);
        message
    }
}

struct HashingReader<R> {
    inner: R,
    hasher: Sha256,
    last_byte: Option<u8>,
    #[cfg(test)]
    path: PathBuf,
}

impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buffer)?;
        if read > 0 {
            self.hasher.update(&buffer[..read]);
            self.last_byte = Some(buffer[read - 1]);
            #[cfg(test)]
            crate::input_record_cache::record_input_bytes(&self.path, read);
        }
        Ok(read)
    }
}

fn session_id_from_path(path: &Path) -> SessionParseResult<String> {
    let mut stem = path.file_stem();
    if super::is_codex_archive(path) {
        stem = stem.and_then(|stem| Path::new(stem).file_stem());
    }
    let session_id = stem
        .and_then(|s| s.to_str())
        .map(str::trim)
        .filter(|session_id| !session_id.is_empty())
        .ok_or_else(|| {
            SessionParseError::invalid(
                "derive Codex session id",
                "input filename must have a non-blank UTF-8 stem",
            )
        })?;
    Ok(session_id.to_string())
}

fn codex_workspace_from_cwd(cwd: &str) -> (Option<String>, Option<String>) {
    let workspace_key = normalize_codex_workspace_key(cwd);
    let workspace_label = workspace_key.as_deref().and_then(workspace_label_from_key);

    if workspace_label.is_none() {
        return (None, None);
    }

    (workspace_key, workspace_label)
}

fn normalize_codex_workspace_key(raw: &str) -> Option<String> {
    let normalized = normalize_workspace_key(raw)?;
    if normalized.chars().any(char::is_control) {
        return None;
    }

    if looks_like_explicit_workspace_path(&normalized) {
        Some(normalized)
    } else {
        None
    }
}

fn looks_like_explicit_workspace_path(path: &str) -> bool {
    if path.starts_with("//") || path.starts_with('/') {
        return true;
    }

    let bytes = path.as_bytes();
    bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'/'
}

fn parse_codex_reader<R: BufRead + ?Sized>(
    reader: &mut R,
    session_id: &str,
    start_offset: u64,
    mut state: CodexParseState,
    cancellation: Option<&crate::engine::AcquisitionCancellation>,
) -> SessionParseResult<ParsedCodexFile> {
    let mut messages = Vec::with_capacity(64);
    let mut buffer = Vec::with_capacity(4096);
    let mut line = String::with_capacity(4096);
    let mut consumed_offset = start_offset;
    let mut pending_model_messages = Vec::new();
    let mut rejections = RejectionSummary::default();
    let mut interrupted = None;

    'records: loop {
        if cancellation.is_some_and(crate::engine::AcquisitionCancellation::is_cancelled) {
            interrupted = Some(InputFailure::new(
                "parse Codex JSONL input",
                "acquisition cancelled",
            ));
            break;
        }
        line.clear();
        let bytes_read = match reader.read_line(&mut line) {
            Ok(bytes_read) => bytes_read,
            Err(source) => {
                let error = SessionParseError::new("read Codex JSONL line", source);
                interrupted = Some(InputFailure::from(&error));
                break;
            }
        };
        if bytes_read == 0 {
            break;
        }
        let Some(updated_offset) = consumed_offset.checked_add(bytes_read as u64) else {
            let error = SessionParseError::invalid(
                "track Codex JSONL input offset",
                "consumed byte offset exceeds u64::MAX",
            );
            interrupted = Some(InputFailure::from(&error));
            break;
        };
        consumed_offset = updated_offset;

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // A line is the Codex record boundary. Parsing is transactional so a
        // rejected record cannot leak a model, token baseline, pending turn,
        // or provisional message into later records.
        let state_before = state.clone();
        let messages_len_before = messages.len();
        let pending_before = pending_model_messages.clone();

        macro_rules! reject_record {
            ($reason:expr) => {{
                let reason = $reason;
                state = state_before.clone();
                messages.truncate(messages_len_before);
                pending_model_messages = pending_before.clone();
                rejections.record(reason);
                continue 'records;
            }};
        }

        macro_rules! interrupt_on_record {
            ($reason:expr, $error:expr) => {{
                let reason = $reason;
                let error = $error;
                state = state_before.clone();
                messages.truncate(messages_len_before);
                pending_model_messages = pending_before.clone();
                rejections.record(reason);
                interrupted = Some(InputFailure::from(&error));
                break 'records;
            }};
        }

        let mut handled = false;
        buffer.clear();
        buffer.extend_from_slice(trimmed.as_bytes());
        let (entry, entry_decode_error) = match simd_json::from_slice::<CodexEntry>(&mut buffer) {
            Ok(entry) => (Some(entry), None),
            Err(error) => (None, Some(error)),
        };
        if let Some(entry) = entry {
            if entry.entry_type == "compacted" && !state.forked_child_waiting_for_turn_context {
                if let Some(pending) = &mut state.pending_response {
                    pending.after_compacted = true;
                }
                handled = true;
            }
            if entry.payload.is_none() {
                let error = match entry.entry_type.as_str() {
                    "session_meta" => Some(SessionParseError::invalid(
                        "validate Codex session metadata",
                        "payload is missing",
                    )),
                    "turn_context" => Some(SessionParseError::invalid(
                        "validate Codex turn_context event",
                        "payload is missing",
                    )),
                    "event_msg" | "token_usage_record" => Some(SessionParseError::invalid(
                        "validate Codex event message",
                        "payload is missing",
                    )),
                    _ => None,
                };
                if let Some(error) = error {
                    interrupt_on_record!(RecordRejectionReason::MalformedRecord, error);
                }
            }
            if let Some(payload) = entry.payload {
                let payload_model = extract_model(&payload);
                let event_type = if entry.entry_type == "event_msg" {
                    match payload.payload_type.as_deref() {
                        Some(payload_type) => Some(payload_type),
                        None => interrupt_on_record!(
                            RecordRejectionReason::MalformedRecord,
                            SessionParseError::invalid(
                                "validate Codex event message",
                                "payload type is missing",
                            )
                        ),
                    }
                } else {
                    None
                };
                let token_info = match event_type {
                    Some("token_count") => payload.info.as_ref(),
                    _ => None,
                };
                let is_token_count = token_info.is_some();
                let info_model = token_info.and_then(extract_model_from_info);
                let event_model = payload_model.clone().or(info_model.clone());

                if state.forked_child_waiting_for_turn_context {
                    if entry.entry_type == "turn_context"
                        && forked_child_turn_starts_own_session(&state, payload.turn_id.as_deref())
                    {
                        state.forked_child_waiting_for_turn_context = false;
                        state.forked_child_replay_session_id = None;
                        if let Some(ref id) = state.forked_child_session_id {
                            state.session_id_from_meta = Some(id.clone());
                        }
                        state.current_model = payload_model.clone();
                        handled = true;
                    } else {
                        if entry.entry_type == "session_meta" {
                            if let Some(ref id) = payload.id {
                                if state
                                    .forked_child_session_id
                                    .as_deref()
                                    .is_some_and(|child_id| child_id != id)
                                {
                                    // Newer Codex fork logs can embed the
                                    // parent session metadata before replaying
                                    // parent token_count history. Keep
                                    // skipping while that copied upstream
                                    // transcript is active.
                                    state.forked_child_replay_session_id = Some(id.clone());
                                }
                            }
                        }
                        if let Some(info) = token_info {
                            if remember_forked_child_inherited_baseline(&mut state, info).is_err() {
                                reject_record!(RecordRejectionReason::MalformedRecord);
                            }
                        }
                        continue;
                    }
                }

                if entry.entry_type == "token_usage_record" {
                    let foreign = payload
                        .thread_id
                        .as_deref()
                        .zip(state.session_id_from_meta.as_deref())
                        .is_some_and(|(thread, own)| thread != own);
                    if foreign {
                        continue;
                    }
                    let pending = match response_record(
                        &payload,
                        entry.timestamp.as_deref(),
                        session_id,
                        &state,
                    ) {
                        Ok(record) => record,
                        Err(reason) => reject_record!(reason),
                    };
                    if pending.record.tokens.total() == 0 {
                        continue;
                    }
                    let key = pending
                        .record
                        .dedup_key
                        .expect("response records have a dedup identity");
                    if state.seen_responses.contains(&key) {
                        continue;
                    }
                    // Commit this validated record before mutating the persistent
                    // set, so the transaction snapshot does not force a full copy.
                    drop(state_before);
                    std::sync::Arc::make_mut(&mut state.seen_responses).insert(key);
                    settle_response(&mut state, &mut messages);
                    state.pending_response = Some(pending);
                    continue;
                }
                let begins_turn =
                    entry.entry_type == "turn_context" || event_type == Some("task_started");
                let starts_other_turn = begins_turn
                    && state.pending_response.as_ref().is_some_and(|pending| {
                        pending.signature.turn_id != payload.turn_id
                            || (event_type == Some("task_started") && payload.turn_id.is_none())
                    });
                let ends_pending_turn =
                    matches!(event_type, Some("turn_aborted" | "task_complete"))
                        && state.pending_response.as_ref().is_some_and(|pending| {
                            payload.turn_id.is_none()
                                || pending.signature.turn_id == payload.turn_id
                        });
                if starts_other_turn || ends_pending_turn {
                    settle_response(&mut state, &mut messages);
                }
                if begins_turn {
                    state.current_turn_id = payload.turn_id.clone();
                }

                if !pending_model_messages.is_empty()
                    && event_model.is_none()
                    && !is_token_count
                    && entry.entry_type != "session_meta"
                {
                    interrupt_on_record!(
                        RecordRejectionReason::MissingModel,
                        SessionParseError::invalid(
                            "resolve Codex token-count model",
                            "token-count rows were not followed by a model-bearing event",
                        )
                    );
                }

                if entry.entry_type == "session_meta" {
                    if codex_origin_is_exec(payload.source.as_ref()) {
                        state.session_is_exec = true;
                    }
                    if let Some(ref id) = payload.id {
                        state.session_id_from_meta = Some(id.clone());
                    }
                    state.session_is_child |= codex_origin_is_thread_spawn(payload.source.as_ref());
                    let forked_from_id = payload
                        .forked_from_id
                        .as_deref()
                        .filter(|id| !id.is_empty())
                        .or_else(|| forked_from_id_from_origin(payload.source.as_ref()));
                    if let Some(forked_from_id) = forked_from_id {
                        state.session_forked_from_id = Some(forked_from_id.to_string());
                        state.forked_child_session_id = payload.id.clone();
                        state.forked_child_waiting_for_turn_context = true;
                        state.forked_child_replay_session_id = None;
                        state.forked_child_inherited_baseline = None;
                        state.forked_child_inherited_reported_total = None;
                    }
                    if let Some(ref provider) = payload.model_provider {
                        state.session_provider = Some(provider.clone());
                    }
                    let agent_role = payload
                        .agent_role
                        .as_deref()
                        .or_else(|| codex_origin_agent_role(payload.source.as_ref()));
                    state.session_agent = codex_agent_label(
                        payload.source.as_ref(),
                        agent_role,
                        payload.agent_nickname.as_deref(),
                        state.session_is_exec,
                    );
                    state.session_agent_instance = payload
                        .agent_nickname
                        .as_ref()
                        .map(|nickname| nickname.trim().to_string())
                        .filter(|nickname| !nickname.is_empty());
                    if let Some(ref cwd) = payload.cwd {
                        let (workspace_key, workspace_label) = codex_workspace_from_cwd(cwd);
                        state.session_workspace_key = workspace_key;
                        state.session_workspace_label = workspace_label;
                    }
                }
                // Extract model from turn_context
                if entry.entry_type == "turn_context" {
                    state.current_model = payload_model.clone();
                    if let Some(model) = state.current_model.clone() {
                        flush_pending_model_messages(
                            &mut pending_model_messages,
                            &mut messages,
                            &model,
                        );
                    }
                    handled = true;
                }

                // Both rollout formats mark the next accepted usage after human
                // input. Co-emitted legacy and structured events share this flag;
                // steering input can mark later usage in the same Codex turn_id.
                // Inherited fork input is skipped by the replay branch above.
                match event_type {
                    Some("thread_settings_applied") => {
                        let Some(settings) = payload.thread_settings.as_ref() else {
                            interrupt_on_record!(
                                RecordRejectionReason::MalformedRecord,
                                SessionParseError::invalid(
                                    "validate Codex thread settings",
                                    "thread_settings is missing",
                                )
                            );
                        };
                        // A settings snapshot applies prospectively. Never infer
                        // earlier usage from a later setting or today's config.
                        state.service_tier = settings.service_tier.clone();
                        handled = true;
                    }
                    Some("user_message") => {
                        state.pending_turn_start |=
                            codex_message_is_human_turn(payload.message.as_deref());
                        handled = true;
                    }
                    Some("item_completed") => {
                        let Some(item) = payload.item.as_ref() else {
                            interrupt_on_record!(
                                RecordRejectionReason::MalformedRecord,
                                SessionParseError::invalid(
                                    "validate Codex completed item",
                                    "item is missing",
                                )
                            );
                        };
                        state.pending_turn_start |= item.is_human_turn();
                        handled = true;
                    }
                    _ => {}
                }

                // Process token_count events
                if let Some(info) = token_info {
                    let model = payload_model
                        .or(info_model)
                        .or_else(|| {
                            state
                                .pending_response
                                .as_ref()
                                .map(|p| p.signature.model.clone())
                        })
                        .or_else(|| state.current_model.clone());
                    if let Some(ref model) = model {
                        state.current_model = Some(model.clone());
                        flush_pending_model_messages(
                            &mut pending_model_messages,
                            &mut messages,
                            model,
                        );
                    }

                    // Use last_token_usage as the primary increment basis.
                    // Upstream totals are mutable snapshots (compaction, context-window
                    // capping can rewrite them), so we only use total_token_usage for
                    // dedup and monotonicity checks — never as a direct delta basis.
                    let (total_usage_record, last_usage_record) =
                        match required_codex_token_usage(info) {
                            Ok(usage) => usage,
                            Err(error) => {
                                interrupt_on_record!(RecordRejectionReason::MalformedRecord, error)
                            }
                        };
                    let total_usage = match validate_codex_token_usage(total_usage_record) {
                        Ok(usage) => usage,
                        Err(_) => reject_record!(RecordRejectionReason::MalformedRecord),
                    };
                    let last_usage = match validate_codex_token_usage(last_usage_record) {
                        Ok(usage) => usage,
                        Err(_) => reject_record!(RecordRejectionReason::MalformedRecord),
                    };

                    // Forked child logs can replay more than one parent
                    // token_count row after the first child turn_context,
                    // often with child-local timestamps. Keep the inherited
                    // baseline active until totals move beyond it.
                    if forked_child_should_skip_inherited_snapshot(
                        &state,
                        total_usage_record,
                        total_usage,
                    ) {
                        state.pending_response = None;
                        continue;
                    }
                    state.forked_child_inherited_baseline = None;
                    state.forked_child_inherited_reported_total = None;

                    if let Some(previous) = state.previous_totals {
                        if total_usage == previous {
                            if state
                                .pending_response
                                .as_ref()
                                .is_some_and(|p| p.after_compacted)
                            {
                                settle_response(&mut state, &mut messages);
                            }
                            continue;
                        }
                        if total_usage.delta_from(previous).is_none()
                            && total_usage.looks_like_stale_regression(previous, last_usage)
                        {
                            if state
                                .pending_response
                                .as_ref()
                                .is_some_and(|p| p.after_compacted)
                            {
                                settle_response(&mut state, &mut messages);
                            }
                            continue;
                        }
                    }
                    let tokens = last_usage.into_tokens();

                    // Skip zero-token snapshots without advancing the baseline so
                    // that post-compaction zero totals don't inflate later deltas.
                    if tokens.input == 0
                        && tokens.output == 0
                        && tokens.cache_read == 0
                        && tokens.cache_write == 0
                        && tokens.reasoning == 0
                    {
                        if state
                            .pending_response
                            .as_ref()
                            .is_some_and(|p| p.after_compacted)
                        {
                            settle_response(&mut state, &mut messages);
                        }
                        continue;
                    }

                    state.previous_totals = Some(total_usage);

                    let reports_pending = state.pending_response.as_ref().is_some_and(|p| {
                        p.signature.matches(
                            last_usage,
                            payload.turn_id.as_deref(),
                            model.as_deref(),
                        )
                    });
                    let mut request_tier = None;
                    if reports_pending {
                        request_tier = state
                            .pending_response
                            .take()
                            .and_then(|p| p.record.service_tier);
                    } else if let Some(index) = state.settled_responses.iter().position(|p| {
                        p.matches(
                            last_usage,
                            payload.turn_id.as_deref(),
                            event_model.as_deref(),
                        )
                    }) {
                        state.settled_responses.remove(index);
                        continue;
                    } else if state.pending_response.as_ref().is_some_and(|p| {
                        p.after_compacted
                            || !p
                                .signature
                                .same_context(payload.turn_id.as_deref(), event_model.as_deref())
                    }) {
                        settle_response(&mut state, &mut messages);
                    } else {
                        // Outside compaction the adjacent token_count is the
                        // authoritative report, even if its bucket split differs.
                        request_tier = state
                            .pending_response
                            .take()
                            .and_then(|p| p.record.service_tier);
                    }

                    let parsed_timestamp =
                        match parse_codex_entry_timestamp(entry.timestamp.as_deref()) {
                            Ok(timestamp) => timestamp,
                            Err(error) => {
                                interrupt_on_record!(RecordRejectionReason::MalformedRecord, error)
                            }
                        };
                    let timestamp = match parsed_timestamp {
                        Some(timestamp) => timestamp,
                        None => interrupt_on_record!(
                            RecordRejectionReason::MissingTimestamp,
                            SessionParseError::invalid(
                                "validate Codex token-count event",
                                "timestamp is missing",
                            )
                        ),
                    };

                    let provider = state.session_provider.as_deref().unwrap_or("openai");

                    // Fork/subagent children replay the same upstream
                    // token_count history into many sibling files. Those
                    // replays carry identical cumulative totals but a
                    // distinct per-file session id, so a session-scoped key
                    // never collapses them and the totals get counted once
                    // per sibling. Scope the key to the fork parent instead
                    // so sibling replays share one key. Unrelated records
                    // keep their own id and never merge.
                    let dedup_scope_id = state
                        .session_forked_from_id
                        .as_deref()
                        .or(state.session_id_from_meta.as_deref())
                        .unwrap_or(session_id)
                        .to_string();
                    let is_turn_start = std::mem::take(&mut state.pending_turn_start);
                    let pending = PendingCodexMessage {
                        billing_provider: crate::BillingProvider::from_observation(
                            state.session_provider.as_deref(),
                        ),
                        service_tier: request_tier
                            .map(|tier| tier.to_string())
                            .or_else(|| state.service_tier.clone()),
                        provider: provider.to_string(),
                        session_id: session_id.to_string(),
                        timestamp,
                        tokens,
                        agent: state.session_agent.clone(),
                        agent_instance: state
                            .session_agent_instance
                            .clone()
                            .or_else(|| state.session_id_from_meta.clone()),
                        is_turn_start,
                        dedup_scope_id,
                        total_usage,
                        workspace_key: state.session_workspace_key.clone(),
                        workspace_label: state.session_workspace_label.clone(),
                        is_main_session: !state.session_is_child,
                    };
                    if let Some(model) = model.as_deref() {
                        messages.push(pending.into_message(model));
                    } else {
                        pending_model_messages.push(pending);
                    }
                    handled = true;
                }
            }

            // Mark session_meta as handled (even if payload was processed above)
            if entry.entry_type == "session_meta" {
                handled = true;
            }
        }

        if handled {
            continue;
        }

        if !pending_model_messages.is_empty() {
            interrupt_on_record!(
                RecordRejectionReason::MissingModel,
                SessionParseError::invalid(
                    "resolve Codex token-count model",
                    "token-count rows were not followed by a model-bearing event",
                )
            );
        }

        if state.forked_child_waiting_for_turn_context {
            let mut json_probe = trimmed.as_bytes().to_vec();
            if simd_json::from_slice::<Value>(&mut json_probe).is_ok() {
                continue;
            }
        }

        if let Some(source) = entry_decode_error {
            let error = SessionParseError::new("decode Codex JSONL entry", source);
            let mut json_probe = trimmed.as_bytes().to_vec();
            let value = match simd_json::from_slice::<Value>(&mut json_probe) {
                Ok(value) => value,
                Err(_) => interrupt_on_record!(RecordRejectionReason::MalformedRecord, error),
            };
            if codex_schema_invalid_value_may_affect_state(&value) {
                interrupt_on_record!(RecordRejectionReason::MalformedRecord, error);
            }
            reject_record!(RecordRejectionReason::MalformedRecord);
        }

        let mut json_probe = trimmed.as_bytes().to_vec();
        if simd_json::from_slice::<Value>(&mut json_probe).is_err() {
            reject_record!(RecordRejectionReason::MalformedRecord);
        }
    }

    if interrupted.is_none() && !pending_model_messages.is_empty() {
        let error = SessionParseError::invalid(
            "resolve Codex token-count model",
            "input ended with token-count rows whose model was never identified",
        );
        rejections.record(RecordRejectionReason::MissingModel);
        interrupted = Some(InputFailure::from(&error));
    }

    Ok(ParsedCodexFile {
        messages,
        rejections,
        interrupted,
        consumed_offset,
        state,
        content_hash: None,
        ends_with_newline: false,
        input_identity: None,
    })
}

fn codex_origin_is_exec(origin: Option<&Value>) -> bool {
    origin.and_then(Value::as_str) == Some("exec")
}

fn codex_origin_is_subagent(origin: Option<&Value>) -> bool {
    origin
        .and_then(|value| value.get("subagent"))
        .and_then(Value::as_object)
        .is_some()
}

fn codex_origin_agent_role(origin: Option<&Value>) -> Option<&str> {
    origin?
        .get("subagent")?
        .get("thread_spawn")?
        .get("agent_role")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|role| !role.is_empty())
}

fn codex_agent_label(
    origin: Option<&Value>,
    agent_role: Option<&str>,
    agent_nickname: Option<&str>,
    is_exec: bool,
) -> Option<String> {
    if is_exec {
        return Some("Codex Exec".to_string());
    }

    if codex_origin_is_subagent(origin) {
        return Some(match agent_role {
            Some(role) => format!("Codex {}", crate::records::normalize_agent_name(role)),
            None => "Codex Subagent".to_string(),
        });
    }

    agent_nickname
        .map(str::trim)
        .filter(|nickname| !nickname.is_empty())
        .map(|_| "Codex Agent".to_string())
}

fn forked_from_id_from_origin(origin: Option<&Value>) -> Option<&str> {
    origin?
        .get("subagent")?
        .get("thread_spawn")?
        .get("parent_thread_id")?
        .as_str()
        .filter(|id| !id.is_empty())
}

fn codex_origin_is_thread_spawn(origin: Option<&Value>) -> bool {
    origin
        .and_then(|origin| origin.get("subagent"))
        .and_then(|subagent| subagent.get("thread_spawn"))
        .is_some()
}

fn forked_child_turn_starts_own_session(state: &CodexParseState, turn_id: Option<&str>) -> bool {
    if state.forked_child_replay_session_id.is_none() {
        return true;
    }

    let Some(child_session_id) = state.forked_child_session_id.as_deref() else {
        return true;
    };

    match (turn_id, codex_uuid_v7_order_key(child_session_id)) {
        (Some(turn_id), Some(child_key)) => {
            codex_uuid_v7_order_key(turn_id).is_none_or(|turn_key| {
                turn_key
                    .get(..12)
                    .zip(child_key.get(..12))
                    .is_none_or(|(turn_ms, child_ms)| turn_ms >= child_ms)
            })
        }
        _ => true,
    }
}

fn codex_uuid_v7_order_key(id: &str) -> Option<String> {
    let mut parts = id.split('-');
    let first = parts.next()?;
    let second = parts.next()?;
    let third = parts.next()?;
    let fourth = parts.next()?;
    let fifth = parts.next()?;

    if parts.next().is_some()
        || first.len() != 8
        || second.len() != 4
        || third.len() != 4
        || fourth.len() != 4
        || fifth.len() != 12
        || !third.starts_with('7')
    {
        return None;
    }

    let mut key = String::with_capacity(32);
    for part in [first, second, third, fourth, fifth] {
        if !part.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        key.push_str(&part.to_ascii_lowercase());
    }
    Some(key)
}

fn parse_codex_entry_timestamp(timestamp: Option<&str>) -> SessionParseResult<Option<i64>> {
    timestamp
        .map(|timestamp| {
            chrono::DateTime::parse_from_rfc3339(timestamp)
                .map(|date_time| date_time.timestamp_millis())
                .map_err(|source| SessionParseError::new("parse Codex event timestamp", source))
        })
        .transpose()
}

fn codex_token_count_dedup_key(
    message: &UsageRecord,
    model: &str,
    upstream_session_id: &str,
    total_usage: CodexTotals,
) -> u64 {
    // Codex fork/subagent logs can replay the same upstream token_count
    // history into many child files with child-local timestamps. Current-format
    // cumulative totals provide the stable upstream identity.
    crate::records::dedup_hash_str(&format!(
        "codex:token_count-total:{}:{}:{}:{}:{}:{}:{}:{}",
        upstream_session_id,
        message.provider_id,
        model,
        total_usage.input,
        total_usage.output,
        total_usage.cached,
        total_usage.cache_write,
        total_usage.reasoning
    ))
}

fn set_codex_dedup_key(
    message: &mut UsageRecord,
    model: &str,
    upstream_session_id: &str,
    total_usage: CodexTotals,
) {
    if message.dedup_key.is_none() {
        message.dedup_key = Some(codex_token_count_dedup_key(
            message,
            model,
            upstream_session_id,
            total_usage,
        ));
    }
}

fn flush_pending_model_messages(
    pending_model_messages: &mut Vec<PendingCodexMessage>,
    messages: &mut Vec<UsageRecord>,
    model: &str,
) {
    for pending in pending_model_messages.drain(..) {
        messages.push(pending.into_message(model));
    }
}

/// Archives use the same event state machine as plain logs, but their decoded
/// offsets are never persisted as physical append offsets.
pub(crate) fn parse_codex_archive(
    path: &Path,
    cancellation: Option<&crate::engine::AcquisitionCancellation>,
) -> SessionParseResult<crate::input_health::ScannedInput> {
    let file = std::fs::File::open(path)
        .map_err(|source| SessionParseError::at_path(path, "open Codex archive", source))?;
    #[cfg(test)]
    let file = ObservedArchiveReader { file, path };
    let decoder = zstd::stream::read::Decoder::new(file)
        .map_err(|source| SessionParseError::at_path(path, "decode Codex zstd stream", source))?;
    let mut reader = BufReader::new(decoder);
    let parsed = parse_codex_reader(
        &mut reader,
        &session_id_from_path(path)?,
        0,
        CodexParseState::default(),
        cancellation,
    )?;
    if parsed.consumed_offset == 0 {
        if let Some(failure) = parsed.interrupted {
            return Err(SessionParseError::invalid(
                "read Codex archive",
                failure.message,
            ));
        }
    }
    Ok(crate::input_health::ScannedInput {
        messages: parsed.messages,
        rejections: parsed.rejections,
        interrupted: parsed.interrupted,
    })
}

#[cfg(test)]
struct ObservedArchiveReader<'a> {
    file: std::fs::File,
    path: &'a Path,
}

#[cfg(test)]
impl Read for ObservedArchiveReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let read = self.file.read(buffer)?;
        crate::input_record_cache::record_input_bytes(self.path, read);
        Ok(read)
    }
}

/// Parse a Codex JSONL file with stateful tracking
#[cfg(test)]
pub fn parse_codex_file(path: &Path) -> SessionParseResult<Vec<UsageRecord>> {
    let file = std::fs::File::open(path)
        .map_err(|source| SessionParseError::new("open Codex JSONL input", source))?;

    let session_id = session_id_from_path(path)?;
    let mut reader = BufReader::new(file);
    let parsed = parse_codex_reader(
        &mut reader,
        &session_id,
        0,
        CodexParseState::default(),
        None,
    )?;
    if let Some(failure) = parsed.interrupted.as_ref() {
        return Err(SessionParseError::invalid(
            "parse Codex JSONL input",
            format!("{}: {}", failure.operation, failure.message),
        ));
    }
    Ok(parsed.messages)
}

fn reported_total_tokens(usage: &CodexTokenUsage) -> Option<i64> {
    usage.total_tokens.filter(|total| *total >= 0)
}

fn required_codex_token_usage(
    info: &CodexInfo,
) -> SessionParseResult<(&CodexTokenUsage, &CodexTokenUsage)> {
    let total = info.total_token_usage.as_ref().ok_or_else(|| {
        SessionParseError::invalid(
            "validate Codex token-count event",
            "total_token_usage is missing",
        )
    })?;
    let last = info.last_token_usage.as_ref().ok_or_else(|| {
        SessionParseError::invalid(
            "validate Codex token-count event",
            "last_token_usage is missing",
        )
    })?;
    Ok((total, last))
}

fn remember_forked_child_inherited_baseline(
    state: &mut CodexParseState,
    info: &CodexInfo,
) -> SessionParseResult<()> {
    let (total_usage, last_usage) = required_codex_token_usage(info)?;
    let totals = validate_codex_token_usage(total_usage)?;
    validate_codex_token_usage(last_usage)?;
    state.previous_totals = Some(totals);
    state.forked_child_inherited_baseline = Some(totals);
    state.forked_child_inherited_reported_total = reported_total_tokens(total_usage);
    Ok(())
}

fn forked_child_should_skip_inherited_snapshot(
    state: &CodexParseState,
    total_usage: &CodexTokenUsage,
    totals: CodexTotals,
) -> bool {
    if let Some(baseline) = state.forked_child_inherited_reported_total {
        if reported_total_tokens(total_usage).is_some_and(|total| total <= baseline) {
            return true;
        }
    }

    if let Some(baseline) = state.forked_child_inherited_baseline {
        return totals.is_within(baseline);
    }

    false
}

#[cfg(test)]
pub(crate) fn parse_codex_file_incremental(
    path: &Path,
    start_offset: u64,
    state: CodexParseState,
) -> SessionParseResult<ParsedCodexFile> {
    parse_codex_file_incremental_with_cancellation(path, start_offset, state, None)
}

pub(crate) fn parse_codex_file_incremental_with_cancellation(
    path: &Path,
    start_offset: u64,
    state: CodexParseState,
    cancellation: Option<&crate::engine::AcquisitionCancellation>,
) -> SessionParseResult<ParsedCodexFile> {
    parse_codex_file_incremental_hashed(path, start_offset, state, None, cancellation)?.ok_or_else(
        || {
            SessionParseError::invalid(
                "validate Codex incremental prefix",
                "input ended before the requested start offset",
            )
        },
    )
}

#[cfg(test)]
pub(crate) fn parse_codex_file_incremental_verified(
    path: &Path,
    start_offset: u64,
    state: CodexParseState,
    expected_prefix_hash: [u8; 32],
) -> SessionParseResult<Option<ParsedCodexFile>> {
    parse_codex_file_incremental_verified_with_cancellation(
        path,
        start_offset,
        state,
        expected_prefix_hash,
        None,
    )
}

pub(crate) fn parse_codex_file_incremental_verified_with_cancellation(
    path: &Path,
    start_offset: u64,
    state: CodexParseState,
    expected_prefix_hash: [u8; 32],
    cancellation: Option<&crate::engine::AcquisitionCancellation>,
) -> SessionParseResult<Option<ParsedCodexFile>> {
    parse_codex_file_incremental_hashed(
        path,
        start_offset,
        state,
        Some(expected_prefix_hash),
        cancellation,
    )
}

fn parse_codex_file_incremental_hashed(
    path: &Path,
    start_offset: u64,
    state: CodexParseState,
    expected_prefix_hash: Option<[u8; 32]>,
    cancellation: Option<&crate::engine::AcquisitionCancellation>,
) -> SessionParseResult<Option<ParsedCodexFile>> {
    let mut file = std::fs::File::open(path)
        .map_err(|source| SessionParseError::new("open Codex JSONL input", source))?;
    let input_identity = crate::input_record_cache::input_file_identity_from_open_file(&file)
        .map_err(|source| SessionParseError::new("read Codex input file identity", source))?;

    #[cfg(test)]
    crate::input_record_cache::record_input_hash_start(path);
    let mut hasher = Sha256::new();
    let mut last_byte = None;
    let mut remaining = start_offset;
    let mut buffer = [0_u8; 64 * 1024];
    while remaining > 0 {
        if cancellation.is_some_and(crate::engine::AcquisitionCancellation::is_cancelled) {
            return Err(SessionParseError::invalid(
                "read Codex incremental prefix",
                "acquisition cancelled",
            ));
        }
        let bytes_to_read = remaining.min(buffer.len() as u64) as usize;
        let read = file
            .read(&mut buffer[..bytes_to_read])
            .map_err(|source| SessionParseError::new("read Codex incremental prefix", source))?;
        if read == 0 {
            return if expected_prefix_hash.is_some() {
                Ok(None)
            } else {
                Err(SessionParseError::invalid(
                    "validate Codex incremental prefix",
                    "input ended before the requested start offset",
                ))
            };
        }
        #[cfg(test)]
        crate::input_record_cache::record_input_bytes(path, read);
        hasher.update(&buffer[..read]);
        last_byte = Some(buffer[read - 1]);
        remaining -= read as u64;
    }
    if expected_prefix_hash
        .is_some_and(|expected| <[u8; 32]>::from(hasher.clone().finalize()) != expected)
    {
        return Ok(None);
    }

    let session_id = session_id_from_path(path)?;
    let hashing_reader = HashingReader {
        inner: file,
        hasher,
        last_byte,
        #[cfg(test)]
        path: path.to_path_buf(),
    };
    let mut reader = BufReader::new(hashing_reader);
    let mut parsed =
        parse_codex_reader(&mut reader, &session_id, start_offset, state, cancellation)?;
    let hashing_reader = reader.into_inner();
    parsed.content_hash = Some(hashing_reader.hasher.finalize().into());
    parsed.ends_with_newline =
        parsed.consumed_offset == 0 || hashing_reader.last_byte == Some(b'\n');
    parsed.input_identity = Some(input_identity);
    Ok(Some(parsed))
}

fn extract_model(payload: &CodexPayload) -> Option<String> {
    payload
        .model_info
        .as_ref()
        .and_then(|mi| mi.slug.clone())
        .filter(|s| !s.is_empty())
        .or(payload.model.clone().filter(|s| !s.is_empty()))
        .or(payload.model_name.clone().filter(|s| !s.is_empty()))
        .or(payload.info.as_ref().and_then(extract_model_from_info))
}

fn extract_model_from_info(info: &CodexInfo) -> Option<String> {
    info.model
        .clone()
        .filter(|s| !s.is_empty())
        .or(info.model_name.clone().filter(|s| !s.is_empty()))
}

fn codex_schema_invalid_value_may_affect_state(value: &Value) -> bool {
    value
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|entry_type| {
            matches!(
                entry_type,
                "session_meta" | "turn_context" | "event_msg" | "token_usage_record"
            )
        })
}

/// Prefixes Codex prepends to context it injects as `user_message` events.
/// These are the bodies that must NOT be counted as human turns.
const CODEX_SYSTEM_INJECTED_PREFIXES: [&str; 3] = [
    "<environment_context>",
    "<system-reminder>",
    "<user_instructions>",
];

/// Returns true when a Codex `user_message` payload represents real human input
/// rather than system-injected context. Codex stores the body as a plain string
/// in `payload.message`; the harness injects context blocks that open with one of
/// the known tags in [`CODEX_SYSTEM_INJECTED_PREFIXES`] after trimming. Matching
/// those specific prefixes — rather than any leading `<` — avoids dropping
/// legitimate human prompts that happen to start with markup (asking about a
/// `<div>`, pasting an XML snippet, etc.). The `kind` field can't be used to
/// distinguish them: both human and injected bodies appear as `kind:"plain"` or
/// with no `kind` at all.
fn codex_message_is_human_turn(message: Option<&str>) -> bool {
    match message {
        Some(text) => {
            let trimmed = text.trim_start();
            !CODEX_SYSTEM_INJECTED_PREFIXES
                .iter()
                .any(|prefix| trimmed.starts_with(prefix))
        }
        None => false,
    }
}

#[cfg(test)]
#[path = "tests/decode.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/responses.rs"]
mod response_tests;

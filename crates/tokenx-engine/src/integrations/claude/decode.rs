//! Claude Code session decoder.
//!
//! Parses JSONL files from ~/.claude/projects/

use crate::input_health::{InputFailure, RecordRejectionReason, RejectionSummary, ScannedInput};
use crate::records::error::{SessionParseError, SessionParseResult};
use crate::records::{normalize_workspace_key, workspace_label_from_key, UsageRecord};
use crate::{provider_identity, TokenBreakdown};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::io::ErrorKind;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

/// Claude Code entry structure (from JSONL files)
#[derive(Debug, Deserialize)]
pub struct ClaudeEntry {
    #[serde(rename = "type")]
    pub entry_type: String,
    pub timestamp: Option<String>,
    pub message: Option<ClaudeMessage>,
    /// Request ID for deduplication (used with message.id)
    #[serde(rename = "requestId")]
    pub request_id: Option<String>,
    /// True for subagent (sidechain) transcript lines
    #[serde(rename = "isSidechain", default)]
    pub is_sidechain: bool,
    /// Stable subagent identifier within its parent session
    #[serde(rename = "agentId")]
    pub agent_id: Option<String>,
    /// Parent session UUID (present on every sidechain line)
    #[serde(rename = "sessionId")]
    pub session_id: Option<String>,
    /// Optional billing or routing provider emitted by wrappers around Claude Code.
    #[serde(rename = "providerId", alias = "provider_id", alias = "provider")]
    pub provider_id: Option<String>,
    /// Current working directory emitted by Claude Code for workspace identity.
    pub cwd: Option<String>,
    /// Authoritative project directory emitted by recovery or wrapper records.
    #[serde(rename = "projectPath", alias = "project_path")]
    pub project_path: Option<String>,
}

/// Meta sidecar written next to nested-layout sidechain transcripts.
/// e.g. `agent-abc123.meta.json` alongside `agent-abc123.jsonl`
#[derive(Debug, Deserialize)]
struct AgentMetaFile {
    #[serde(rename = "agentType")]
    agent_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct ClaudeWorkspaceParts {
    key: String,
    label: Option<String>,
}

impl ClaudeWorkspaceParts {
    fn to_options(&self) -> (Option<String>, Option<String>) {
        (Some(self.key.clone()), self.label.clone())
    }
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct ClaudeProjectCandidates {
    project_paths: HashSet<String>,
    cwds: HashSet<String>,
}

#[derive(Debug)]
struct ParentMetadata {
    project_candidates: ClaudeProjectCandidates,
    subagent_types: HashMap<String, String>,
}

#[derive(Debug, Clone)]
struct ParentMetadataFailure {
    operation: &'static str,
    kind: ErrorKind,
    detail: String,
}

impl ParentMetadataFailure {
    fn at_line(
        operation: &'static str,
        kind: ErrorKind,
        line_number: usize,
        source: impl std::fmt::Display,
    ) -> Self {
        Self {
            operation,
            kind,
            detail: format!("line {line_number}: {source}"),
        }
    }

    fn from_io(operation: &'static str, source: std::io::Error) -> Self {
        Self {
            operation,
            kind: source.kind(),
            detail: source.to_string(),
        }
    }

    fn invalid(operation: &'static str, detail: impl Into<String>) -> Self {
        Self {
            operation,
            kind: ErrorKind::InvalidData,
            detail: detail.into(),
        }
    }

    fn to_parse_error(&self, path: &Path) -> SessionParseError {
        SessionParseError::at_path(
            path,
            self.operation,
            std::io::Error::new(self.kind, self.detail.clone()),
        )
    }
}

type ParentMetadataResult = Result<Arc<ParentMetadata>, ParentMetadataFailure>;
type ParentMetadataCell = OnceLock<ParentMetadataResult>;

impl ClaudeProjectCandidates {
    fn record(&mut self, project_path: Option<&str>, cwd: Option<&str>) {
        if let Some(project_path) = project_path.filter(|path| !path.trim().is_empty()) {
            self.project_paths.insert(project_path.to_string());
        }
        if let Some(cwd) = cwd.filter(|path| !path.trim().is_empty()) {
            self.cwds.insert(cwd.to_string());
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ClaudeProjectResolution {
    Resolved(ClaudeWorkspaceParts),
    Unresolved,
    Ambiguous,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CandidateMatch {
    None,
    Unique(String),
    Ambiguous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum ClaudeProjectDependency {
    None,
    ParentSession,
    ExternalMetadata,
}

/// Per-scan resolver for Claude's lossy project-directory names.
///
/// Transcript JSONL remains single-pass: candidates are collected while usage
/// records are parsed. History and config are loaded only when those candidates
/// cannot identify the project, and at most once for the resolver.
#[derive(Debug)]
pub(crate) struct ClaudeProjectResolver {
    home_dir: Option<PathBuf>,
    parent_metadata: Mutex<HashMap<PathBuf, Arc<ParentMetadataCell>>>,
    external_candidates: OnceLock<ClaudeProjectCandidates>,
    reported_diagnostics: Mutex<HashSet<(String, &'static str)>>,
    #[cfg(test)]
    external_loads: AtomicUsize,
    #[cfg(test)]
    parent_metadata_loads: AtomicUsize,
}

impl ClaudeProjectResolver {
    pub(crate) fn new(home_dir: Option<&Path>) -> Self {
        Self {
            home_dir: home_dir.map(Path::to_path_buf),
            parent_metadata: Mutex::new(HashMap::new()),
            external_candidates: OnceLock::new(),
            reported_diagnostics: Mutex::new(HashSet::new()),
            #[cfg(test)]
            external_loads: AtomicUsize::new(0),
            #[cfg(test)]
            parent_metadata_loads: AtomicUsize::new(0),
        }
    }

    fn resolve(
        &self,
        project_key: &str,
        candidates: &ClaudeProjectCandidates,
        input_path: &Path,
        parent_session_id: Option<&str>,
    ) -> (ClaudeProjectResolution, ClaudeProjectDependency) {
        let local_match = match_project_candidate_tiers(project_key, candidates);
        if !matches!(local_match, CandidateMatch::None) {
            return (
                self.finish_match(project_key, local_match, input_path),
                ClaudeProjectDependency::None,
            );
        }

        if let Some(parent_session_id) = parent_session_id {
            match find_parent_session_path(input_path, parent_session_id) {
                Ok(Some(parent_path)) => match self.parent_metadata(&parent_path) {
                    Ok(metadata) => {
                        let parent_match = match_project_candidate_tiers(
                            project_key,
                            &metadata.project_candidates,
                        );
                        if !matches!(parent_match, CandidateMatch::None) {
                            return (
                                self.finish_match(project_key, parent_match, input_path),
                                ClaudeProjectDependency::ParentSession,
                            );
                        }
                    }
                    Err(error) => tracing::warn!(
                        code = "claude_project_parent_unreadable",
                        input = %parent_path.display(),
                        error = %error,
                        "could not read Claude parent session while resolving project path"
                    ),
                },
                Ok(None) => {}
                Err(error) => tracing::warn!(
                    code = "claude_project_parent_unreadable",
                    input = %input_path.display(),
                    error = %error,
                    "could not inspect Claude parent session while resolving project path"
                ),
            }
        }

        let external_match = match_project_candidates(
            project_key,
            self.external_candidates()
                .project_paths
                .iter()
                .map(String::as_str),
        );
        (
            self.finish_match(project_key, external_match, input_path),
            ClaudeProjectDependency::ExternalMetadata,
        )
    }

    fn finish_match(
        &self,
        project_key: &str,
        candidate_match: CandidateMatch,
        input_path: &Path,
    ) -> ClaudeProjectResolution {
        match candidate_match {
            CandidateMatch::Unique(path) => resolved_workspace(&path),
            CandidateMatch::Ambiguous => {
                self.report_once(project_key, "claude_project_path_ambiguous", input_path);
                ClaudeProjectResolution::Ambiguous
            }
            CandidateMatch::None => {
                self.report_once(project_key, "claude_project_path_unresolved", input_path);
                ClaudeProjectResolution::Unresolved
            }
        }
    }

    fn parent_metadata(&self, parent_path: &Path) -> SessionParseResult<Arc<ParentMetadata>> {
        let canonical_path = std::fs::canonicalize(parent_path).map_err(|source| {
            SessionParseError::at_path(parent_path, "canonicalize Claude parent session", source)
        })?;
        let cell = self
            .parent_metadata
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(canonical_path.clone())
            .or_insert_with(|| Arc::new(OnceLock::new()))
            .clone();
        cell.get_or_init(|| {
            #[cfg(test)]
            self.parent_metadata_loads.fetch_add(1, Ordering::Relaxed);
            read_parent_metadata(&canonical_path).map(Arc::new)
        })
        .clone()
        .map_err(|failure| failure.to_parse_error(&canonical_path))
    }

    fn lookup_subagent_type(
        &self,
        parent_path: &Path,
        agent_id: &str,
    ) -> SessionParseResult<Option<String>> {
        let metadata = self.parent_metadata(parent_path)?;
        Ok(metadata.subagent_types.get(agent_id).cloned())
    }

    #[cfg(test)]
    pub(crate) fn parent_metadata_load_count(&self) -> usize {
        self.parent_metadata_loads.load(Ordering::Relaxed)
    }

    fn external_candidates(&self) -> &ClaudeProjectCandidates {
        self.external_candidates.get_or_init(|| {
            #[cfg(test)]
            self.external_loads.fetch_add(1, Ordering::Relaxed);
            self.home_dir
                .as_deref()
                .map(read_external_project_candidates)
                .unwrap_or_default()
        })
    }

    fn report_once(&self, project_key: &str, code: &'static str, input_path: &Path) {
        let inserted = self
            .reported_diagnostics
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert((project_key.to_string(), code));
        if inserted {
            tracing::warn!(
                code,
                project_key,
                    input = %input_path.display(),
                "Claude project path could not be resolved uniquely; usage is retained without workspace metadata"
            );
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ClaudeMessage {
    pub model: Option<String>,
    pub usage: Option<ClaudeUsage>,
    /// Message ID for deduplication (used with requestId)
    pub id: Option<String>,
    /// Optional billing or routing provider emitted by wrappers around Claude Code.
    #[serde(rename = "providerId", alias = "provider_id", alias = "provider")]
    pub provider_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ClaudeUsage {
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_read_input_tokens: Option<i64>,
    pub cache_creation_input_tokens: Option<i64>,
    pub cache_creation: Option<ClaudeCacheCreation>,
    pub iterations: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct ClaudeCacheCreation {
    pub ephemeral_1h_input_tokens: Option<i64>,
}

impl ClaudeUsage {
    fn tokens(&self) -> TokenBreakdown {
        TokenBreakdown {
            input: self.input_tokens.unwrap_or(0),
            output: self.output_tokens.unwrap_or(0),
            cache_read: self.cache_read_input_tokens.unwrap_or(0),
            cache_write: self.cache_creation_input_tokens.unwrap_or(0),
            cache_write_1h: self.cache_write_1h(),
            reasoning: 0,
        }
    }

    fn reports_input_split(&self) -> bool {
        self.input_tokens.is_some()
            && (self.cache_read_input_tokens.is_some()
                || self.cache_creation_input_tokens.is_some())
    }

    fn cache_write_1h(&self) -> i64 {
        self.cache_creation
            .as_ref()
            .and_then(|creation| creation.ephemeral_1h_input_tokens)
            .unwrap_or(0)
    }
}

fn normalize_claude_agent_label(agent_type: &str) -> Option<String> {
    let normalized = agent_type.trim().to_ascii_lowercase();
    let normalized = normalized
        .strip_prefix("oh-my-claudecode:")
        .unwrap_or(&normalized);

    match normalized {
        "explore" => Some("Claude Explore".to_string()),
        "plan" => Some("Claude Plan".to_string()),
        "general-purpose" => Some("Claude General Purpose".to_string()),
        "claude-code-guide" => Some("Claude Code Guide".to_string()),
        "verification" => Some("Claude Verification".to_string()),
        "workflow-subagent" => Some("Claude Workflow Subagent".to_string()),
        "fork" => Some("Claude Fork".to_string()),
        _ => None,
    }
}

/// Resolve the subagent display name for a sidechain transcript file.
///
/// Tier 1: Read the sibling `.meta.json` sidecar for the `agentType` field.
/// Tier 2: Scan the parent session JSONL for the tool_use that spawned this agent.
/// Tier 3: Fall back to a generic "claude-code-subagent" label.
fn resolve_subagent_name(
    path: &Path,
    parent_session_id: Option<&str>,
    entry_agent_id: Option<&str>,
    project_resolver: &ClaudeProjectResolver,
) -> SessionParseResult<String> {
    let stem = match path.file_stem().and_then(|s| s.to_str()) {
        Some(s) => s,
        None => {
            return Err(SessionParseError::at_path(
                path,
                "validate Claude sidechain input path",
                std::io::Error::new(ErrorKind::InvalidData, "input file name is not valid UTF-8"),
            ));
        }
    };

    // Tier 1: sibling meta.json (e.g. agent-abc123.meta.json next to agent-abc123.jsonl)
    let meta_path = path.with_file_name(format!("{}.meta.json", stem));
    let meta_text = match std::fs::read_to_string(&meta_path) {
        Ok(text) => Some(text),
        Err(source) if source.kind() == ErrorKind::NotFound => None,
        Err(source) => {
            return Err(SessionParseError::at_path(
                &meta_path,
                "read Claude sidechain metadata",
                source,
            ));
        }
    };
    if let Some(text) = meta_text {
        let meta: AgentMetaFile = serde_json::from_str(&text).map_err(|source| {
            SessionParseError::at_path(&meta_path, "decode Claude sidechain metadata", source)
        })?;
        let agent_type = meta.agent_type.as_deref().ok_or_else(|| {
            SessionParseError::at_path(
                &meta_path,
                "validate Claude sidechain metadata",
                std::io::Error::new(ErrorKind::InvalidData, "missing agentType"),
            )
        })?;
        let agent_type = agent_type.trim();
        if agent_type.is_empty() {
            return Err(SessionParseError::at_path(
                &meta_path,
                "validate Claude sidechain metadata",
                std::io::Error::new(ErrorKind::InvalidData, "agentType is blank"),
            ));
        }
        return Ok(normalize_claude_agent_label(agent_type)
            .unwrap_or_else(|| "Claude Subagent".to_string()));
    }

    // Tier 2: parent session tool_use inference
    let lookup_agent_id = entry_agent_id
        .filter(|agent_id| !agent_id.trim().is_empty())
        .map(|agent_id| agent_id.to_string())
        .or_else(|| sidechain_agent_id_from_stem(stem));
    if let (Some(parent_id), Some(agent_id)) = (parent_session_id, lookup_agent_id.as_deref()) {
        if let Some(parent_path) = find_parent_session_path(path, parent_id)? {
            if let Some(subagent_type) =
                project_resolver.lookup_subagent_type(&parent_path, agent_id)?
            {
                let subagent_type = subagent_type.trim();
                if subagent_type.is_empty() {
                    return Err(SessionParseError::at_path(
                        &parent_path,
                        "validate Claude parent sidechain type",
                        std::io::Error::new(ErrorKind::InvalidData, "subagent_type is blank"),
                    ));
                }
                return Ok(normalize_claude_agent_label(subagent_type)
                    .unwrap_or_else(|| "Claude Subagent".to_string()));
            }
        }
    }

    // Tier 3: generic fallback (still visible in the Agents tab)
    Ok("Claude Subagent".to_string())
}

pub(super) fn is_workflow_journal(path: &Path) -> bool {
    path.file_name().and_then(|name| name.to_str()) == Some("journal.jsonl")
        && path.ancestors().any(|ancestor| {
            ancestor.file_name().and_then(|name| name.to_str()) == Some("subagents")
        })
}

pub(crate) fn nested_parent_session_path(sidechain_path: &Path) -> Option<PathBuf> {
    let subagents_dir = sidechain_path.ancestors().find(|ancestor| {
        ancestor.file_name().and_then(|name| name.to_str()) == Some("subagents")
    })?;
    let session_dir = subagents_dir.parent()?;
    let project_dir = session_dir.parent()?;
    let mut parent_filename = session_dir.file_name()?.to_os_string();
    parent_filename.push(".jsonl");
    Some(project_dir.join(parent_filename))
}

/// Locate the parent main-session JSONL for a sidechain transcript.
///
/// Nested layout: `.../projects/<key>/<session>/subagents/agent-X.jsonl`
///   → parent at `.../projects/<key>/<session>.jsonl`
/// Workflow layout: `.../<session>/subagents/workflows/<workflow>/agent-X.jsonl`
///   → parent at `.../projects/<key>/<session>.jsonl`
/// Flat layout: `.../projects/<key>/agent-X.jsonl`
///   → parent at `.../projects/<key>/<session-id>.jsonl`
fn find_parent_session_path(
    sidechain_path: &Path,
    parent_session_id: &str,
) -> SessionParseResult<Option<PathBuf>> {
    if let Some(candidate) = nested_parent_session_path(sidechain_path) {
        match std::fs::metadata(&candidate) {
            Ok(_) => return Ok(Some(candidate)),
            Err(source) if source.kind() == ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(SessionParseError::at_path(
                    &candidate,
                    "inspect Claude parent session",
                    source,
                ));
            }
        }
    }

    // Flat layout: parent dir is 1 level up
    if let Some(project_dir) = sidechain_path.parent() {
        let candidate = project_dir.join(format!("{}.jsonl", parent_session_id));
        match std::fs::metadata(&candidate) {
            Ok(_) => return Ok(Some(candidate)),
            Err(source) if source.kind() == ErrorKind::NotFound => {}
            Err(source) => {
                return Err(SessionParseError::at_path(
                    &candidate,
                    "inspect Claude parent session",
                    source,
                ));
            }
        }
    }

    Ok(None)
}

/// Read all parent-session metadata needed by sidechains in one pass.
///
/// The parent session contains:
/// - project paths and working directories used for workspace resolution;
/// - Assistant `tool_use` blocks (`name: "Agent"`, `input.subagent_type`);
/// - User `tool_result` blocks whose text contains `agentId: <hex>`.
///
/// We join on `tool_use_id` to map `agentId → subagent_type`.
fn read_parent_metadata(parent_path: &Path) -> Result<ParentMetadata, ParentMetadataFailure> {
    let file = std::fs::File::open(parent_path)
        .map_err(|source| ParentMetadataFailure::from_io("open Claude parent metadata", source))?;
    let reader = BufReader::new(file);

    let mut project_candidates = ClaudeProjectCandidates::default();
    // tool_use.id → subagent_type
    let mut tool_use_types: HashMap<String, String> = HashMap::new();
    // tool_use_id → agentId (from tool_result text)
    let mut agent_id_links: HashMap<String, String> = HashMap::new();

    for (line_index, line) in reader.lines().enumerate() {
        let line = line.map_err(|source| {
            ParentMetadataFailure::at_line(
                "read Claude parent metadata",
                source.kind(),
                line_index + 1,
                source,
            )
        })?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let has_subagent_type = trimmed.contains("subagent_type");
        let has_agent_id_text = trimmed.contains("agentId:");
        let value: serde_json::Value = serde_json::from_str(trimmed).map_err(|source| {
            ParentMetadataFailure::at_line(
                "decode Claude parent metadata",
                ErrorKind::InvalidData,
                line_index + 1,
                source,
            )
        })?;

        if value
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|entry_type| !entry_type.trim().is_empty())
        {
            project_candidates.record(
                value
                    .get("projectPath")
                    .or_else(|| value.get("project_path"))
                    .and_then(Value::as_str),
                value.get("cwd").and_then(Value::as_str),
            );
        }
        if !has_subagent_type && !has_agent_id_text {
            continue;
        }

        let content = match value
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
        {
            Some(arr) => arr,
            None => continue,
        };

        for block in content {
            let block_type = block
                .get("type")
                .and_then(|value| value.as_str())
                .ok_or_else(|| {
                    ParentMetadataFailure::invalid(
                        "validate Claude parent session line",
                        format!(
                            "line {}: message content block is missing type",
                            line_index + 1
                        ),
                    )
                })?;

            match block_type {
                "tool_use" if has_subagent_type => {
                    let Some(subagent_type) = block
                        .get("input")
                        .and_then(|input| input.get("subagent_type"))
                        .and_then(|value| value.as_str())
                    else {
                        continue;
                    };
                    let id = block
                        .get("id")
                        .and_then(|value| value.as_str())
                        .ok_or_else(|| {
                            ParentMetadataFailure::invalid(
                                "validate Claude parent session line",
                                format!("line {}: subagent tool_use is missing id", line_index + 1),
                            )
                        })?;
                    if subagent_type.trim().is_empty() {
                        return Err(ParentMetadataFailure::invalid(
                            "validate Claude parent session line",
                            format!("line {}: subagent_type is blank", line_index + 1),
                        ));
                    }
                    tool_use_types.insert(id.to_string(), subagent_type.to_string());
                }
                "tool_result" if has_agent_id_text => {
                    let block_contains_agent_id = block
                        .get("content")
                        .is_some_and(|content| content.to_string().contains("agentId:"));
                    if !block_contains_agent_id {
                        continue;
                    }
                    let tool_use_id = match block.get("tool_use_id").and_then(|i| i.as_str()) {
                        Some(id) => id.to_string(),
                        None => {
                            return Err(ParentMetadataFailure::invalid(
                                "validate Claude parent session line",
                                format!(
                                    "line {}: agent tool_result is missing tool_use_id",
                                    line_index + 1
                                ),
                            ));
                        }
                    };
                    // Walk content blocks looking for "agentId: <hex>" in text
                    let result_content = block
                        .get("content")
                        .and_then(|content| content.as_array())
                        .ok_or_else(|| {
                            ParentMetadataFailure::invalid(
                                "validate Claude parent session line",
                                format!(
                                    "line {}: agent tool_result content is not an array",
                                    line_index + 1
                                ),
                            )
                        })?;
                    let mut linked_agent = false;
                    for cb in result_content {
                        if let Some(text) = cb.get("text").and_then(|t| t.as_str()) {
                            if let Some(aid) = extract_agent_id_from_text(text) {
                                agent_id_links.insert(tool_use_id.clone(), aid);
                                linked_agent = true;
                                break;
                            }
                        }
                    }
                    if !linked_agent {
                        return Err(ParentMetadataFailure::invalid(
                            "validate Claude parent session line",
                            format!(
                                "line {}: agent tool_result contains no valid agentId",
                                line_index + 1
                            ),
                        ));
                    }
                }
                _ => {}
            }
        }
    }

    let mut subagent_types = HashMap::new();
    for (tool_use_id, agent_id) in &agent_id_links {
        if let Some(subagent_type) = tool_use_types.get(tool_use_id) {
            subagent_types.insert(agent_id.clone(), subagent_type.clone());
        }
    }

    Ok(ParentMetadata {
        project_candidates,
        subagent_types,
    })
}

fn sidechain_agent_id_from_stem(stem: &str) -> Option<String> {
    let agent_stem = stem.strip_prefix("agent-")?;
    if !agent_stem.contains('-') {
        return Some(agent_stem.to_string());
    }

    let trailing_segment = agent_stem.rsplit('-').next()?;
    if trailing_segment.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(trailing_segment.to_string())
    } else {
        Some(agent_stem.to_string())
    }
}

/// Extract the `agentId` hex string from a tool_result text block.
/// Matches the pattern `agentId: <alphanumeric>` written by Claude Code's Agent tool.
fn extract_agent_id_from_text(text: &str) -> Option<String> {
    let marker = "agentId: ";
    let pos = text.find(marker)?;
    let start = pos + marker.len();
    let rest = &text[start..];
    let end = rest
        .find(|c: char| !c.is_ascii_alphanumeric())
        .unwrap_or(rest.len());
    if end > 0 {
        Some(rest[..end].to_string())
    } else {
        None
    }
}

/// Parse a Claude Code JSONL file
#[cfg(test)]
pub fn parse_claude_file(path: &Path) -> SessionParseResult<ScannedInput> {
    let home_dir = dirs::home_dir();
    parse_claude_file_with_home(path, home_dir.as_deref())
}

#[cfg(test)]
pub fn parse_claude_file_with_home(
    path: &Path,
    home_dir: Option<&Path>,
) -> SessionParseResult<ScannedInput> {
    let project_resolver = ClaudeProjectResolver::new(home_dir);
    parse_claude_file_with_project_resolver(path, &project_resolver, None)
        .map(|(scanned, _)| scanned)
}

pub(crate) fn parse_claude_file_with_project_resolver_and_cancellation(
    path: &Path,
    project_resolver: &ClaudeProjectResolver,
    cancellation: Option<&crate::engine::AcquisitionCancellation>,
) -> SessionParseResult<(ScannedInput, ClaudeProjectDependency)> {
    parse_claude_file_with_project_resolver(path, project_resolver, cancellation)
}

fn parse_claude_file_with_project_resolver(
    path: &Path,
    project_resolver: &ClaudeProjectResolver,
    cancellation: Option<&crate::engine::AcquisitionCancellation>,
) -> SessionParseResult<(ScannedInput, ClaudeProjectDependency)> {
    if is_workflow_journal(path) {
        return Ok((
            ScannedInput::complete(Vec::new()),
            ClaudeProjectDependency::None,
        ));
    }

    let file = std::fs::File::open(path)
        .map_err(|source| SessionParseError::at_path(path, "open Claude session", source))?;
    let mut reader = BufReader::new(file);
    let parsed = parse_claude_reader(
        path,
        &mut reader,
        project_resolver,
        ClaudeParseState::new(path)?,
        Vec::new(),
        0,
        None,
        cancellation,
    )?;
    Ok(parsed.finish(path, project_resolver))
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct CostSnapshot {
    records: Vec<UsageRecord>,
    rejections: RejectionSummary,
}

fn cost_snapshot(line: &str) -> CostSnapshot {
    let mut snapshot = CostSnapshot::default();
    super::cost_state::append_records(line, &mut snapshot.records, &mut snapshot.rejections);
    snapshot
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct ClaudeParseState {
    project_candidates: ClaudeProjectCandidates,
    parent_session_id: Option<String>,
    session_id: String,
    provider_confidences: Vec<u8>,
    pending_turn_start: bool,
    sidechain_agent: Option<String>,
    sidechain_agent_instance: Option<String>,
    sidechain_detected: bool,
    is_main_session: bool,
    last_cost_state: Option<CostSnapshot>,
    rejections: RejectionSummary,
    lines: usize,
}

impl ClaudeParseState {
    pub(super) fn new(path: &Path) -> SessionParseResult<Self> {
        let session_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| {
                SessionParseError::invalid(
                    "validate Claude session input path",
                    "input file stem is missing or not valid UTF-8",
                )
            })?
            .to_string();
        Ok(Self {
            project_candidates: Default::default(),
            parent_session_id: None,
            session_id,
            provider_confidences: Vec::new(),
            pending_turn_start: false,
            sidechain_agent: None,
            sidechain_agent_instance: None,
            sidechain_detected: false,
            is_main_session: true,
            last_cost_state: None,
            rejections: Default::default(),
            lines: 0,
        })
    }

    pub(super) fn matches_records(&self, records: &[UsageRecord]) -> bool {
        self.provider_confidences.len() == records.len()
            && records.iter().all(|record| !record.claude_is_cost_snapshot)
    }
}

#[derive(Debug)]
pub(crate) struct RawClaudeScan {
    pub records: Vec<UsageRecord>,
    pub state: ClaudeParseState,
    pub interrupted: Option<InputFailure>,
    pub consumed_offset: u64,
    pub ends_with_newline: bool,
    pub content_hash: Option<[u8; 32]>,
}

impl RawClaudeScan {
    pub(super) fn health(&self) -> crate::integrations::UnitScanHealth {
        let mut rejections = self.state.rejections.clone();
        if let Some(snapshot) = &self.state.last_cost_state {
            rejections.merge(&snapshot.rejections);
        }
        crate::integrations::UnitScanHealth {
            status: self
                .interrupted
                .clone()
                .map_or(crate::input_health::InputStatus::Complete, |failure| {
                    crate::input_health::InputStatus::Partial { failure }
                }),
            rejections,
        }
    }
    pub(super) fn project_context(
        &self,
        path: &Path,
        resolver: &ClaudeProjectResolver,
    ) -> ClaudeProjectContext {
        resolve_project_workspace(
            resolver,
            claude_project_key_from_path(path).as_deref(),
            &self.state.project_candidates,
            path,
            self.state.parent_session_id.as_deref(),
        )
    }

    pub(super) fn finish(
        self,
        path: &Path,
        resolver: &ClaudeProjectResolver,
    ) -> (ScannedInput, ClaudeProjectDependency) {
        let project = self.project_context(path, resolver);
        let dependency = project.dependency;
        (self.finish_with_project(&project), dependency)
    }

    pub(super) fn finish_with_project(self, project: &ClaudeProjectContext) -> ScannedInput {
        let Self {
            records: mut messages,
            state,
            interrupted,
            ..
        } = self;
        let mut rejections = state.rejections;
        for message in &mut messages {
            message.tokens.cache_write_1h = message
                .tokens
                .cache_write_1h
                .min(message.tokens.cache_write);
        }
        messages.retain(|message| crate::finalization::has_positive_tokens(&message.tokens));
        if let Some(snapshot) = state.last_cost_state {
            messages.extend(snapshot.records);
            rejections.merge(&snapshot.rejections);
        }
        project.apply(&mut messages);
        ScannedInput {
            messages,
            rejections,
            interrupted,
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn parse_claude_reader(
    path: &Path,
    reader: &mut impl BufRead,
    project_resolver: &ClaudeProjectResolver,
    state: ClaudeParseState,
    mut messages: Vec<UsageRecord>,
    mut consumed_offset: u64,
    mut hasher: Option<Sha256>,
    cancellation: Option<&crate::engine::AcquisitionCancellation>,
) -> SessionParseResult<RawClaudeScan> {
    let ClaudeParseState {
        mut project_candidates,
        mut parent_session_id,
        mut session_id,
        mut provider_confidences,
        mut pending_turn_start,
        mut sidechain_agent,
        mut sidechain_agent_instance,
        mut sidechain_detected,
        mut is_main_session,
        mut last_cost_state,
        mut rejections,
        mut lines,
    } = state;
    let workspace_key = None;
    let workspace_label = None;
    let mut interrupted = None;
    let mut processed_hashes: HashMap<u64, usize> = messages
        .iter()
        .enumerate()
        .filter_map(|(index, record)| record.dedup_key.map(|key| (key, index)))
        .collect();
    let mut buffer = Vec::with_capacity(4096);
    let mut line = String::with_capacity(4096);
    let mut ends_with_newline = consumed_offset > 0;
    loop {
        if cancellation.is_some_and(crate::engine::AcquisitionCancellation::is_cancelled) {
            interrupted = Some(InputFailure::new(
                "parse Claude session",
                "acquisition cancelled",
            ));
            break;
        }
        line.clear();
        let count = match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(count) => count,
            Err(source) => {
                interrupted = Some(InputFailure::from(&SessionParseError::at_path(
                    path,
                    "read Claude session line",
                    source,
                )));
                break;
            }
        };
        consumed_offset = consumed_offset.checked_add(count as u64).ok_or_else(|| {
            SessionParseError::invalid("track Claude input offset", "byte offset exceeds u64")
        })?;
        if let Some(hasher) = hasher.as_mut() {
            hasher.update(line.as_bytes());
        }
        ends_with_newline = line.ends_with('\n');
        let line_index = lines;
        lines = lines.checked_add(1).ok_or_else(|| {
            SessionParseError::invalid("track Claude input lines", "line count exceeds usize")
        })?;
        #[cfg(test)]
        super::incremental::record_decoded_line(path);

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        buffer.clear();
        buffer.extend_from_slice(trimmed.as_bytes());
        let entry = match simd_json::from_slice::<ClaudeEntry>(&mut buffer) {
            Ok(entry) => entry,
            Err(source) => {
                if super::cost_state::is_snapshot(trimmed) {
                    // A malformed snapshot belongs to its own decoder. It must
                    // replace the previous snapshot and not stop later usage.
                    last_cost_state = Some(cost_snapshot(&line));
                    continue;
                }
                let error = SessionParseError::at_path(
                    path,
                    "decode Claude session line",
                    std::io::Error::new(
                        ErrorKind::InvalidData,
                        format!("line {}: {source}", line_index + 1),
                    ),
                );
                record_claude_rejection(
                    &mut rejections,
                    RecordRejectionReason::MalformedRecord,
                    &error,
                );
                interrupted = Some(InputFailure::from(&error));
                break;
            }
        };
        {
            if entry.entry_type.trim().is_empty() {
                let error = SessionParseError::at_path(
                    path,
                    "validate Claude session entry",
                    std::io::Error::new(
                        ErrorKind::InvalidData,
                        format!("line {}: entry type is blank", line_index + 1),
                    ),
                );
                record_claude_rejection(
                    &mut rejections,
                    RecordRejectionReason::MalformedRecord,
                    &error,
                );
                continue;
            }
            if entry.entry_type == "cost-state" {
                // Claude restores the last complete cost-state object, rather
                // than summing snapshots or retaining removed model entries.
                last_cost_state = Some(cost_snapshot(&line));
                continue;
            }
            project_candidates.record(entry.project_path.as_deref(), entry.cwd.as_deref());
            let entry_workspace = entry
                .project_path
                .as_deref()
                .or(entry.cwd.as_deref())
                .and_then(workspace_parts_from_key);
            // Detect sidechain on the first parseable entry (any type).
            // All lines in a subagent file carry isSidechain: true.
            if !sidechain_detected {
                if entry.entry_type == "fork-context-ref" {
                    is_main_session = false;
                    // Preserve the legacy Total key for independent fork-context
                    // transcripts instead of folding later sidechain rows into
                    // the parent session ID.
                    sidechain_detected = true;
                } else if entry.is_sidechain {
                    let parent_id = match entry
                        .session_id
                        .as_deref()
                        .filter(|parent_id| !parent_id.trim().is_empty())
                    {
                        Some(parent_id) => parent_id,
                        None => {
                            let error = SessionParseError::at_path(
                                path,
                                "validate Claude sidechain session",
                                std::io::Error::new(
                                    ErrorKind::InvalidData,
                                    format!(
                                        "line {}: sidechain entry is missing sessionId",
                                        line_index + 1
                                    ),
                                ),
                            );
                            record_claude_rejection(
                                &mut rejections,
                                RecordRejectionReason::MalformedRecord,
                                &error,
                            );
                            continue;
                        }
                    };
                    sidechain_detected = true;
                    is_main_session = false;
                    session_id = parent_id.to_string();
                    parent_session_id = Some(parent_id.to_string());
                    let stem_agent_id = path
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .and_then(sidechain_agent_id_from_stem);
                    sidechain_agent_instance = entry.agent_id.clone().or(stem_agent_id);
                    let agent = match resolve_subagent_name(
                        path,
                        entry.session_id.as_deref(),
                        entry.agent_id.as_deref(),
                        project_resolver,
                    ) {
                        Ok(agent) => agent,
                        Err(error) => {
                            record_claude_rejection(
                                &mut rejections,
                                RecordRejectionReason::MalformedRecord,
                                &error,
                            );
                            "Claude Subagent".to_string()
                        }
                    };
                    sidechain_agent = Some(agent);
                } else {
                    sidechain_detected = true;
                }
            }

            if entry.entry_type == "user" && is_human_turn(trimmed) {
                pending_turn_start = true;
            }

            // Only process assistant messages with usage data
            if entry.entry_type == "assistant" {
                let message = match entry.message {
                    Some(m) => m,
                    None => continue,
                };
                let token_breakdown = message
                    .usage
                    .as_ref()
                    .map(ClaudeUsage::tokens)
                    .unwrap_or_default();
                if crate::finalization::positive_token_total(&token_breakdown).is_none() {
                    let error = SessionParseError::at_path(
                        path,
                        "validate Claude assistant usage",
                        std::io::Error::new(
                            ErrorKind::InvalidData,
                            format!(
                                "line {}: token bucket is negative or token total exceeds i64::MAX",
                                line_index + 1
                            ),
                        ),
                    );
                    record_claude_rejection(
                        &mut rejections,
                        RecordRejectionReason::MalformedRecord,
                        &error,
                    );
                    continue;
                }
                let has_positive_usage = crate::finalization::has_positive_tokens(&token_breakdown);

                if message
                    .model
                    .as_deref()
                    .is_some_and(|model| model.trim().is_empty())
                {
                    if !has_positive_usage {
                        pending_turn_start = false;
                        continue;
                    }
                    let error = SessionParseError::at_path(
                        path,
                        "validate Claude assistant message",
                        std::io::Error::new(
                            ErrorKind::InvalidData,
                            format!("line {}: assistant model is blank", line_index + 1),
                        ),
                    );
                    record_claude_rejection(
                        &mut rejections,
                        RecordRejectionReason::MissingModel,
                        &error,
                    );
                    pending_turn_start = false;
                    continue;
                }

                if message
                    .model
                    .as_deref()
                    .is_some_and(is_claude_synthetic_placeholder_model)
                {
                    continue;
                }

                let usage = match message.usage {
                    Some(usage) => usage,
                    None => continue,
                };

                let parsed_timestamp = match parse_claude_entry_timestamp_checked(
                    path,
                    line_index + 1,
                    entry.timestamp.as_deref(),
                ) {
                    Ok(Some(timestamp)) => timestamp,
                    Ok(None) => {
                        if !has_positive_usage {
                            pending_turn_start = false;
                            continue;
                        }
                        let error = SessionParseError::at_path(
                            path,
                            "validate Claude assistant timestamp",
                            std::io::Error::new(
                                ErrorKind::InvalidData,
                                format!(
                                    "line {}: token-bearing assistant message is missing timestamp",
                                    line_index + 1
                                ),
                            ),
                        );
                        record_claude_rejection(
                            &mut rejections,
                            RecordRejectionReason::MissingTimestamp,
                            &error,
                        );
                        pending_turn_start = false;
                        continue;
                    }
                    Err(error) => {
                        if has_positive_usage {
                            record_claude_rejection(
                                &mut rejections,
                                RecordRejectionReason::MissingTimestamp,
                                &error,
                            );
                        }
                        pending_turn_start = false;
                        continue;
                    }
                };

                let provider_hint = message.provider_id.clone().or(entry.provider_id.clone());

                // Build dedup key for global deduplication (messageId:requestId composite).
                // Merge streaming counters under the input convention reported by
                // each copy; explicit split input takes precedence over snapshots.
                let pending_hash = match (&message.id, &entry.request_id) {
                    (Some(message_id), Some(request_id)) => Some(crate::records::dedup_hash_str(
                        &format!("{message_id}:{request_id}"),
                    )),
                    (Some(message_id), None) => Some(crate::records::dedup_hash_str(&format!(
                        "message:{message_id}"
                    ))),
                    _ => None,
                };
                let parent_idx = if let Some(existing_idx) =
                    pending_hash.and_then(|key| processed_hashes.get(&key).copied())
                {
                    let duplicate_model = message
                        .model
                        .as_deref()
                        .map(str::trim)
                        .unwrap_or(messages[existing_idx].model_id.as_ref());
                    let provider_choice = claude_provider_choice_from_parts(
                        Some(duplicate_model),
                        provider_hint.as_deref(),
                    );
                    let existing = &mut messages[existing_idx];
                    merge_claude_duplicate(
                        existing,
                        &usage,
                        parsed_timestamp,
                        provider_hint.as_deref(),
                    );
                    if let Some(workspace) = entry_workspace.as_ref() {
                        set_message_workspace(existing, workspace);
                    }
                    if let Some(choice) = provider_choice {
                        update_claude_provider_id(
                            &mut existing.provider_id,
                            &mut provider_confidences[existing_idx],
                            choice,
                        );
                    }
                    existing_idx
                } else {
                    let raw_model = match message.model {
                        Some(m) => m,
                        None => {
                            if !has_positive_usage {
                                pending_turn_start = false;
                                continue;
                            }
                            let error = SessionParseError::at_path(
                                path,
                                "validate Claude assistant message",
                                std::io::Error::new(
                                    ErrorKind::InvalidData,
                                    format!(
                                        "line {}: token-bearing assistant message is missing model",
                                        line_index + 1
                                    ),
                                ),
                            );
                            record_claude_rejection(
                                &mut rejections,
                                RecordRejectionReason::MissingModel,
                                &error,
                            );
                            pending_turn_start = false;
                            continue;
                        }
                    };
                    let model = observed_claude_model(&raw_model);
                    let provider_choice = claude_provider_choice_for_models(
                        &raw_model,
                        &model,
                        provider_hint.as_deref(),
                    );
                    let provider_confidence = provider_choice.confidence;

                    // Insert dedup index only after all checks pass, right before push
                    let dedup_key = pending_hash.inspect(|hash| {
                        processed_hashes.insert(*hash, messages.len());
                    });

                    let mut message = UsageRecord::new_with_dedup(
                        model,
                        provider_choice.id,
                        session_id.clone(),
                        parsed_timestamp,
                        token_breakdown,
                        0.0,
                        dedup_key,
                    );
                    message.set_billing_provider(provider_hint.as_deref());
                    message.claude_input_is_split = usage.reports_input_split();
                    message.claude_session_id = entry
                        .session_id
                        .as_deref()
                        .filter(|id| !id.trim().is_empty())
                        .map(crate::records::intern::intern);
                    message.is_main_session = is_main_session;
                    message.agent = sidechain_agent
                        .as_deref()
                        .map(crate::records::intern::intern);
                    message.set_agent_instance(sidechain_agent_instance.clone());
                    let (message_workspace_key, message_workspace_label) =
                        workspace_options_for_entry(
                            entry_workspace.as_ref(),
                            &workspace_key,
                            &workspace_label,
                        );
                    message.set_workspace(message_workspace_key, message_workspace_label);
                    // Mark the first assistant response after a user message as a turn start
                    if pending_turn_start {
                        message.is_turn_start = true;
                        pending_turn_start = false;
                    }
                    messages.push(message);
                    provider_confidences.push(provider_confidence);
                    messages.len() - 1
                };
                let advisors = advisor_records(
                    &usage,
                    &messages[parent_idx],
                    provider_hint.as_deref(),
                    &mut rejections,
                );
                for (record, advisor_usage, choice) in advisors {
                    if let Some(index) = record
                        .dedup_key
                        .and_then(|key| processed_hashes.get(&key).copied())
                    {
                        merge_claude_duplicate(
                            &mut messages[index],
                            &advisor_usage,
                            record.timestamp,
                            provider_hint.as_deref(),
                        );
                        update_claude_provider_id(
                            &mut messages[index].provider_id,
                            &mut provider_confidences[index],
                            choice,
                        );
                    } else {
                        if let Some(key) = record.dedup_key {
                            processed_hashes.insert(key, messages.len());
                        }
                        provider_confidences.push(choice.confidence);
                        messages.push(record);
                    }
                }
            }
        }
    }

    Ok(RawClaudeScan {
        records: messages,
        state: ClaudeParseState {
            project_candidates,
            parent_session_id,
            session_id,
            provider_confidences,
            pending_turn_start,
            sidechain_agent,
            sidechain_agent_instance,
            sidechain_detected,
            is_main_session,
            last_cost_state,
            rejections,
            lines,
        },
        interrupted,
        consumed_offset,
        ends_with_newline,
        content_hash: hasher.map(|hasher| hasher.finalize().into()),
    })
}

/// Advisor counters are additional calls, whereas ordinary iterations are
/// already included in their parent's usage. Array positions identify calls
/// across streamed duplicates, appends, and mirrored transcripts.
fn advisor_records(
    usage: &ClaudeUsage,
    parent: &UsageRecord,
    provider_hint: Option<&str>,
    rejections: &mut RejectionSummary,
) -> Vec<(UsageRecord, ClaudeUsage, ClaudeProviderChoice)> {
    let Some(iterations) = usage.iterations.as_ref() else {
        return Vec::new();
    };
    let Some(iterations) = iterations.as_array() else {
        rejections.record(RecordRejectionReason::MalformedRecord);
        return Vec::new();
    };
    let mut records = Vec::new();
    for (index, iteration) in iterations.iter().enumerate() {
        if iteration.get("type").and_then(Value::as_str) != Some("advisor_message") {
            continue;
        }
        let Some(model) = iteration
            .get("model")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|model| !model.is_empty())
        else {
            rejections.record(RecordRejectionReason::MissingModel);
            continue;
        };
        if is_claude_synthetic_placeholder_model(model) {
            continue;
        }
        let Ok(usage) = serde_json::from_value::<ClaudeUsage>(iteration.clone()) else {
            rejections.record(RecordRejectionReason::MalformedRecord);
            continue;
        };
        let tokens = usage.tokens();
        if crate::finalization::positive_token_total(&tokens).is_none() {
            rejections.record(RecordRejectionReason::MalformedRecord);
            continue;
        }
        let choice = claude_provider_choice(model, provider_hint);
        let mut record = parent.clone();
        record.model_id = crate::records::intern::intern(model);
        record.raw_model_id = record.model_id.clone();
        record.provider_id = crate::records::intern::intern(&choice.id);
        record.tokens = tokens;
        record.claude_input_is_split = usage.reports_input_split();
        record.message_count = 0;
        record.is_turn_start = false;
        record.dedup_key = parent
            .dedup_key
            .map(|key| crate::records::dedup_hash_str(&format!("claude-advisor:{key}:{index}")));
        records.push((record, usage, choice));
    }
    records
}

fn record_claude_rejection(
    rejections: &mut RejectionSummary,
    reason: RecordRejectionReason,
    _error: &SessionParseError,
) {
    rejections.record(reason);
}

fn claude_project_key_from_path(path: &Path) -> Option<String> {
    let components: Vec<String> = path
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_string())
        .collect();

    for window in components.windows(3) {
        if window[0] == ".claude" && window[1] == "projects" {
            return Some(window[2].clone());
        }
    }

    for window in components.windows(2).rev() {
        if window[0] == "projects" {
            return Some(window[1].clone());
        }
    }

    None
}

fn workspace_parts_from_key(raw: &str) -> Option<ClaudeWorkspaceParts> {
    let key = normalize_workspace_key(raw)?;
    let label = workspace_label_from_key(&key);
    Some(ClaudeWorkspaceParts { key, label })
}

fn set_message_workspace(message: &mut UsageRecord, workspace: &ClaudeWorkspaceParts) {
    let (workspace_key, workspace_label) = workspace.to_options();
    message.set_workspace(workspace_key, workspace_label);
}

fn workspace_options_for_entry(
    entry_workspace: Option<&ClaudeWorkspaceParts>,
    fallback_key: &Option<String>,
    fallback_label: &Option<String>,
) -> (Option<String>, Option<String>) {
    if let Some(workspace) = entry_workspace {
        workspace.to_options()
    } else {
        (fallback_key.clone(), fallback_label.clone())
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
enum ProjectWorkspace {
    PerRecord,
    Shared(Option<ClaudeWorkspaceParts>),
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct ClaudeProjectContext {
    workspace: ProjectWorkspace,
    pub(super) dependency: ClaudeProjectDependency,
}

impl ClaudeProjectContext {
    fn apply(&self, messages: &mut [UsageRecord]) {
        let ProjectWorkspace::Shared(workspace) = &self.workspace else {
            return;
        };
        for message in messages {
            if let Some(workspace) = workspace {
                set_message_workspace(message, workspace);
            } else {
                message.set_workspace(None, None);
            }
        }
    }
}

fn resolve_project_workspace(
    resolver: &ClaudeProjectResolver,
    project_key: Option<&str>,
    candidates: &ClaudeProjectCandidates,
    input_path: &Path,
    parent_session_id: Option<&str>,
) -> ClaudeProjectContext {
    let Some(project_key) = project_key else {
        return ClaudeProjectContext {
            workspace: ProjectWorkspace::PerRecord,
            dependency: ClaudeProjectDependency::None,
        };
    };
    let (resolution, dependency) =
        resolver.resolve(project_key, candidates, input_path, parent_session_id);
    let workspace = match resolution {
        ClaudeProjectResolution::Resolved(workspace) => Some(workspace),
        ClaudeProjectResolution::Unresolved | ClaudeProjectResolution::Ambiguous => {
            workspace_parts_from_key(project_key)
        }
    };
    ClaudeProjectContext {
        workspace: ProjectWorkspace::Shared(workspace),
        dependency,
    }
}

fn resolved_workspace(path: &str) -> ClaudeProjectResolution {
    workspace_parts_from_key(path)
        .map(ClaudeProjectResolution::Resolved)
        .unwrap_or(ClaudeProjectResolution::Unresolved)
}

fn match_project_candidate_tiers(
    project_key: &str,
    candidates: &ClaudeProjectCandidates,
) -> CandidateMatch {
    let project_path_match = match_project_candidates(
        project_key,
        candidates.project_paths.iter().map(String::as_str),
    );
    if !matches!(project_path_match, CandidateMatch::None) {
        return project_path_match;
    }

    let cwd_match =
        match_project_candidates(project_key, candidates.cwds.iter().map(String::as_str));
    if !matches!(cwd_match, CandidateMatch::None) {
        return cwd_match;
    }

    match_project_candidates(
        project_key,
        candidates
            .cwds
            .iter()
            .flat_map(|cwd| workspace_ancestor_candidates(cwd)),
    )
}

fn match_project_candidates<'a>(
    project_key: &str,
    candidates: impl IntoIterator<Item = &'a str>,
) -> CandidateMatch {
    let mut matches = HashSet::new();
    for candidate in candidates {
        let Some(normalized) = normalize_workspace_key(candidate) else {
            continue;
        };
        if claude_project_key(candidate) == project_key
            || claude_project_key(&normalized) == project_key
        {
            matches.insert(normalized);
            if matches.len() > 1 {
                return CandidateMatch::Ambiguous;
            }
        }
    }

    matches
        .into_iter()
        .next()
        .map(CandidateMatch::Unique)
        .unwrap_or(CandidateMatch::None)
}

fn workspace_ancestor_candidates(raw: &str) -> Vec<&str> {
    let trimmed = raw.trim().trim_end_matches(['/', '\\']);
    let mut ancestors = Vec::new();
    let mut end = trimmed.len();
    while let Some(index) = trimmed[..end].rfind(['/', '\\']) {
        if index == 0 {
            ancestors.push(&trimmed[..1]);
            break;
        }
        end = index;
        ancestors.push(&trimmed[..end]);
    }
    ancestors
}

/// Match Claude Code's project-directory encoding exactly. The replacement
/// and length limit operate on JavaScript UTF-16 code units.
fn claude_project_key(path: &str) -> String {
    let utf16: Vec<u16> = path.encode_utf16().collect();
    let mut encoded = String::with_capacity(utf16.len());
    for code_unit in &utf16 {
        if (*code_unit >= b'a' as u16 && *code_unit <= b'z' as u16)
            || (*code_unit >= b'A' as u16 && *code_unit <= b'Z' as u16)
            || (*code_unit >= b'0' as u16 && *code_unit <= b'9' as u16)
        {
            encoded.push(char::from_u32(*code_unit as u32).expect("ASCII code unit"));
        } else {
            encoded.push('-');
        }
    }

    if encoded.len() <= 200 {
        return encoded;
    }

    let hash = utf16.iter().fold(0_i32, |hash, code_unit| {
        hash.wrapping_mul(31).wrapping_add(*code_unit as i32)
    });
    format!(
        "{}-{}",
        &encoded[..200],
        unsigned_to_base36(i64::from(hash).unsigned_abs())
    )
}

fn unsigned_to_base36(mut value: u64) -> String {
    if value == 0 {
        return "0".to_string();
    }

    let mut digits = Vec::new();
    while value > 0 {
        let digit = (value % 36) as u8;
        digits.push(if digit < 10 {
            (b'0' + digit) as char
        } else {
            (b'a' + digit - 10) as char
        });
        value /= 36;
    }
    digits.into_iter().rev().collect()
}

fn read_external_project_candidates(home_dir: &Path) -> ClaudeProjectCandidates {
    let mut candidates = ClaudeProjectCandidates::default();
    let history_path = home_dir.join(".claude/history.jsonl");
    match std::fs::File::open(&history_path) {
        Ok(file) => {
            for (line_index, line) in BufReader::new(file).lines().enumerate() {
                let line = match line {
                    Ok(line) => line,
                    Err(error) => {
                        tracing::warn!(
                                code = "claude_project_history_unreadable",
                        input = %history_path.display(),
                                line = line_index + 1,
                                error = %error,
                                "could not read Claude history project metadata"
                            );
                        break;
                    }
                };
                match serde_json::from_str::<Value>(&line) {
                    Ok(value) => {
                        candidates.record(value.get("project").and_then(Value::as_str), None)
                    }
                    Err(error) => tracing::warn!(
                        code = "claude_project_history_invalid",
                    input = %history_path.display(),
                        line = line_index + 1,
                        error = %error,
                        "could not decode Claude history project metadata"
                    ),
                }
            }
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => tracing::warn!(
            code = "claude_project_history_unreadable",
                    input = %history_path.display(),
            error = %error,
            "could not open Claude history project metadata"
        ),
    }

    let config_path = home_dir.join(".claude.json");
    match std::fs::read(&config_path) {
        Ok(mut bytes) => match simd_json::from_slice::<Value>(&mut bytes) {
            Ok(value) => {
                if let Some(projects) = value.get("projects").and_then(Value::as_object) {
                    for project in projects.keys() {
                        candidates.record(Some(project), None);
                    }
                }
            }
            Err(error) => tracing::warn!(
                code = "claude_project_config_invalid",
                    input = %config_path.display(),
                error = %error,
                "could not decode Claude project configuration"
            ),
        },
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => tracing::warn!(
            code = "claude_project_config_unreadable",
                    input = %config_path.display(),
            error = %error,
            "could not read Claude project configuration"
        ),
    }

    candidates
}

fn parse_claude_entry_timestamp_checked(
    path: &Path,
    line_number: usize,
    timestamp: Option<&str>,
) -> SessionParseResult<Option<i64>> {
    match timestamp {
        None => Ok(None),
        Some(timestamp) => chrono::DateTime::parse_from_rfc3339(timestamp)
            .map(|parsed| Some(parsed.timestamp_millis()))
            .map_err(|source| {
                SessionParseError::at_path(
                    path,
                    "validate Claude session timestamp",
                    std::io::Error::new(
                        ErrorKind::InvalidData,
                        format!(
                            "line {line_number}: invalid RFC 3339 timestamp `{timestamp}`: {source}"
                        ),
                    ),
                )
            }),
    }
}

fn merge_claude_duplicate(
    existing: &mut UsageRecord,
    usage: &ClaudeUsage,
    parsed_timestamp: i64,
    provider_hint: Option<&str>,
) {
    existing
        .billing_provider
        .merge(&crate::BillingProvider::from_observation(provider_hint));
    super::merge::merge_input(
        &mut existing.tokens.input,
        &mut existing.claude_input_is_split,
        usage.input_tokens.unwrap_or(0),
        usage.reports_input_split(),
    );
    let t = &mut existing.tokens;
    t.output = t.output.max(usage.output_tokens.unwrap_or(0));
    t.cache_read = t.cache_read.max(usage.cache_read_input_tokens.unwrap_or(0));
    t.cache_write = t
        .cache_write
        .max(usage.cache_creation_input_tokens.unwrap_or(0));
    t.cache_write_1h = t.cache_write_1h.max(usage.cache_write_1h());

    if parsed_timestamp >= existing.timestamp {
        existing.set_timestamp(parsed_timestamp);
    }
}

fn is_claude_synthetic_placeholder_model(model: &str) -> bool {
    model.trim().eq_ignore_ascii_case("<synthetic>")
}

fn observed_claude_model(model: &str) -> String {
    model.trim().to_string()
}

/// Internal Claude Code system/tool tags that should NOT be counted as human turns.
/// User prompts containing arbitrary HTML/XML (e.g. `<div>hello</div>`) are still
/// counted, only this narrow allowlist is excluded.
const CLAUDECODE_INTERNAL_USER_TAGS: &[&str] = &[
    "<local-command-stdout>",
    "<local-command-stderr>",
    "<command-name>",
    "<command-message>",
    "<system-reminder>",
    "<bash-input>",
    "<bash-stdout>",
    "<bash-stderr>",
];

/// Returns true if a `type: "user"` JSONL entry is genuine human input (not tool results or system messages).
fn is_human_turn(raw_line: &str) -> bool {
    if let Some(pos) = raw_line.find("\"content\":") {
        let after = &raw_line[pos + 10..];
        let after_trimmed = after.trim_start();
        if after_trimmed.starts_with('[') {
            return false;
        }
        if let Some(content_start) = after_trimmed.strip_prefix('"') {
            // Only filter out content that begins with a known internal tag.
            // Anything else (including `<div>`, `<table>`, etc. in genuine prompts)
            // is treated as a real human turn.
            for tag in CLAUDECODE_INTERNAL_USER_TAGS {
                if content_start.starts_with(tag) {
                    return false;
                }
            }
            return true;
        }
    }
    false
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ClaudeProviderChoice {
    id: String,
    confidence: u8,
}

impl ClaudeProviderChoice {
    fn new(id: impl Into<String>, confidence: u8) -> Self {
        Self {
            id: id.into(),
            confidence,
        }
    }
}

const CLAUDE_PROVIDER_DEFAULT_CONFIDENCE: u8 = 1;
const CLAUDE_PROVIDER_INFERRED_CONFIDENCE: u8 = 2;
const CLAUDE_PROVIDER_EXPLICIT_CONFIDENCE: u8 = 3;
const CLAUDE_PROVIDER_MODEL_OVERRIDE_CONFIDENCE: u8 = 4;

fn claude_provider_choice_for_models(
    raw_model: &str,
    canonical_model: &str,
    provider_hint: Option<&str>,
) -> ClaudeProviderChoice {
    let raw_choice = claude_provider_choice(raw_model, provider_hint);
    if raw_choice.confidence > 0 {
        raw_choice
    } else {
        claude_provider_choice(canonical_model, provider_hint)
    }
}

fn claude_provider_choice_from_parts(
    model: Option<&str>,
    provider_hint: Option<&str>,
) -> Option<ClaudeProviderChoice> {
    match model {
        Some(model) => Some(claude_provider_choice(model, provider_hint)),
        None => claude_provider_choice_from_hint(None, provider_hint),
    }
}

fn claude_provider_choice(model: &str, provider_hint: Option<&str>) -> ClaudeProviderChoice {
    if let Some(choice) = claude_provider_choice_from_hint(Some(model), provider_hint) {
        return choice;
    }

    let inferred = provider_identity::inferred_provider_from_model(model);

    if let Some(provider) = provider_from_model_prefix(model) {
        return ClaudeProviderChoice::new(provider, CLAUDE_PROVIDER_EXPLICIT_CONFIDENCE);
    }

    if let Some(provider) = inferred {
        return ClaudeProviderChoice::new(provider, CLAUDE_PROVIDER_INFERRED_CONFIDENCE);
    }

    ClaudeProviderChoice::new("unknown", 0)
}

fn claude_provider_choice_from_hint(
    model: Option<&str>,
    provider_hint: Option<&str>,
) -> Option<ClaudeProviderChoice> {
    if let Some(provider) = model.and_then(provider_identity::provider_override_from_model) {
        return Some(ClaudeProviderChoice::new(
            provider,
            CLAUDE_PROVIDER_MODEL_OVERRIDE_CONFIDENCE,
        ));
    }

    let hint = provider_hint.and_then(provider_identity::canonical_provider)?;

    if hint == "anthropic" {
        if let Some(inferred_provider) =
            model.and_then(provider_identity::inferred_provider_from_model)
        {
            if inferred_provider != "anthropic" {
                return Some(ClaudeProviderChoice::new(
                    inferred_provider,
                    CLAUDE_PROVIDER_INFERRED_CONFIDENCE,
                ));
            }
        }
        if model.is_some_and(|model| !provider_identity::is_anthropic_model(model)) {
            return Some(ClaudeProviderChoice::new("unknown", 0));
        }
        return Some(ClaudeProviderChoice::new(
            hint,
            CLAUDE_PROVIDER_DEFAULT_CONFIDENCE,
        ));
    }

    Some(ClaudeProviderChoice::new(
        hint,
        CLAUDE_PROVIDER_EXPLICIT_CONFIDENCE,
    ))
}

fn update_claude_provider_id(
    existing: &mut std::sync::Arc<str>,
    existing_confidence: &mut u8,
    candidate: ClaudeProviderChoice,
) {
    if candidate.confidence > *existing_confidence {
        *existing_confidence = candidate.confidence;
        *existing = crate::records::intern::intern(&candidate.id);
    }
}

fn provider_from_model_prefix(model: &str) -> Option<String> {
    let provider = model
        .trim()
        .contains('/')
        .then(|| provider_identity::canonical_provider(model))
        .flatten()?;

    if provider == "anthropic" && !provider_identity::is_anthropic_model(model) {
        None
    } else {
        Some(provider)
    }
}

#[cfg(test)]
#[path = "tests/decode.rs"]
mod tests;

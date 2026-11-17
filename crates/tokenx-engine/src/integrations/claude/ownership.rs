//! Acquisition-local OpenCode ownership evidence; Claude shards remain raw.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use sha2::{Digest, Sha256};

use crate::input_health::{DataHealth, InputFailure, InputStatus, RejectionSummary};
use crate::integrations::{opencode, PreparedInput};

#[derive(Debug)]
pub(crate) struct OpenCodeOwnership {
    inputs: Vec<PreparedInput>,
    failures: Vec<(PathBuf, InputFailure)>,
    signature: [u8; 32],
    claims: OnceLock<OwnershipClaims>,
}

#[derive(Debug, Default)]
pub(super) struct OwnershipClaims {
    pub sessions: HashSet<String>,
    pub health: Vec<(PathBuf, InputStatus, RejectionSummary)>,
}

impl OpenCodeOwnership {
    pub(crate) fn from_prepared(inputs: &[PreparedInput], health: &DataHealth) -> Self {
        let failures: Vec<_> = health
            .inputs()
            .iter()
            .filter_map(|input| {
                input
                    .status
                    .failure()
                    .map(|failure| (input.path.clone(), failure.clone()))
            })
            .collect();
        let mut hasher = Sha256::new();
        crate::input_record_cache::hash_inventory_bytes(&mut hasher, b"claude-opencode-ownership");
        for input in inputs {
            hasher.update(input.inventory_signature_digest());
        }
        for (path, failure) in &failures {
            crate::input_record_cache::hash_inventory_path(&mut hasher, path);
            crate::input_record_cache::hash_inventory_bytes(
                &mut hasher,
                failure.operation.as_bytes(),
            );
        }
        Self {
            inputs: inputs.to_vec(),
            failures,
            signature: hasher.finalize().into(),
            claims: OnceLock::new(),
        }
    }

    pub(crate) fn signature(&self) -> &[u8; 32] {
        &self.signature
    }

    pub(crate) fn visit_present_files(
        &self,
        mut visit: impl FnMut(crate::input_record_cache::InputFileIdentity, u64),
    ) {
        for input in &self.inputs {
            input.snapshot().visit_present_files(&mut visit);
        }
    }

    pub(super) fn claims(&self, cancellation: &crate::AcquisitionCancellation) -> &OwnershipClaims {
        self.claims.get_or_init(|| {
            let mut result = OwnershipClaims::default();
            for (path, failure) in &self.failures {
                result.health.push((
                    path.clone(),
                    InputStatus::Unavailable {
                        failure: failure.clone(),
                    },
                    RejectionSummary::default(),
                ));
            }
            for input in &self.inputs {
                match read_claims(input, cancellation) {
                    Ok(claims) => {
                        result.sessions.extend(claims.sessions);
                        let status = match claims.interrupted {
                            Some(failure) => InputStatus::Partial { failure },
                            None => InputStatus::Complete,
                        };
                        if !claims.rejections.is_empty() || !matches!(status, InputStatus::Complete)
                        {
                            result
                                .health
                                .push((input.path.clone(), status, claims.rejections));
                        }
                    }
                    Err(failure) => result.health.push((
                        input.path.clone(),
                        InputStatus::Unavailable { failure },
                        RejectionSummary::default(),
                    )),
                }
            }
            result
        })
    }
}

fn read_claims(
    input: &PreparedInput,
    cancellation: &crate::AcquisitionCancellation,
) -> Result<opencode::claude_claims::Claims, InputFailure> {
    let claims = opencode::claude_claims::read(&input.path, cancellation)
        .map_err(|error| InputFailure::from(&error))?;
    let after = input.input_policy().snapshot().map_err(|error| {
        InputFailure::new("verify OpenCode ownership snapshot", error.to_string())
    })?;
    if &after != input.snapshot() {
        return Err(InputFailure::new(
            "verify OpenCode ownership snapshot",
            "database or WAL changed after acquisition was prepared",
        ));
    }
    Ok(claims)
}

pub(super) fn transcript_session_id(path: &Path) -> Option<&str> {
    let parent = path.parent()?;
    if parent.file_name().and_then(|name| name.to_str()) == Some("subagents") {
        parent.parent()?.file_name()?.to_str()
    } else {
        path.file_stem()?.to_str()
    }
}

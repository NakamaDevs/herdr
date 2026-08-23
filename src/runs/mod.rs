//! Server-owned durable run resource.
//!
//! A run is one authenticated, idempotent unit of agent work that an external
//! orchestrator submits once, recovers after a Herdr restart, and cancels by
//! id. The registry here is pure data plus a state machine: it never touches
//! PTYs, terminal snapshots, or presentation state. Binding a run to a concrete
//! pane is the caller's job, so this module stays testable without a runtime.
//!
//! Persisted records deliberately carry no terminal output, no ANSI text, and
//! no prompt body — only a digest, a length, and typed states.

pub mod auth;

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use self::auth::{Capability, CapabilityRef, RunOperation};

/// Maximum accepted caller idempotency key length, in characters.
pub const MAX_IDEMPOTENCY_KEY_LEN: usize = 128;
/// Maximum accepted prompt size, in bytes.
pub const MAX_PROMPT_BYTES: usize = 16 * 1024;
/// Maximum accepted checkout path length, in bytes.
pub const MAX_CHECKOUT_PATH_LEN: usize = 4096;
/// Maximum accepted workspace id length, in bytes.
pub const MAX_WORKSPACE_ID_LEN: usize = 64;
/// Maximum accepted submit target length, in bytes.
pub const MAX_TARGET_LEN: usize = 128;
/// Maximum retained run records. Oldest finished runs are dropped first.
pub const MAX_RUN_RECORDS: usize = 512;
/// Maximum retained private idempotency digests.
pub const MAX_RUN_DEDUPLICATION: usize = MAX_RUN_RECORDS;
/// Maximum serialized result metadata returned for one run.
pub const MAX_RUN_RESULT_BYTES: usize = 8 * 1024;
/// Durable registry format version.
pub const REGISTRY_VERSION: u32 = 1;

/// Lifecycle state of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    /// Accepted and bound, prompt not delivered yet.
    Queued,
    /// Prompt delivered to the bound agent.
    Running,
    /// Bound agent needs interactive input.
    Blocked,
    /// Bound agent returned to idle after doing work.
    Succeeded,
    /// The run could not be delivered or the bound agent went away mid-flight.
    Failed,
    /// Cancellation accepted and signalled, not observed as stopped yet.
    CancelRequested,
    /// Cancellation observed as stopped.
    Cancelled,
    /// Outcome is unknown because the binding disappeared.
    Lost,
}

impl RunState {
    /// True when no further transition is possible.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            RunState::Succeeded | RunState::Failed | RunState::Cancelled | RunState::Lost
        )
    }
}

/// Typed, bounded reason a run failed. Never carries free text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunFailureKind {
    /// The bound pane or agent was not usable at submit time.
    AgentUnavailable,
    /// The agent refused the prompt or the write failed.
    PromptRejected,
    /// The provider process exited before the run reached an observed outcome.
    ProviderExited,
    /// The bound checkout was not resolvable.
    CheckoutUnavailable,
    /// The binding did not survive a server restart.
    ServerRestart,
    /// The server could not complete the operation.
    Internal,
}

/// Request field that failed a bound or charset check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunInvalidField {
    IdempotencyKey,
    WorkspaceId,
    Checkout,
    Target,
    Prompt,
    Ttl,
    Operations,
    Sequence,
}

impl RunInvalidField {
    fn name(self) -> &'static str {
        match self {
            RunInvalidField::IdempotencyKey => "idempotency_key",
            RunInvalidField::WorkspaceId => "workspace_id",
            RunInvalidField::Checkout => "checkout",
            RunInvalidField::Target => "target",
            RunInvalidField::Prompt => "prompt",
            RunInvalidField::Ttl => "ttl_ms",
            RunInvalidField::Operations => "operations",
            RunInvalidField::Sequence => "sequence",
        }
    }
}

/// Every way a run operation can be refused. All variants fail closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunError {
    /// A bounded field was missing, oversized, or malformed.
    InvalidRequest(RunInvalidField),
    /// The idempotency key was reused for different work.
    IdempotencyConflict,
    /// The claimed checkout does not match the workspace checkout.
    CheckoutMismatch,
    /// The run does not exist, or the capability scope does not cover it.
    NotFound,
    /// The run already reached a terminal state.
    NotCancellable,
    /// The capability is unknown, expired, or out of scope.
    CapabilityInvalid,
    /// The capability sequence was already used.
    ReplayRejected,
    /// The socket peer is not the local owner.
    Unauthorized,
    /// The bound pane or agent cannot accept work.
    TargetUnavailable,
    /// Another active run already owns the full binding.
    BindingBusy,
    /// The registry has no terminal record that can make room for this run.
    CapacityReached,
    /// The durable registry could not be saved or loaded safely.
    PersistenceUnavailable,
}

impl RunError {
    /// Stable wire error code.
    pub fn code(self) -> &'static str {
        match self {
            RunError::InvalidRequest(_) => "run_invalid_request",
            RunError::IdempotencyConflict => "run_idempotency_conflict",
            RunError::CheckoutMismatch => "run_checkout_mismatch",
            RunError::NotFound => "run_not_found",
            RunError::NotCancellable => "run_not_cancellable",
            RunError::CapabilityInvalid => "run_capability_invalid",
            RunError::ReplayRejected => "run_replay_rejected",
            RunError::Unauthorized => "run_unauthorized",
            RunError::TargetUnavailable => "run_target_unavailable",
            RunError::BindingBusy => "run_binding_busy",
            RunError::CapacityReached => "run_capacity_reached",
            RunError::PersistenceUnavailable => "run_persistence_unavailable",
        }
    }

    /// Operator-facing message. Never includes prompt text or pane output.
    pub fn message(self) -> String {
        match self {
            RunError::InvalidRequest(field) => {
                format!(
                    "run request field {} is missing or out of bounds",
                    field.name()
                )
            }
            RunError::IdempotencyConflict => {
                "idempotency key was already used for different run input".to_string()
            }
            RunError::CheckoutMismatch => {
                "workspace checkout does not match the requested checkout".to_string()
            }
            RunError::NotFound => "run not found".to_string(),
            RunError::NotCancellable => "run already reached a terminal state".to_string(),
            RunError::CapabilityInvalid => {
                "run capability is unknown, expired, or out of scope".to_string()
            }
            RunError::ReplayRejected => "run capability sequence was already used".to_string(),
            RunError::Unauthorized => "run operations require local socket peer access".to_string(),
            RunError::TargetUnavailable => "run target is not an available agent".to_string(),
            RunError::BindingBusy => "run binding already has an active run".to_string(),
            RunError::CapacityReached => {
                "durable run registry reached its active run limit".to_string()
            }
            RunError::PersistenceUnavailable => "durable run registry is unavailable".to_string(),
        }
    }
}

/// Concrete binding produced by the caller before a run is stored.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RunBinding {
    pub workspace_id: String,
    pub checkout_path: String,
    pub pane_id: String,
    pub agent_name: Option<String>,
    pub agent_session_id: String,
}

/// Identity attached to an agent observation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RunObservationBinding {
    pub workspace_id: String,
    pub checkout_path: String,
    pub pane_id: String,
    pub agent_name: Option<String>,
    pub agent_session_id: String,
}

/// One submit request after the caller resolved its binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSubmission {
    pub idempotency_key: String,
    pub prompt: String,
    pub binding: RunBinding,
}

/// Authorized scope for a run operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunScope {
    pub workspace_id: String,
}

/// Durable run record. Serialized verbatim into the registry file and into the
/// API response, so every field here must stay free of terminal content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RunRecord {
    pub run_id: String,
    pub workspace_id: String,
    pub checkout_path: String,
    pub pane_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    pub agent_session_id: String,
    /// Lowercase hex sha256 of the submitted prompt.
    pub prompt_digest: String,
    pub prompt_bytes: u32,
    pub state: RunState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<RunFailureKind>,
    pub created_at_unix: u64,
    pub updated_at_unix: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_unix: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at_unix: Option<u64>,
    /// Set once the bound agent was observed doing work for this run.
    #[serde(default)]
    pub activity_seen: bool,
}

/// Private durable data used only to resolve an idempotent submit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RunDeduplication {
    run_id: String,
    workspace_id: String,
    idempotency_key_digest: String,
    prompt_digest: String,
    checkout_path: String,
    pane_id: String,
    agent_name: Option<String>,
    agent_session_id: String,
}

/// Result of an accepted submit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitOutcome {
    pub record: RunRecord,
    pub deduplicated: bool,
}

/// Agent facts the registry reacts to, mapped by the caller from detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunAgentObservation {
    Working,
    Blocked,
    Idle,
    Gone,
}

/// Durable, server-owned run and capability store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunRegistry {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    runs: Vec<RunRecord>,
    #[serde(default)]
    deduplication: Vec<RunDeduplication>,
    #[serde(default)]
    capabilities: Vec<Capability>,
    #[serde(default)]
    next_run_seq: u64,
    #[serde(default)]
    next_capability_seq: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunRegistryDisk {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    runs: Vec<RunRecord>,
    #[serde(default)]
    deduplication: Vec<RunDeduplication>,
    #[serde(default)]
    capabilities: Vec<Capability>,
    #[serde(default)]
    next_run_seq: u64,
    #[serde(default)]
    next_capability_seq: u64,
}

impl<'de> Deserialize<'de> for RunRegistry {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let disk = RunRegistryDisk::deserialize(deserializer)?;
        if disk.version != REGISTRY_VERSION || !valid_registry_disk(&disk) {
            return Err(serde::de::Error::custom("unsupported run registry version"));
        }
        Ok(Self {
            version: disk.version,
            runs: disk.runs,
            deduplication: disk.deduplication,
            capabilities: disk.capabilities,
            next_run_seq: disk.next_run_seq,
            next_capability_seq: disk.next_capability_seq,
        })
    }
}

fn valid_registry_disk(disk: &RunRegistryDisk) -> bool {
    if disk.deduplication.len() > MAX_RUN_DEDUPLICATION
        || disk.capabilities.len() > auth::MAX_CAPABILITIES
        || disk.runs.len() > MAX_RUN_RECORDS
        || disk.next_run_seq < disk.runs.len() as u64
        || disk.next_capability_seq < disk.capabilities.len() as u64
    {
        return false;
    }

    let run_ids: HashSet<_> = disk.runs.iter().map(|run| run.run_id.as_str()).collect();
    if run_ids.len() != disk.runs.len() || disk.runs.iter().any(|run| !valid_run_record(run)) {
        return false;
    }

    let dedup_keys: HashSet<_> = disk
        .deduplication
        .iter()
        .map(|entry| {
            (
                entry.workspace_id.as_str(),
                entry.idempotency_key_digest.as_str(),
            )
        })
        .collect();
    if dedup_keys.len() != disk.deduplication.len()
        || disk.deduplication.iter().any(|entry| {
            !valid_digest(&entry.idempotency_key_digest)
                || !disk.runs.iter().any(|run| {
                    run.run_id == entry.run_id
                        && run.workspace_id == entry.workspace_id
                        && run.prompt_digest == entry.prompt_digest
                        && run.checkout_path == entry.checkout_path
                        && run.pane_id == entry.pane_id
                        && run.agent_name == entry.agent_name
                        && run.agent_session_id == entry.agent_session_id
                })
        })
    {
        return false;
    }

    let capability_ids: HashSet<_> = disk
        .capabilities
        .iter()
        .map(|capability| capability.capability_id.as_str())
        .collect();
    capability_ids.len() == disk.capabilities.len()
        && disk.capabilities.iter().all(|capability| {
            bounded_identifier(&capability.capability_id, MAX_TARGET_LEN)
                && bounded_identifier(&capability.workspace_id, MAX_WORKSPACE_ID_LEN)
                && capability.issued_at_unix <= capability.expires_at_unix
                && !capability.operations.is_empty()
                && capability.operations.len() <= auth::MAX_CAPABILITY_OPERATIONS
                && capability.operations.iter().collect::<HashSet<_>>().len()
                    == capability.operations.len()
        })
}

fn valid_run_record(record: &RunRecord) -> bool {
    if !bounded_identifier(&record.run_id, MAX_TARGET_LEN)
        || !bounded_identifier(&record.workspace_id, MAX_WORKSPACE_ID_LEN)
        || !bounded_path(&record.checkout_path, MAX_CHECKOUT_PATH_LEN)
        || !bounded_identifier(&record.pane_id, MAX_TARGET_LEN)
        || !record
            .agent_name
            .as_deref()
            .is_some_and(|agent_name| bounded_identifier(agent_name, MAX_TARGET_LEN))
        || !bounded_identifier(&record.agent_session_id, MAX_TARGET_LEN)
        || !valid_digest(&record.prompt_digest)
        || record.created_at_unix > record.updated_at_unix
        || record.started_at_unix.is_some_and(|started| {
            started < record.created_at_unix || started > record.updated_at_unix
        })
        || record.finished_at_unix.is_some_and(|finished| {
            finished < record.updated_at_unix || finished < record.created_at_unix
        })
    {
        return false;
    }
    match record.state {
        RunState::Queued => record.started_at_unix.is_none() && record.finished_at_unix.is_none(),
        RunState::Running | RunState::Blocked => {
            record.started_at_unix.is_some() && record.finished_at_unix.is_none()
        }
        RunState::Succeeded | RunState::Failed | RunState::Cancelled | RunState::Lost => {
            record.finished_at_unix.is_some()
        }
        RunState::CancelRequested => record.finished_at_unix.is_none(),
    }
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

impl Default for RunRegistry {
    fn default() -> Self {
        Self {
            version: REGISTRY_VERSION,
            runs: Vec::new(),
            deduplication: Vec::new(),
            capabilities: Vec::new(),
            next_run_seq: 0,
            next_capability_seq: 0,
        }
    }
}

/// Lowercase hex sha256 of `value`.
pub fn digest_hex(value: &[u8]) -> String {
    use sha2::{Digest as _, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(value);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn bounded_identifier(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':'))
}

/// Checkout paths are not identifiers, so only length and control characters
/// are bounded here. Resolving the path against a real workspace is the
/// caller's job.
fn bounded_path(value: &str, max_len: usize) -> bool {
    !value.is_empty() && value.len() <= max_len && !value.chars().any(char::is_control)
}

/// Opaque run id. Derived from the registry sequence so it is unique and
/// carries no caller-supplied text.
fn run_id_for(sequence: u64, workspace_id: &str, idempotency_key: &str) -> String {
    let seed = format!("{sequence}\u{1f}{workspace_id}\u{1f}{idempotency_key}");
    let digest = digest_hex(seed.as_bytes());
    format!("run_{}", &digest[..32])
}

/// Apply one agent observation to a single active record.
///
/// Returns true when the record changed. Terminal records never change, and an
/// idle agent only completes a run once that run was observed doing work, so a
/// pane that was already idle at submit time cannot report a false success.
fn apply_observation(
    record: &mut RunRecord,
    observation: RunAgentObservation,
    now_unix: u64,
) -> bool {
    if record.state.is_terminal() || now_unix < record.updated_at_unix {
        return false;
    }
    match observation {
        RunAgentObservation::Working => {
            if record.state == RunState::Queued {
                return false;
            }
            record.activity_seen = true;
            if record.state == RunState::Blocked {
                record.state = RunState::Running;
            }
        }
        RunAgentObservation::Blocked => {
            if record.state == RunState::Queued {
                return false;
            }
            record.activity_seen = true;
            if record.state == RunState::Running {
                record.state = RunState::Blocked;
            }
        }
        RunAgentObservation::Idle => {
            if !record.activity_seen && record.state != RunState::CancelRequested {
                return false;
            }
            match record.state {
                RunState::Running | RunState::Blocked => record.state = RunState::Succeeded,
                RunState::CancelRequested => record.state = RunState::Cancelled,
                _ => return false,
            }
            record.finished_at_unix = Some(now_unix);
        }
        RunAgentObservation::Gone => {
            record.state = RunState::Lost;
            record.failure = Some(RunFailureKind::AgentUnavailable);
            record.finished_at_unix = Some(now_unix);
        }
    }
    record.updated_at_unix = now_unix;
    true
}

impl RunRegistry {
    /// Number of stored runs.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.runs.len()
    }

    /// True when no run is stored.
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    /// All stored runs, oldest first.
    #[cfg(test)]
    pub fn records(&self) -> &[RunRecord] {
        &self.runs
    }

    /// Issue a capability scoped to one workspace and operation set.
    pub fn issue_capability(
        &mut self,
        workspace_id: &str,
        ttl_ms: u64,
        operations: &[RunOperation],
        now_unix: u64,
    ) -> Result<Capability, RunError> {
        if !bounded_identifier(workspace_id, MAX_WORKSPACE_ID_LEN) {
            return Err(RunError::InvalidRequest(RunInvalidField::WorkspaceId));
        }
        if !(auth::MIN_CAPABILITY_TTL_MS..=auth::MAX_CAPABILITY_TTL_MS).contains(&ttl_ms) {
            return Err(RunError::InvalidRequest(RunInvalidField::Ttl));
        }
        if operations.is_empty()
            || operations.len() > auth::MAX_CAPABILITY_OPERATIONS
            || operations.iter().collect::<HashSet<_>>().len() != operations.len()
        {
            return Err(RunError::InvalidRequest(RunInvalidField::Operations));
        }
        self.capabilities.retain(|cap| !cap.is_expired(now_unix));
        self.next_capability_seq = self.next_capability_seq.saturating_add(1);
        let capability = Capability {
            capability_id: format!(
                "cap_{}",
                &digest_hex(format!("{}:{workspace_id}", self.next_capability_seq).as_bytes())
                    [..24]
            ),
            workspace_id: workspace_id.to_string(),
            operations: operations.to_vec(),
            issued_at_unix: now_unix,
            expires_at_unix: now_unix.saturating_add(ttl_ms / 1000),
            last_sequence: 0,
        };
        self.capabilities.push(capability.clone());
        if self.capabilities.len() > auth::MAX_CAPABILITIES {
            self.capabilities.remove(0);
        }
        Ok(capability)
    }

    /// Validate a capability reference and burn its sequence.
    ///
    /// The sequence is committed on every fully validated reference, including
    /// when the operation itself later fails, so a captured request can never
    /// be replayed.
    pub fn authorize(
        &mut self,
        capability: &CapabilityRef,
        operation: RunOperation,
        now_unix: u64,
    ) -> Result<RunScope, RunError> {
        if capability.sequence == 0 {
            return Err(RunError::InvalidRequest(RunInvalidField::Sequence));
        }
        let Some(cap) = self
            .capabilities
            .iter_mut()
            .find(|cap| cap.capability_id == capability.capability_id)
        else {
            return Err(RunError::CapabilityInvalid);
        };
        if cap.is_expired(now_unix) || !cap.allows(operation) {
            return Err(RunError::CapabilityInvalid);
        }
        if capability.sequence <= cap.last_sequence {
            return Err(RunError::ReplayRejected);
        }
        cap.last_sequence = capability.sequence;
        Ok(RunScope {
            workspace_id: cap.workspace_id.clone(),
        })
    }

    /// Store a new run, or return the original record for a repeated key.
    ///
    /// Every bound is checked before any state is touched, so a rejected
    /// submission can never reach persisted state.
    pub fn submit(
        &mut self,
        submission: &RunSubmission,
        now_unix: u64,
    ) -> Result<SubmitOutcome, RunError> {
        let binding = &submission.binding;
        if !bounded_identifier(&submission.idempotency_key, MAX_IDEMPOTENCY_KEY_LEN) {
            return Err(RunError::InvalidRequest(RunInvalidField::IdempotencyKey));
        }
        if !bounded_identifier(&binding.workspace_id, MAX_WORKSPACE_ID_LEN) {
            return Err(RunError::InvalidRequest(RunInvalidField::WorkspaceId));
        }
        if !bounded_path(&binding.checkout_path, MAX_CHECKOUT_PATH_LEN) {
            return Err(RunError::InvalidRequest(RunInvalidField::Checkout));
        }
        if !bounded_identifier(&binding.pane_id, MAX_TARGET_LEN) {
            return Err(RunError::InvalidRequest(RunInvalidField::Target));
        }
        if let Some(agent_name) = binding.agent_name.as_deref() {
            if !bounded_identifier(agent_name, MAX_TARGET_LEN) {
                return Err(RunError::InvalidRequest(RunInvalidField::Target));
            }
        }
        if !bounded_identifier(&binding.agent_session_id, MAX_TARGET_LEN) {
            return Err(RunError::InvalidRequest(RunInvalidField::Target));
        }
        if submission.prompt.is_empty() || submission.prompt.len() > MAX_PROMPT_BYTES {
            return Err(RunError::InvalidRequest(RunInvalidField::Prompt));
        }

        let prompt_digest = digest_hex(submission.prompt.as_bytes());
        let idempotency_key_digest = digest_hex(submission.idempotency_key.as_bytes());
        if let Some(existing) = self.deduplication.iter().find(|entry| {
            entry.workspace_id == binding.workspace_id
                && entry.idempotency_key_digest == idempotency_key_digest
        }) {
            let same_work = existing.prompt_digest == prompt_digest
                && existing.checkout_path == binding.checkout_path
                && existing.pane_id == binding.pane_id
                && existing.agent_name == binding.agent_name
                && existing.agent_session_id == binding.agent_session_id;
            return if same_work {
                let Some(record) = self
                    .runs
                    .iter()
                    .find(|record| record.run_id == existing.run_id)
                else {
                    return Err(RunError::IdempotencyConflict);
                };
                Ok(SubmitOutcome {
                    record: record.clone(),
                    deduplicated: true,
                })
            } else {
                Err(RunError::IdempotencyConflict)
            };
        }

        if self.runs.iter().any(|record| {
            !record.state.is_terminal()
                && record.workspace_id == binding.workspace_id
                && record.checkout_path == binding.checkout_path
                && record.pane_id == binding.pane_id
                && record.agent_name == binding.agent_name
                && record.agent_session_id == binding.agent_session_id
        }) {
            return Err(RunError::BindingBusy);
        }

        if self.runs.len() >= MAX_RUN_RECORDS {
            self.trim_terminal_records_to(MAX_RUN_RECORDS - 1);
            if self.runs.len() >= MAX_RUN_RECORDS {
                return Err(RunError::CapacityReached);
            }
        }

        self.next_run_seq = self.next_run_seq.saturating_add(1);
        let record = RunRecord {
            run_id: run_id_for(
                self.next_run_seq,
                &binding.workspace_id,
                &submission.idempotency_key,
            ),
            workspace_id: binding.workspace_id.clone(),
            checkout_path: binding.checkout_path.clone(),
            pane_id: binding.pane_id.clone(),
            agent_name: binding.agent_name.clone(),
            agent_session_id: binding.agent_session_id.clone(),
            prompt_digest,
            prompt_bytes: submission.prompt.len() as u32,
            state: RunState::Queued,
            failure: None,
            created_at_unix: now_unix,
            updated_at_unix: now_unix,
            started_at_unix: None,
            finished_at_unix: None,
            activity_seen: false,
        };
        self.runs.push(record.clone());
        self.deduplication.push(RunDeduplication {
            run_id: record.run_id.clone(),
            workspace_id: record.workspace_id.clone(),
            idempotency_key_digest,
            prompt_digest: record.prompt_digest.clone(),
            checkout_path: record.checkout_path.clone(),
            pane_id: record.pane_id.clone(),
            agent_name: record.agent_name.clone(),
            agent_session_id: record.agent_session_id.clone(),
        });
        self.enforce_retention();
        Ok(SubmitOutcome {
            record,
            deduplicated: false,
        })
    }

    /// Drop the oldest finished runs once the retention bound is exceeded.
    ///
    /// Active runs are never evicted; a caller that floods the registry with
    /// finished runs must not be able to make a live run unobservable.
    fn enforce_retention(&mut self) {
        self.trim_terminal_records_to(MAX_RUN_RECORDS);
    }

    /// Drop terminal records until `maximum` records remain, if possible.
    fn trim_terminal_records_to(&mut self, maximum: usize) {
        while self.runs.len() > maximum {
            let Some(index) = self
                .runs
                .iter()
                .position(|record| record.state.is_terminal())
            else {
                break;
            };
            self.runs.remove(index);
        }
        self.deduplication
            .retain(|entry| self.runs.iter().any(|run| run.run_id == entry.run_id));
        if self.deduplication.len() > MAX_RUN_DEDUPLICATION {
            self.deduplication
                .drain(0..self.deduplication.len() - MAX_RUN_DEDUPLICATION);
        }
    }

    /// Read one run inside `scope`.
    pub fn get(&self, run_id: &str, scope: &RunScope) -> Result<&RunRecord, RunError> {
        self.runs
            .iter()
            .find(|record| record.run_id == run_id && record.workspace_id == scope.workspace_id)
            .ok_or(RunError::NotFound)
    }

    /// Move a queued run to running after successful prompt delivery.
    pub fn mark_started(&mut self, run_id: &str, now_unix: u64) -> Option<RunRecord> {
        let record = self
            .runs
            .iter_mut()
            .find(|record| record.run_id == run_id)?;
        if record.state != RunState::Queued || now_unix < record.created_at_unix {
            return None;
        }
        record.state = RunState::Running;
        record.started_at_unix = Some(now_unix);
        record.updated_at_unix = now_unix;
        Some(record.clone())
    }

    /// Move a run to failed with a typed reason.
    pub fn mark_failed(
        &mut self,
        run_id: &str,
        failure: RunFailureKind,
        now_unix: u64,
    ) -> Option<RunRecord> {
        let record = self
            .runs
            .iter_mut()
            .find(|record| record.run_id == run_id)?;
        if record.state.is_terminal() || now_unix < record.updated_at_unix {
            return None;
        }
        record.state = RunState::Failed;
        record.failure = Some(failure);
        record.finished_at_unix = Some(now_unix);
        record.updated_at_unix = now_unix;
        Some(record.clone())
    }

    /// Accept a cancel request for exactly one run inside `scope`.
    pub fn request_cancel(
        &mut self,
        run_id: &str,
        scope: &RunScope,
        now_unix: u64,
    ) -> Result<RunRecord, RunError> {
        let record = self
            .runs
            .iter_mut()
            .find(|record| record.run_id == run_id && record.workspace_id == scope.workspace_id)
            .ok_or(RunError::NotFound)?;
        if record.state.is_terminal() || now_unix < record.updated_at_unix {
            return Err(RunError::NotCancellable);
        }
        record.state = RunState::CancelRequested;
        record.updated_at_unix = now_unix;
        Ok(record.clone())
    }

    /// Advance exactly one active run matched by the full agent binding.
    ///
    /// Returns true when any record changed.
    pub fn observe_agent_state(
        &mut self,
        binding: &RunObservationBinding,
        observation: RunAgentObservation,
        now_unix: u64,
    ) -> bool {
        let mut changed = false;
        for record in self.runs.iter_mut().filter(|record| {
            record.workspace_id == binding.workspace_id
                && record.checkout_path == binding.checkout_path
                && record.pane_id == binding.pane_id
                && record.agent_name == binding.agent_name
                && record.agent_session_id == binding.agent_session_id
        }) {
            if apply_observation(record, observation, now_unix) {
                changed = true;
            }
        }
        changed
    }

    /// Reconcile only records whose complete live binding survived a restart.
    ///
    /// This safe placeholder rejects restoration until the durable registry
    /// owns complete runtime binding discovery.
    pub fn reconcile_after_restart_with_bindings(
        &mut self,
        live_bindings: &HashSet<RunObservationBinding>,
        now_unix: u64,
    ) -> usize {
        let mut lost = 0;
        for record in &mut self.runs {
            let binding = RunObservationBinding {
                workspace_id: record.workspace_id.clone(),
                checkout_path: record.checkout_path.clone(),
                pane_id: record.pane_id.clone(),
                agent_name: record.agent_name.clone(),
                agent_session_id: record.agent_session_id.clone(),
            };
            if !record.state.is_terminal() && !live_bindings.contains(&binding) {
                record.state = RunState::Lost;
                record.failure = Some(RunFailureKind::ServerRestart);
                record.finished_at_unix = Some(now_unix);
                record.updated_at_unix = now_unix;
                lost += 1;
            }
        }
        if lost > 0 {
            self.enforce_retention();
        }
        lost
    }

    /// Mark active runs lost when their workspace closes.
    pub fn mark_lost_for_closed_workspace(
        &mut self,
        workspace_id: &str,
        now_unix: u64,
    ) -> Vec<String> {
        self.mark_lost_matching(now_unix, |record| record.workspace_id == workspace_id)
    }

    /// Mark active runs lost when their bound pane closes.
    pub fn mark_lost_for_closed_pane(
        &mut self,
        workspace_id: &str,
        pane_id: &str,
        now_unix: u64,
    ) -> Vec<String> {
        self.mark_lost_matching(now_unix, |record| {
            record.workspace_id == workspace_id && record.pane_id == pane_id
        })
    }

    fn mark_lost_matching(
        &mut self,
        now_unix: u64,
        matches: impl Fn(&RunRecord) -> bool,
    ) -> Vec<String> {
        let mut lost = Vec::new();
        for record in &mut self.runs {
            if record.state.is_terminal() || now_unix < record.updated_at_unix || !matches(record) {
                continue;
            }
            record.state = RunState::Lost;
            record.failure = Some(RunFailureKind::AgentUnavailable);
            record.finished_at_unix = Some(now_unix);
            record.updated_at_unix = now_unix;
            lost.push(record.run_id.clone());
        }
        if !lost.is_empty() {
            self.enforce_retention();
        }
        lost
    }

    /// Legacy pane-only adapter. New callers must use `observe_agent_state`.
    #[cfg(test)]
    fn observe_agent_state_for_pane(
        &mut self,
        pane_id: &str,
        observation: RunAgentObservation,
        now_unix: u64,
    ) -> bool {
        let Some(record) = self.runs.iter().find(|record| record.pane_id == pane_id) else {
            return false;
        };
        self.observe_agent_state(
            &RunObservationBinding {
                workspace_id: record.workspace_id.clone(),
                checkout_path: record.checkout_path.clone(),
                pane_id: record.pane_id.clone(),
                agent_name: record.agent_name.clone(),
                agent_session_id: record.agent_session_id.clone(),
            },
            observation,
            now_unix,
        )
    }

    /// Fail closed for active runs whose binding did not survive a restart.
    ///
    /// Returns the number of records moved to `lost`.
    #[cfg(test)]
    pub fn reconcile_after_restart(
        &mut self,
        live_pane_ids: &HashSet<String>,
        now_unix: u64,
    ) -> usize {
        let mut lost = 0;
        for record in self.runs.iter_mut() {
            if record.state.is_terminal() || live_pane_ids.contains(&record.pane_id) {
                continue;
            }
            record.state = RunState::Lost;
            record.failure = Some(RunFailureKind::ServerRestart);
            record.finished_at_unix = Some(now_unix);
            record.updated_at_unix = now_unix;
            lost += 1;
        }
        lost
    }
}

#[cfg(test)]
mod tests;

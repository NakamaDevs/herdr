//! Scoped, expiring capabilities for run operations.
//!
//! A capability is an authorization object, not a bearer secret. The api socket
//! is already owner-only (`0600` on unix, an owner/SYSTEM SDDL on the Windows
//! named pipe), and run methods additionally assert socket peer authority, so
//! the capability's job is to bound *what* an authorized peer may do and for
//! *how long*, and to reject replayed requests.

use serde::{Deserialize, Serialize};

/// Shortest accepted capability lifetime.
pub const MIN_CAPABILITY_TTL_MS: u64 = 1_000;
/// Longest accepted capability lifetime.
pub const MAX_CAPABILITY_TTL_MS: u64 = 12 * 60 * 60 * 1_000;
/// Maximum retained capabilities. Expired entries are pruned before this bound
/// is applied, and the oldest live entry is dropped when it is still exceeded.
pub const MAX_CAPABILITIES: usize = 64;
/// A capability can name each supported operation once.
pub const MAX_CAPABILITY_OPERATIONS: usize = 3;

/// Run operations a capability can authorize.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunOperation {
    Submit,
    Status,
    Cancel,
}

/// Durable capability record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Capability {
    pub capability_id: String,
    pub workspace_id: String,
    pub operations: Vec<RunOperation>,
    pub issued_at_unix: u64,
    pub expires_at_unix: u64,
    /// Highest sequence already consumed. A reference must exceed it.
    #[serde(default)]
    pub last_sequence: u64,
}

impl Capability {
    /// True when `now_unix` is at or past the absolute expiry.
    pub fn is_expired(&self, now_unix: u64) -> bool {
        now_unix >= self.expires_at_unix
    }

    /// True when this capability authorizes `operation`.
    pub fn allows(&self, operation: RunOperation) -> bool {
        self.operations.contains(&operation)
    }
}

/// Caller-supplied reference to a capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CapabilityRef {
    pub capability_id: String,
    /// Strictly increasing per capability. Reuse is rejected as a replay.
    pub sequence: u64,
}

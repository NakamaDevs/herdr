use serde::{Deserialize, Serialize};

use crate::runs::auth::RunOperation;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunCapabilityRef {
    pub capability_id: String,
    pub sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunCapabilityIssueParams {
    pub workspace_id: String,
    pub ttl_ms: u64,
    pub operations: Vec<RunOperation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunCheckout {
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunTarget {
    pub pane_id: String,
    pub agent_name: String,
    pub agent_session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunSubmitParams {
    pub capability: RunCapabilityRef,
    pub idempotency_key: String,
    pub workspace_id: String,
    pub checkout: RunCheckout,
    pub target: RunTarget,
    pub prompt: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunStatusParams {
    pub capability: RunCapabilityRef,
    pub run_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunCancelParams {
    pub capability: RunCapabilityRef,
    pub run_id: String,
}

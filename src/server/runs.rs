//! Durable-run API binding checks.
//!
//! This module owns the boundary from API values to server-owned run bindings.

use bytes::Bytes;

#[cfg(test)]
use std::cell::RefCell;
#[cfg(test)]
use std::collections::VecDeque;

use crate::api::schema::{
    ResponseResult, RunCancelParams, RunCapabilityIssueParams, RunStatusParams, RunSubmitParams,
};
use crate::runs::{
    auth::{CapabilityRef, RunOperation},
    RunAgentObservation, RunBinding, RunError, RunFailureKind, RunObservationBinding, RunRecord,
    RunRegistry, RunState, RunSubmission,
};

use crate::api::responses::{encode_error, encode_success};

const MAX_RUN_REQUEST_ID_BYTES: usize = 128;
const INVALID_RUN_REQUEST_ID: &str = "invalid-run-request-id";
pub(crate) const AGENT_PROMPT_SUBMIT_DELAY: std::time::Duration =
    std::time::Duration::from_millis(300);

#[cfg(test)]
thread_local! {
    static TEST_RUN_CLOCK: RefCell<VecDeque<u64>> = const { RefCell::new(VecDeque::new()) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST: AtomicU64 = AtomicU64::new(0);

    struct Host {
        runtime: crate::terminal::TerminalRuntime,
        target: LiveRunTarget,
        observation: Cell<RunAgentObservation>,
    }

    impl RunHost for Host {
        fn has_workspace(&self, workspace_id: &str) -> bool {
            workspace_id == self.target.binding.workspace_id
        }
        fn live_run_target(
            &self,
            requested: &RequestedRunBinding,
        ) -> Result<LiveRunTarget, RunError> {
            validate_run_binding(requested, &self.target.binding)?;
            Ok(self.target.clone())
        }
        fn run_observation(&self, _: &RunRecord) -> Option<RunAgentObservation> {
            Some(self.observation.get())
        }
        fn lookup_runtime_sender(
            &self,
            _: usize,
            _: crate::layout::PaneId,
        ) -> Option<&crate::terminal::TerminalRuntime> {
            Some(&self.runtime)
        }
        fn target_ready_for_submit(&self, _: &LiveRunTarget) -> bool {
            self.observation.get() == RunAgentObservation::Idle
        }
    }

    #[tokio::test]
    async fn run_service_owns_lifecycle_and_recovery_without_a_tui() {
        let directory = std::env::temp_dir().join(format!(
            "herdr-run-service-{}-{}",
            std::process::id(),
            NEXT_TEST.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&directory).expect("test directory");
        let path = directory.join("runs.json");
        let (runtime, mut receiver) = crate::terminal::TerminalRuntime::test_with_channel(80, 24);
        let host = Host {
            runtime,
            target: LiveRunTarget {
                binding: ResolvedRunBinding {
                    workspace_id: "w1".into(),
                    checkout_path: "/repo".into(),
                    pane_id: "w1:p1".into(),
                    agent_name: "reviewer".into(),
                    agent_session_id: "s1".into(),
                },
                workspace_index: 0,
                pane_id: crate::layout::PaneId::alloc(),
            },
            observation: Cell::new(RunAgentObservation::Idle),
        };
        let mut service = RunService::load_from_path(path.clone());
        service.reconcile_after_startup(&std::collections::HashSet::new());
        let issued: serde_json::Value = serde_json::from_str(&service.handle_run_capability_issue(
            "issue".into(),
            RunCapabilityIssueParams {
                workspace_id: "w1".into(),
                ttl_ms: 60_000,
                operations: vec![RunOperation::Submit, RunOperation::Status],
            },
            &host,
        ))
        .expect("response");
        let capability = crate::api::schema::RunCapabilityRef {
            capability_id: issued["result"]["capability"]["capability_id"]
                .as_str()
                .expect("id")
                .into(),
            sequence: 1,
        };
        let submitted: serde_json::Value = serde_json::from_str(&service.handle_run_submit(
            "submit".into(),
            RunSubmitParams {
                capability: capability.clone(),
                idempotency_key: "k1".into(),
                workspace_id: "w1".into(),
                checkout: crate::api::schema::RunCheckout {
                    path: "/repo".into(),
                },
                target: crate::api::schema::RunTarget {
                    pane_id: "w1:p1".into(),
                    agent_name: "reviewer".into(),
                    agent_session_id: "s1".into(),
                },
                prompt: "review".into(),
            },
            &host,
        ))
        .expect("response");
        let run_id = submitted["result"]["run"]["run_id"]
            .as_str()
            .expect("id")
            .to_string();
        assert_eq!(
            receiver.recv().await.expect("prompt"),
            Bytes::from_static(b"review")
        );
        assert_eq!(
            receiver.recv().await.expect("enter"),
            Bytes::from_static(b"\r")
        );
        host.observation.set(RunAgentObservation::Working);
        service.handle_run_status(
            "working".into(),
            RunStatusParams {
                capability: crate::api::schema::RunCapabilityRef {
                    sequence: 2,
                    ..capability.clone()
                },
                run_id: run_id.clone(),
            },
            &host,
        );
        host.observation.set(RunAgentObservation::Idle);
        let completed: serde_json::Value = serde_json::from_str(&service.handle_run_status(
            "done".into(),
            RunStatusParams {
                capability: crate::api::schema::RunCapabilityRef {
                    sequence: 3,
                    ..capability
                },
                run_id,
            },
            &host,
        ))
        .expect("response");
        assert_eq!(completed["result"]["run"]["state"], "succeeded");
        let mut restored = RunService::load_from_path(path);
        restored.reconcile_after_startup(&std::collections::HashSet::new());
        assert_eq!(restored.run_registry, service.run_registry);
        std::fs::remove_dir_all(directory).expect("remove test directory");
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveRunTarget {
    pub(crate) binding: ResolvedRunBinding,
    pub(crate) workspace_index: usize,
    pub(crate) pane_id: crate::layout::PaneId,
}

/// Full identity resolved from the current workspace and pane state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedRunBinding {
    pub workspace_id: String,
    pub checkout_path: String,
    pub pane_id: String,
    pub agent_name: String,
    pub agent_session_id: String,
}

/// Full identity supplied by a durable-run submit request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RequestedRunBinding {
    pub workspace_id: String,
    pub checkout_path: String,
    pub pane_id: String,
    pub agent_name: String,
    pub agent_session_id: String,
}

/// Validate that a caller targets the exact live workspace and agent binding.
pub(crate) fn validate_run_binding(
    requested: &RequestedRunBinding,
    resolved: &ResolvedRunBinding,
) -> Result<(), RunError> {
    if requested.workspace_id != resolved.workspace_id
        || requested.checkout_path != resolved.checkout_path
    {
        return Err(RunError::CheckoutMismatch);
    }
    if requested.pane_id != resolved.pane_id
        || requested.agent_name != resolved.agent_name
        || requested.agent_session_id != resolved.agent_session_id
    {
        return Err(RunError::TargetUnavailable);
    }
    Ok(())
}

/// Runtime facts and terminal access required by the run service.
/// Implementations supply current identities without owning durable run policy.
pub(crate) trait RunHost {
    fn has_workspace(&self, workspace_id: &str) -> bool;
    fn live_run_target(&self, requested: &RequestedRunBinding) -> Result<LiveRunTarget, RunError>;
    fn run_observation(&self, record: &RunRecord) -> Option<RunAgentObservation>;
    fn lookup_runtime_sender(
        &self,
        workspace_index: usize,
        pane_id: crate::layout::PaneId,
    ) -> Option<&crate::terminal::TerminalRuntime>;
    fn target_ready_for_submit(&self, target: &LiveRunTarget) -> bool;
}

/// Owns durable runs, capabilities, persistence failures, and reconciliation.
/// The combined event loop holds this service while the TUI supplies only runtime facts.
#[derive(Default)]
pub(crate) struct RunService {
    pub(crate) run_registry: RunRegistry,
    pub(crate) run_registry_path: Option<std::path::PathBuf>,
    pub(crate) run_registry_load_error: Option<String>,
}

impl RunService {
    pub(crate) fn load(no_session: bool) -> Self {
        if no_session || cfg!(test) {
            return Self::default();
        }
        Self::load_from_path(crate::persist::run_registry::session_path())
    }

    pub(crate) fn load_from_path(path: std::path::PathBuf) -> Self {
        let mut service = Self {
            run_registry_path: Some(path.clone()),
            ..Self::default()
        };
        if path.exists() {
            match crate::persist::run_registry::load_from_path(&path) {
                Ok(registry) => service.run_registry = registry,
                Err(_) => {
                    service.run_registry_load_error =
                        Some("durable run registry is unavailable".to_string())
                }
            }
        }
        service
    }

    pub(crate) fn reconcile_after_startup(
        &mut self,
        live_bindings: &std::collections::HashSet<RunObservationBinding>,
    ) {
        if self.run_registry_load_error.is_some() || self.run_registry_path.is_none() {
            return;
        }
        let mut next = self.run_registry.clone();
        if next.reconcile_after_restart_with_bindings(live_bindings, Self::run_now_unix()) == 0 {
            return;
        }
        if self.persist_run_registry(next).is_err() {
            self.run_registry_load_error = Some("durable run registry is unavailable".to_string());
        }
    }

    pub(crate) fn run_now_unix() -> u64 {
        Self::run_now_unix_ms() / 1000
    }

    fn run_now_unix_ms() -> u64 {
        #[cfg(test)]
        if let Some(now_unix) = TEST_RUN_CLOCK.with(|clock| clock.borrow_mut().pop_front()) {
            return now_unix.saturating_mul(1000);
        }
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0)
    }

    #[cfg(test)]
    pub(crate) fn set_run_clock_for_test(values: impl IntoIterator<Item = u64>) {
        TEST_RUN_CLOCK.with(|clock| {
            *clock.borrow_mut() = values.into_iter().collect();
        });
    }

    fn bounded_run_request_id(id: String) -> String {
        if id.len() <= MAX_RUN_REQUEST_ID_BYTES {
            id
        } else {
            INVALID_RUN_REQUEST_ID.to_string()
        }
    }

    fn run_error(id: String, error: RunError) -> String {
        let id = Self::bounded_run_request_id(id);
        encode_error(id, error.code(), error.message())
    }

    fn run_success(id: String, result: ResponseResult) -> String {
        let id = Self::bounded_run_request_id(id);
        let response = encode_success(id.clone(), result);
        if response.len() <= crate::runs::MAX_RUN_RESULT_BYTES {
            response
        } else {
            encode_error(
                id,
                "run_invalid_request",
                "run response exceeds supported bounds",
            )
        }
    }

    fn persist_run_registry(&mut self, next: RunRegistry) -> Result<(), RunError> {
        if self.run_registry_load_error.is_some() {
            return Err(RunError::PersistenceUnavailable);
        }
        let Some(path) = self.run_registry_path.clone() else {
            return Err(RunError::PersistenceUnavailable);
        };
        crate::persist::run_registry::save_to_path(&path, &next)
            .map_err(|_| RunError::PersistenceUnavailable)?;
        self.run_registry = next;
        Ok(())
    }

    fn mutate_run_registry<T>(
        &mut self,
        mutation: impl FnOnce(&mut RunRegistry) -> Result<T, RunError>,
    ) -> Result<T, RunError> {
        let mut next = self.run_registry.clone();
        let result = mutation(&mut next);
        if next != self.run_registry {
            self.persist_run_registry(next)?;
        } else if self.run_registry_load_error.is_some() || self.run_registry_path.is_none() {
            return Err(RunError::PersistenceUnavailable);
        }
        result
    }

    pub(crate) fn record_requested_binding(
        record: &RunRecord,
    ) -> Result<RequestedRunBinding, RunError> {
        let Some(agent_name) = record.agent_name.clone() else {
            return Err(RunError::TargetUnavailable);
        };
        Ok(RequestedRunBinding {
            workspace_id: record.workspace_id.clone(),
            checkout_path: record.checkout_path.clone(),
            pane_id: record.pane_id.clone(),
            agent_name,
            agent_session_id: record.agent_session_id.clone(),
        })
    }

    fn observe_run_or_mark_lost(
        &self,
        host: &impl RunHost,
        registry: &mut RunRegistry,
        record: &RunRecord,
        now_unix: u64,
    ) {
        let requested = match Self::record_requested_binding(record) {
            Ok(requested) => requested,
            Err(_) => return,
        };
        let observation = match host.run_observation(record) {
            Some(observation) => observation,
            None if host.live_run_target(&requested).is_err() => RunAgentObservation::Gone,
            None => return,
        };
        let binding = RunObservationBinding {
            workspace_id: record.workspace_id.clone(),
            checkout_path: record.checkout_path.clone(),
            pane_id: record.pane_id.clone(),
            agent_name: record.agent_name.clone(),
            agent_session_id: record.agent_session_id.clone(),
        };
        if observation == RunAgentObservation::Working && record.state == RunState::Queued {
            let _ = registry.mark_started(&record.run_id, now_unix);
        }
        let _ = registry.observe_agent_state(&binding, observation, now_unix);
    }

    pub(crate) fn handle_run_capability_issue(
        &mut self,
        id: String,
        params: RunCapabilityIssueParams,
        host: &impl RunHost,
    ) -> String {
        if !host.has_workspace(&params.workspace_id) {
            return Self::run_error(id, RunError::NotFound);
        }
        match self.mutate_run_registry(|registry| {
            registry.issue_capability_at_millis(
                &params.workspace_id,
                params.ttl_ms,
                &params.operations,
                Self::run_now_unix_ms(),
            )
        }) {
            Ok(capability) => {
                Self::run_success(id, ResponseResult::RunCapabilityIssued { capability })
            }
            Err(error) => Self::run_error(id, error),
        }
    }

    pub(crate) fn handle_run_status(
        &mut self,
        id: String,
        params: RunStatusParams,
        host: &impl RunHost,
    ) -> String {
        let now_unix_ms = Self::run_now_unix_ms();
        let now_unix = now_unix_ms / 1000;
        let mut next = self.run_registry.clone();
        let result = (|| {
            let scope = next.authorize_at_millis(
                &CapabilityRef {
                    capability_id: params.capability.capability_id.clone(),
                    sequence: params.capability.sequence,
                },
                RunOperation::Status,
                now_unix_ms,
            )?;
            let record = next.get(&params.run_id, &scope)?.clone();
            self.observe_run_or_mark_lost(host, &mut next, &record, now_unix);
            next.get(&params.run_id, &scope).cloned()
        })();
        let persisted = if next != self.run_registry {
            self.persist_run_registry(next)
        } else if self.run_registry_load_error.is_some() || self.run_registry_path.is_none() {
            Err(RunError::PersistenceUnavailable)
        } else {
            Ok(())
        };
        match (result, persisted) {
            (_, Err(error)) => Self::run_error(id, error),
            (Ok(run), Ok(())) => Self::run_success(id, ResponseResult::RunStatus { run }),
            (Err(error), Ok(())) => Self::run_error(id, error),
        }
    }

    pub(crate) fn handle_run_submit(
        &mut self,
        id: String,
        params: RunSubmitParams,
        host: &impl RunHost,
    ) -> String {
        let requested = RequestedRunBinding {
            workspace_id: params.workspace_id.clone(),
            checkout_path: params.checkout.path.clone(),
            pane_id: params.target.pane_id.clone(),
            agent_name: params.target.agent_name.clone(),
            agent_session_id: params.target.agent_session_id.clone(),
        };
        let target = match host.live_run_target(&requested) {
            Ok(target) => target,
            Err(error) => return Self::run_error(id, error),
        };
        let now_unix_ms = Self::run_now_unix_ms();
        let now_unix = now_unix_ms / 1000;
        let mut next = self.run_registry.clone();
        let outcome = (|| {
            let scope = next.authorize_at_millis(
                &CapabilityRef {
                    capability_id: params.capability.capability_id.clone(),
                    sequence: params.capability.sequence,
                },
                RunOperation::Submit,
                now_unix_ms,
            )?;
            if scope.workspace_id != requested.workspace_id {
                return Err(RunError::NotFound);
            }
            let mut submitted = next.clone();
            let outcome = submitted.submit(
                &RunSubmission {
                    idempotency_key: params.idempotency_key.clone(),
                    prompt: params.prompt.clone(),
                    binding: RunBinding {
                        workspace_id: target.binding.workspace_id.clone(),
                        checkout_path: target.binding.checkout_path.clone(),
                        pane_id: target.binding.pane_id.clone(),
                        agent_name: Some(target.binding.agent_name.clone()),
                        agent_session_id: target.binding.agent_session_id.clone(),
                    },
                },
                now_unix,
            )?;
            if !outcome.deduplicated && !host.target_ready_for_submit(&target) {
                return Err(RunError::TargetUnavailable);
            }
            next = submitted;
            Ok(outcome)
        })();
        if next != self.run_registry {
            if let Err(error) = self.persist_run_registry(next) {
                return Self::run_error(id, error);
            }
        } else if self.run_registry_load_error.is_some() || self.run_registry_path.is_none() {
            return Self::run_error(id, RunError::PersistenceUnavailable);
        }
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => return Self::run_error(id, error),
        };
        if outcome.deduplicated {
            return Self::run_success(
                id,
                ResponseResult::RunSubmitted {
                    run: outcome.record,
                    deduplicated: true,
                },
            );
        }

        let enter = match host.lookup_runtime_sender(target.workspace_index, target.pane_id) {
            Some(runtime) => {
                let (text, enter) = runtime.run_submission_parts(&params.prompt);
                if runtime.try_send_bytes(Bytes::from(text)).is_err() {
                    return self.finish_failed_submission(id, outcome.record, now_unix);
                }
                enter
            }
            None => {
                return self.finish_failed_submission(id, outcome.record, now_unix);
            }
        };
        let mut running = self.run_registry.clone();
        let Some(run) = running.mark_started(&outcome.record.run_id, now_unix) else {
            return Self::run_error(id, RunError::PersistenceUnavailable);
        };
        if let Err(error) = self.persist_run_registry(running) {
            return Self::run_error(id, error);
        }
        let Some(runtime) = host.lookup_runtime_sender(target.workspace_index, target.pane_id)
        else {
            return self.finish_failed_submission(id, outcome.record, now_unix);
        };
        runtime.schedule_run_bytes_after(
            outcome.record.run_id.clone(),
            Bytes::from(enter),
            AGENT_PROMPT_SUBMIT_DELAY,
        );
        Self::run_success(
            id,
            ResponseResult::RunSubmitted {
                run,
                deduplicated: false,
            },
        )
    }

    fn finish_failed_submission(&mut self, id: String, record: RunRecord, now_unix: u64) -> String {
        let mut failed = self.run_registry.clone();
        let Some(_) = failed.mark_failed(&record.run_id, RunFailureKind::PromptRejected, now_unix)
        else {
            return Self::run_error(id, RunError::PersistenceUnavailable);
        };
        match self.persist_run_registry(failed) {
            Ok(()) => Self::run_error(id, RunError::TargetUnavailable),
            Err(error) => Self::run_error(id, error),
        }
    }

    pub(crate) fn mark_closed_runs_lost(&mut self, workspace_id: &str, pane_id: Option<&str>) {
        let now_unix = Self::run_now_unix();
        let mut next = self.run_registry.clone();
        let lost = match pane_id {
            Some(pane_id) => next.mark_lost_for_closed_pane(workspace_id, pane_id, now_unix),
            None => next.mark_lost_for_closed_workspace(workspace_id, now_unix),
        };
        if lost.is_empty() {
            return;
        }
        if let Err(error) = self.persist_run_registry(next) {
            tracing::warn!(
                error_code = error.code(),
                "durable run reconciliation did not persist before close"
            );
        }
    }

    pub(crate) fn handle_run_cancel(
        &mut self,
        id: String,
        params: RunCancelParams,
        host: &impl RunHost,
    ) -> String {
        let now_unix_ms = Self::run_now_unix_ms();
        let now_unix = now_unix_ms / 1000;
        let mut pending = self.run_registry.clone();
        // Snapshot the registry immediately after `authorize()` burns the
        // capability's replay-protection sequence, before `request_cancel`
        // below mutates the run record. Compensation must preserve this
        // sequence burn when an interrupt write fails.
        let mut authorized_registry: Option<RunRegistry> = None;
        // Resolve everything needed to actually deliver the interrupt first
        // (authorization, the target run's cancellability, the live pane,
        // and the encoded Ctrl-C bytes), and only once delivery is truly
        // about to be attempted, commit the `CancelRequested` transition into
        // `pending`. An interrupt is a real-world action we cannot take
        // back, so the durable record must already reflect the attempt
        // before we make it -- not persisted afterward, where a failed save
        // would leave a delivered interrupt with no durable trace of it.
        let prepared = (|| {
            let scope = pending.authorize_at_millis(
                &CapabilityRef {
                    capability_id: params.capability.capability_id.clone(),
                    sequence: params.capability.sequence,
                },
                RunOperation::Cancel,
                now_unix_ms,
            )?;
            authorized_registry = Some(pending.clone());
            let record = pending.get(&params.run_id, &scope)?.clone();
            let requested = Self::record_requested_binding(&record)?;
            let target = host.live_run_target(&requested)?;
            let Some(runtime) = host.lookup_runtime_sender(target.workspace_index, target.pane_id)
            else {
                return Err(RunError::TargetUnavailable);
            };
            let encoded = runtime.encode_terminal_key(
                crossterm::event::KeyEvent::new(
                    crossterm::event::KeyCode::Char('c'),
                    crossterm::event::KeyModifiers::CONTROL,
                )
                .into(),
            );
            let run = pending.request_cancel(&params.run_id, &scope, now_unix)?;
            Ok::<_, RunError>((run, target.workspace_index, target.pane_id, encoded))
        })();
        if pending != self.run_registry {
            if let Err(error) = self.persist_run_registry(pending) {
                return Self::run_error(id, error);
            }
        } else if self.run_registry_load_error.is_some() || self.run_registry_path.is_none() {
            return Self::run_error(id, RunError::PersistenceUnavailable);
        }
        let (run, workspace_index, pane_id, encoded) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => return Self::run_error(id, error),
        };
        let Some(runtime) = host.lookup_runtime_sender(workspace_index, pane_id) else {
            return Self::run_error(id, RunError::TargetUnavailable);
        };
        // Suppressing the scheduled Enter and sending the interrupt are
        // serialized inside the runtime: cancellation either fully prevents
        // the delayed Enter or waits for it to finish landing first, so the
        // interrupt below can never be followed by a stray Enter.
        let suppressed_enter = runtime.cancel_scheduled_run_input(&params.run_id);
        if runtime.try_send_bytes(Bytes::from(encoded)).is_err() {
            // Restore the active state before resuming any suppressed Enter.
            // This keeps the binding reserved and preserves the authorization sequence.
            let reverted = authorized_registry
                .expect("authorized_registry is set once authorize() succeeds, which it must have for `prepared` to reach this branch");
            if !self.revert_run_registry_after_failed_interrupt(reverted) {
                return Self::run_error(id, RunError::PersistenceUnavailable);
            }
            if let Some(enter) = suppressed_enter {
                runtime.schedule_run_bytes_after(
                    params.run_id.clone(),
                    enter,
                    AGENT_PROMPT_SUBMIT_DELAY,
                );
            }
            return Self::run_error(id, RunError::TargetUnavailable);
        }
        Self::run_success(id, ResponseResult::RunCancelRequested { run })
    }

    /// Persist compensation after a failed interrupt write.
    /// A run must not retain `CancelRequested` when no interrupt reached its transport.
    ///
    /// Returns `true` once the revert itself is durable. If the revert save
    /// fails, the just-persisted `CancelRequested` cannot be undone and
    /// cannot be trusted either: rather than leave that durable lie standing
    /// with only a log line, this disables every future run operation
    /// (`run_registry_load_error`) the same way a corrupt or unreadable
    /// registry does at startup, so the inconsistency is surfaced to every
    /// caller instead of silently going unnoticed.
    pub(crate) fn revert_run_registry_after_failed_interrupt(
        &mut self,
        reverted: RunRegistry,
    ) -> bool {
        let Some(path) = self.run_registry_path.clone() else {
            return true;
        };
        match crate::persist::run_registry::save_to_path(&path, &reverted) {
            Ok(()) => {
                self.run_registry = reverted;
                true
            }
            Err(_) => {
                tracing::error!(
                    "failed to revert a durably-persisted cancellation after the interrupt write \
                     failed; disabling durable run operations until this is investigated"
                );
                self.run_registry_load_error =
                    Some("durable run registry is unavailable".to_string());
                false
            }
        }
    }
}

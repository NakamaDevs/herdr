//! Adapts current session identities and terminal access to the server run service.

use crate::api::schema::{
    RunCancelParams, RunCapabilityIssueParams, RunStatusParams, RunSubmitParams,
};
#[cfg(test)]
use crate::runs::RunRegistry;
use crate::runs::{RunAgentObservation, RunError, RunRecord};
#[cfg(test)]
use crate::server::runs::AGENT_PROMPT_SUBMIT_DELAY;
use crate::server::runs::{
    validate_run_binding, LiveRunTarget, RequestedRunBinding, ResolvedRunBinding, RunHost,
    RunService,
};

impl RunHost for crate::app::App {
    fn has_workspace(&self, workspace_id: &str) -> bool {
        self.state
            .workspaces
            .iter()
            .any(|workspace| workspace.id == workspace_id)
    }
    fn live_run_target(&self, requested: &RequestedRunBinding) -> Result<LiveRunTarget, RunError> {
        let Some(workspace_index) = self
            .state
            .workspaces
            .iter()
            .position(|workspace| workspace.id == requested.workspace_id)
        else {
            return Err(RunError::CheckoutMismatch);
        };
        let workspace = &self.state.workspaces[workspace_index];
        let checkout_path = workspace
            .worktree_space()
            .map(|space| space.checkout_path.display().to_string())
            .unwrap_or_else(|| workspace.identity_cwd.display().to_string());
        let Some((pane_workspace_index, pane_id)) =
            self.parse_current_public_pane_id(&requested.pane_id)
        else {
            return Err(RunError::TargetUnavailable);
        };
        if pane_workspace_index != workspace_index {
            return Err(RunError::TargetUnavailable);
        }
        let Some(terminal_id) = workspace.terminal_id(pane_id) else {
            return Err(RunError::TargetUnavailable);
        };
        let Some(terminal) = self.state.terminals.get(terminal_id) else {
            return Err(RunError::TargetUnavailable);
        };
        let Some(agent_name) = terminal
            .agent_name
            .as_deref()
            .or_else(|| terminal.effective_agent_label())
        else {
            return Err(RunError::TargetUnavailable);
        };
        let session_ref = terminal
            .hook_authority
            .as_ref()
            .and_then(|authority| authority.session_ref.as_ref())
            .or_else(|| {
                terminal
                    .persisted_agent_session
                    .as_ref()
                    .map(|session| &session.session_ref)
            });
        let Some(agent_session_id) = session_ref.map(|session| session.value.clone()) else {
            return Err(RunError::TargetUnavailable);
        };
        let binding = ResolvedRunBinding {
            workspace_id: workspace.id.clone(),
            checkout_path,
            pane_id: requested.pane_id.clone(),
            agent_name: agent_name.to_string(),
            agent_session_id,
        };
        validate_run_binding(requested, &binding)?;
        Ok(LiveRunTarget {
            binding,
            workspace_index,
            pane_id,
        })
    }

    fn run_observation(&self, record: &RunRecord) -> Option<RunAgentObservation> {
        let requested = RunService::record_requested_binding(record).ok()?;
        let target = self.live_run_target(&requested).ok()?;
        if self
            .lookup_runtime_sender(target.workspace_index, target.pane_id)
            .is_none()
        {
            return Some(RunAgentObservation::Gone);
        }
        let workspace = self.state.workspaces.get(target.workspace_index)?;
        let terminal_id = workspace.terminal_id(target.pane_id)?;
        let terminal = self.state.terminals.get(terminal_id)?;
        match terminal.state {
            crate::detect::AgentState::Working => Some(RunAgentObservation::Working),
            crate::detect::AgentState::Blocked => Some(RunAgentObservation::Blocked),
            crate::detect::AgentState::Idle => Some(RunAgentObservation::Idle),
            crate::detect::AgentState::Unknown => None,
        }
    }

    fn lookup_runtime_sender(
        &self,
        workspace_index: usize,
        pane_id: crate::layout::PaneId,
    ) -> Option<&crate::terminal::TerminalRuntime> {
        crate::app::App::lookup_runtime_sender(self, workspace_index, pane_id)
    }

    fn target_ready_for_submit(&self, target: &LiveRunTarget) -> bool {
        if self
            .lookup_runtime_sender(target.workspace_index, target.pane_id)
            .is_none_or(|runtime| runtime.pending_delayed_input_count() != 0)
        {
            return false;
        }
        let workspace = &self.state.workspaces[target.workspace_index];
        workspace
            .terminal_id(target.pane_id)
            .and_then(|id| self.state.terminals.get(id))
            .is_some_and(|terminal| terminal.state == crate::detect::AgentState::Idle)
    }
}

impl crate::app::App {
    // Dispatch is synchronous. Move the service out while the adapter borrows runtime facts.
    fn with_run_service<T>(&mut self, operation: impl FnOnce(&mut RunService, &Self) -> T) -> T {
        let mut service = std::mem::take(&mut self.run_service);
        let result = operation(&mut service, self);
        self.run_service = service;
        result
    }

    pub(super) fn handle_run_capability_issue(
        &mut self,
        id: String,
        params: RunCapabilityIssueParams,
    ) -> String {
        self.with_run_service(|service, host| service.handle_run_capability_issue(id, params, host))
    }
    pub(super) fn handle_run_status(&mut self, id: String, params: RunStatusParams) -> String {
        self.with_run_service(|service, host| service.handle_run_status(id, params, host))
    }
    pub(super) fn handle_run_submit(&mut self, id: String, params: RunSubmitParams) -> String {
        self.with_run_service(|service, host| service.handle_run_submit(id, params, host))
    }
    pub(super) fn handle_run_cancel(&mut self, id: String, params: RunCancelParams) -> String {
        self.with_run_service(|service, host| service.handle_run_cancel(id, params, host))
    }
    pub(super) fn mark_closed_runs_lost(&mut self, workspace_id: &str, pane_id: Option<&str>) {
        self.run_service
            .mark_closed_runs_lost(workspace_id, pane_id);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::schema::{
        Method, PaneTarget, Request, RunCancelParams, RunCapabilityIssueParams, RunCapabilityRef,
        RunCheckout, RunStatusParams, RunSubmitParams, RunTarget,
    };
    use crate::runs::{
        auth::RunOperation, RunBinding, RunFailureKind, RunRecord, RunScope, RunState,
        RunSubmission,
    };
    use bytes::Bytes;
    use std::sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    };
    use tokio::sync::mpsc;

    static NEXT_TEMP_REGISTRY: AtomicU64 = AtomicU64::new(0);

    struct TempRegistry {
        directory: std::path::PathBuf,
        path: std::path::PathBuf,
    }

    impl TempRegistry {
        fn new(name: &str) -> Self {
            loop {
                let sequence = NEXT_TEMP_REGISTRY.fetch_add(1, Ordering::Relaxed);
                let directory = std::env::temp_dir()
                    .join(format!("nak-439-{name}-{}-{sequence}", std::process::id(),));
                match std::fs::create_dir(&directory) {
                    Ok(()) => {
                        let path = directory.join("runs.json");
                        return Self { directory, path };
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("temporary registry directory: {error}"),
                }
            }
        }
    }

    impl Drop for TempRegistry {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    fn app() -> crate::app::App {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        crate::app::App::new(
            &crate::config::Config::default(),
            true,
            None,
            api_rx,
            crate::api::EventHub::default(),
        )
    }

    fn app_with_workspace_and_path(temp: &TempRegistry) -> (crate::app::App, String) {
        let mut app = app();
        app.state.workspaces = vec![crate::workspace::Workspace::test_new("run-api")];
        let workspace_id = app.state.workspaces[0].id.clone();
        app.set_run_registry_path_for_test(temp.path.clone());
        (app, workspace_id)
    }

    fn issue_params(workspace_id: String) -> RunCapabilityIssueParams {
        RunCapabilityIssueParams {
            workspace_id,
            ttl_ms: 60_000,
            operations: vec![RunOperation::Status],
        }
    }

    fn response(response: String) -> serde_json::Value {
        serde_json::from_str(&response).expect("valid API response")
    }

    fn seed_status_run(temp: &TempRegistry, workspace_id: &str) -> (RunRegistry, String, String) {
        let mut registry = RunRegistry::default();
        let now_unix = RunService::run_now_unix();
        let run_id = registry
            .submit(
                &RunSubmission {
                    idempotency_key: "status-key".to_string(),
                    prompt: "review the change".to_string(),
                    binding: RunBinding {
                        workspace_id: workspace_id.to_string(),
                        checkout_path: "/tmp/repo-a".to_string(),
                        pane_id: "pane-a".to_string(),
                        agent_name: Some("reviewer".to_string()),
                        agent_session_id: "session-a".to_string(),
                    },
                },
                now_unix,
            )
            .expect("seed run")
            .record
            .run_id;
        let capability = registry
            .issue_capability(workspace_id, 60_000, &[RunOperation::Status], now_unix)
            .expect("seed capability");
        crate::persist::run_registry::save_to_path(&temp.path, &registry)
            .expect("save seeded registry");
        (registry, run_id, capability.capability_id)
    }

    struct LiveRunFixture {
        app: crate::app::App,
        prompt_rx: Option<mpsc::Receiver<Bytes>>,
        workspace_id: String,
        checkout_path: String,
        pane_id: String,
        agent_name: String,
        agent_session_id: String,
        terminal_id: crate::terminal::TerminalId,
    }

    impl LiveRunFixture {
        fn submit_params(
            &self,
            capability: RunCapabilityRef,
            key: &str,
            prompt: &str,
        ) -> RunSubmitParams {
            RunSubmitParams {
                capability,
                idempotency_key: key.to_string(),
                workspace_id: self.workspace_id.clone(),
                checkout: RunCheckout {
                    path: self.checkout_path.clone(),
                },
                target: RunTarget {
                    pane_id: self.pane_id.clone(),
                    agent_name: self.agent_name.clone(),
                    agent_session_id: self.agent_session_id.clone(),
                },
                prompt: prompt.to_string(),
            }
        }

        fn issue_capability(&mut self, operations: Vec<RunOperation>) -> RunCapabilityRef {
            let issued = response(self.app.handle_run_capability_issue(
                "issue-capability".to_string(),
                RunCapabilityIssueParams {
                    workspace_id: self.workspace_id.clone(),
                    ttl_ms: 60_000,
                    operations,
                },
            ));
            RunCapabilityRef {
                capability_id: issued["result"]["capability"]["capability_id"]
                    .as_str()
                    .expect("capability id")
                    .to_string(),
                sequence: 1,
            }
        }

        fn request(&mut self, id: &str, method: Method) -> serde_json::Value {
            response(self.app.handle_api_request(Request {
                id: id.to_string(),
                method,
            }))
        }

        fn seed_run(&mut self, key: &str, state: RunState) -> RunRecord {
            let now_unix = RunService::run_now_unix();
            let mut registry = self.app.run_service.run_registry.clone();
            let record = registry
                .submit(
                    &RunSubmission {
                        idempotency_key: key.to_string(),
                        prompt: "seed prompt".to_string(),
                        binding: RunBinding {
                            workspace_id: self.workspace_id.clone(),
                            checkout_path: self.checkout_path.clone(),
                            pane_id: self.pane_id.clone(),
                            agent_name: Some(self.agent_name.clone()),
                            agent_session_id: self.agent_session_id.clone(),
                        },
                    },
                    now_unix,
                )
                .expect("seed run")
                .record;
            match state {
                RunState::Queued => {}
                RunState::Running => {
                    registry.mark_started(&record.run_id, now_unix);
                }
                RunState::Failed => {
                    registry.mark_failed(&record.run_id, RunFailureKind::PromptRejected, now_unix);
                }
                RunState::CancelRequested => {
                    registry
                        .request_cancel(
                            &record.run_id,
                            &RunScope {
                                workspace_id: self.workspace_id.clone(),
                            },
                            now_unix,
                        )
                        .expect("seed cancellation");
                }
                _ => panic!("unsupported seed state"),
            }
            crate::persist::run_registry::save_to_path(
                self.app
                    .run_service
                    .run_registry_path
                    .as_deref()
                    .expect("temporary registry path"),
                &registry,
            )
            .expect("persist seeded run");
            self.app.run_service.run_registry = registry;
            self.app
                .run_service
                .run_registry
                .records()
                .iter()
                .find(|candidate| candidate.run_id == record.run_id)
                .expect("seeded run record")
                .clone()
        }
    }

    fn live_run_fixture(temp: &TempRegistry) -> LiveRunFixture {
        let (runtime, prompt_rx) = crate::terminal::TerminalRuntime::test_with_channel(80, 24);
        live_run_fixture_with_runtime(temp, runtime, prompt_rx)
    }

    fn live_run_fixture_with_input_observer<F>(
        temp: &TempRegistry,
        input_observer: F,
    ) -> LiveRunFixture
    where
        F: Fn(&Bytes) + Send + Sync + 'static,
    {
        let (runtime, prompt_rx) =
            crate::terminal::TerminalRuntime::test_with_channel_and_input_observer(
                80,
                24,
                input_observer,
            );
        live_run_fixture_with_runtime(temp, runtime, prompt_rx)
    }

    fn live_run_fixture_with_runtime(
        temp: &TempRegistry,
        runtime: crate::terminal::TerminalRuntime,
        prompt_rx: mpsc::Receiver<Bytes>,
    ) -> LiveRunFixture {
        let mut app = app();
        let workspace = crate::workspace::Workspace::test_new("run-api");
        let pane = workspace.tabs[0].root_pane;
        let terminal_id = workspace.terminal_id(pane).expect("root terminal").clone();
        let workspace_id = workspace.id.clone();
        let checkout_path = workspace.identity_cwd.display().to_string();
        app.state.workspaces = vec![workspace];
        app.state.ensure_test_terminals();
        app.state.active = Some(0);
        app.state.selected = 0;
        let terminal = app
            .state
            .terminals
            .get_mut(&terminal_id)
            .expect("root terminal state");
        terminal.set_agent_name("reviewer".to_string());
        terminal.set_detected_state(
            Some(crate::detect::Agent::Codex),
            crate::detect::AgentState::Idle,
        );
        terminal.set_persisted_agent_session(crate::agent_resume::PersistedAgentSession {
            source: "herdr:codex".to_string(),
            agent: "reviewer".to_string(),
            session_ref: crate::agent_resume::AgentSessionRef::id("session-a")
                .expect("test session"),
        });
        runtime.test_process_pty_bytes(b"\x1b[?2004h");
        app.state.insert_test_runtime(pane, runtime);
        let pane_id = app.public_pane_id(0, pane).expect("public pane id");
        app.set_run_registry_path_for_test(temp.path.clone());

        app.state.assert_invariants_for_test();
        LiveRunFixture {
            app,
            prompt_rx: Some(prompt_rx),
            workspace_id,
            checkout_path,
            pane_id,
            agent_name: "reviewer".to_string(),
            agent_session_id: "session-a".to_string(),
            terminal_id,
        }
    }

    fn capability(sequence: u64) -> RunCapabilityRef {
        RunCapabilityRef {
            capability_id: "cap_1".to_string(),
            sequence,
        }
    }

    #[tokio::test]
    async fn submit_rejects_a_busy_agent_without_writing_prompt_bytes() {
        for state in [
            crate::detect::AgentState::Working,
            crate::detect::AgentState::Blocked,
            crate::detect::AgentState::Unknown,
        ] {
            let temp = TempRegistry::new("submit-busy");
            let mut fixture = live_run_fixture(&temp);
            fixture
                .app
                .state
                .terminals
                .get_mut(&fixture.terminal_id)
                .expect("terminal")
                .set_detected_state(Some(crate::detect::Agent::Codex), state);
            let capability = fixture.issue_capability(vec![RunOperation::Submit]);
            let submitted = fixture.request(
                "busy",
                Method::RunSubmit(fixture.submit_params(capability, "busy", "review")),
            );
            assert_eq!(submitted["error"]["code"], "run_target_unavailable");
            assert!(fixture
                .prompt_rx
                .as_mut()
                .expect("receiver")
                .try_recv()
                .is_err());
            assert!(fixture.app.run_service.run_registry.is_empty());
        }
    }

    #[tokio::test]
    async fn submit_rejects_an_idle_agent_with_an_earlier_pending_enter() {
        let temp = TempRegistry::new("submit-pending-enter");
        let mut fixture = live_run_fixture(&temp);
        let pane = fixture.app.state.workspaces[0].tabs[0].root_pane;
        fixture
            .app
            .lookup_runtime_sender(0, pane)
            .unwrap()
            .send_bytes_after(
                Bytes::from_static(b"\r"),
                std::time::Duration::from_secs(60),
            );
        let capability = fixture.issue_capability(vec![RunOperation::Submit]);
        let submitted = fixture.request(
            "pending-enter",
            Method::RunSubmit(fixture.submit_params(capability, "pending-enter", "review")),
        );
        assert_eq!(submitted["error"]["code"], "run_target_unavailable");
        assert!(fixture.prompt_rx.as_mut().unwrap().try_recv().is_err());
        assert!(fixture.app.run_service.run_registry.is_empty());
    }

    #[tokio::test]
    async fn submit_retry_keeps_the_existing_run_when_its_agent_is_busy() {
        let temp = TempRegistry::new("retry-busy");
        let mut fixture = live_run_fixture(&temp);
        let run = fixture.seed_run("retry", RunState::Running);
        fixture
            .app
            .state
            .terminals
            .get_mut(&fixture.terminal_id)
            .expect("terminal")
            .set_detected_state(
                Some(crate::detect::Agent::Codex),
                crate::detect::AgentState::Working,
            );
        let capability = fixture.issue_capability(vec![RunOperation::Submit]);
        let repeated = fixture.request(
            "retry",
            Method::RunSubmit(fixture.submit_params(capability, "retry", "seed prompt")),
        );
        assert_eq!(repeated["result"]["deduplicated"], true);
        assert_eq!(repeated["result"]["run"]["run_id"], run.run_id);
        assert_eq!(repeated["result"]["run"]["state"], "running");
        assert!(fixture
            .prompt_rx
            .as_mut()
            .expect("receiver")
            .try_recv()
            .is_err());
    }

    #[tokio::test]
    async fn retry_after_cancelled_enter_and_failed_interrupt_recovers_delivery() {
        let temp = TempRegistry::new("retry-cancelled-enter");
        let mut fixture = live_run_fixture(&temp);
        fixture
            .app
            .state
            .terminals
            .get_mut(&fixture.terminal_id)
            .expect("terminal")
            .set_detected_state(
                Some(crate::detect::Agent::Codex),
                crate::detect::AgentState::Idle,
            );
        let capability = fixture.issue_capability(vec![RunOperation::Submit, RunOperation::Cancel]);
        let submitted = fixture.request(
            "submit",
            Method::RunSubmit(fixture.submit_params(capability.clone(), "retry", "review")),
        );
        assert_result_type(&submitted, "run_submitted");
        let run_id = submitted["result"]["run"]["run_id"]
            .as_str()
            .expect("run id")
            .to_string();
        let (_, pane_id) = fixture
            .app
            .parse_current_public_pane_id(&fixture.pane_id)
            .expect("pane");
        let runtime = fixture
            .app
            .lookup_runtime_sender(0, pane_id)
            .expect("runtime");
        while runtime
            .try_send_bytes(Bytes::from_static(b"filled"))
            .is_ok()
        {}
        let cancelled = fixture.request(
            "cancel",
            Method::RunCancel(RunCancelParams {
                capability: RunCapabilityRef {
                    sequence: 2,
                    ..capability.clone()
                },
                run_id,
            }),
        );
        assert_eq!(cancelled["error"]["code"], "run_target_unavailable");
        while fixture
            .prompt_rx
            .as_mut()
            .expect("receiver")
            .try_recv()
            .is_ok()
        {}
        let repeated = fixture.request(
            "retry",
            Method::RunSubmit(fixture.submit_params(
                RunCapabilityRef {
                    sequence: 3,
                    ..capability.clone()
                },
                "retry",
                "review",
            )),
        );
        assert_eq!(repeated["result"]["deduplicated"], true);
        assert_eq!(repeated["result"]["run"]["state"], "running");
        let competing = fixture.request(
            "competing",
            Method::RunSubmit(fixture.submit_params(
                RunCapabilityRef {
                    sequence: 4,
                    ..capability
                },
                "different-key",
                "new prompt",
            )),
        );
        assert_eq!(competing["error"]["code"], "run_binding_busy");
        let enter = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            fixture.prompt_rx.as_mut().expect("receiver").recv(),
        )
        .await
        .expect("recovered Enter must arrive")
        .expect("Enter bytes");
        assert_eq!(enter, Bytes::from_static(b"\r"));
        assert!(fixture
            .prompt_rx
            .as_mut()
            .expect("receiver")
            .try_recv()
            .is_err());
        let persisted = crate::persist::run_registry::load_from_path(&temp.path).expect("registry");
        assert_eq!(persisted.records()[0].state, RunState::Running);
    }

    #[test]
    fn durable_run_ownership_is_outside_the_tui_app() {
        let app_source = include_str!("../mod.rs");
        assert!(!app_source.contains("pub(crate) run_registry:"));
        assert!(!app_source.contains("fn load_run_registry("));
        assert!(!app_source.contains("reconcile_after_restart_with_bindings("));
    }

    fn resolved() -> ResolvedRunBinding {
        ResolvedRunBinding {
            workspace_id: "w1".to_string(),
            checkout_path: "/tmp/repo-a".to_string(),
            pane_id: "w1:p1".to_string(),
            agent_name: "reviewer".to_string(),
            agent_session_id: "session-a".to_string(),
        }
    }

    fn requested() -> RequestedRunBinding {
        RequestedRunBinding {
            workspace_id: "w1".to_string(),
            checkout_path: "/tmp/repo-a".to_string(),
            pane_id: "w1:p1".to_string(),
            agent_name: "reviewer".to_string(),
            agent_session_id: "session-a".to_string(),
        }
    }

    // Matrix row 26. Submit requires all workspace and agent identities to match.
    #[test]
    fn run_submit_requires_workspace_checkout_pane_agent_and_session_identity() {
        let resolved = resolved();
        assert_eq!(validate_run_binding(&requested(), &resolved), Ok(()));

        let mut wrong_workspace = requested();
        wrong_workspace.workspace_id = "w2".to_string();
        assert_eq!(
            validate_run_binding(&wrong_workspace, &resolved),
            Err(RunError::CheckoutMismatch)
        );

        let mut wrong_checkout = requested();
        wrong_checkout.checkout_path = "/tmp/repo-b".to_string();
        assert_eq!(
            validate_run_binding(&wrong_checkout, &resolved),
            Err(RunError::CheckoutMismatch)
        );

        let mut wrong_pane = requested();
        wrong_pane.pane_id = "w1:p2".to_string();
        assert_eq!(
            validate_run_binding(&wrong_pane, &resolved),
            Err(RunError::TargetUnavailable)
        );

        let mut wrong_agent = requested();
        wrong_agent.agent_name = "implementer".to_string();
        assert_eq!(
            validate_run_binding(&wrong_agent, &resolved),
            Err(RunError::TargetUnavailable)
        );

        let mut wrong_session = requested();
        wrong_session.agent_session_id = "session-b".to_string();
        assert_eq!(
            validate_run_binding(&wrong_session, &resolved),
            Err(RunError::TargetUnavailable)
        );
    }

    #[tokio::test]
    async fn app_api_dispatches_capability_submit_repeat_status_cancel_and_foreign_access() {
        let temp = TempRegistry::new("dispatch");
        let mut fixture = live_run_fixture(&temp);
        let capability = fixture.issue_capability(vec![
            RunOperation::Submit,
            RunOperation::Status,
            RunOperation::Cancel,
        ]);
        let submitted = fixture.request(
            "submit",
            Method::RunSubmit(fixture.submit_params(capability.clone(), "key-1", "review")),
        );
        assert_eq!(submitted["id"], "submit");
        assert_result_type(&submitted, "run_submitted");
        let run_id = submitted["result"]["run"]["run_id"]
            .as_str()
            .expect("run id")
            .to_string();

        let repeated = fixture.request(
            "repeat",
            Method::RunSubmit(fixture.submit_params(
                RunCapabilityRef {
                    sequence: 2,
                    ..capability.clone()
                },
                "key-1",
                "review",
            )),
        );
        assert_eq!(repeated["id"], "repeat");
        assert_result_type(&repeated, "run_submitted");
        assert_eq!(repeated["result"]["deduplicated"], true);

        let status = fixture.request(
            "status",
            Method::RunStatus(RunStatusParams {
                capability: RunCapabilityRef {
                    sequence: 3,
                    ..capability.clone()
                },
                run_id: run_id.clone(),
            }),
        );
        assert_eq!(status["id"], "status");
        assert_result_type(&status, "run_status");

        let cancelled = fixture.request(
            "cancel",
            Method::RunCancel(RunCancelParams {
                capability: RunCapabilityRef {
                    sequence: 4,
                    ..capability.clone()
                },
                run_id,
            }),
        );
        assert_eq!(cancelled["id"], "cancel");
        assert_result_type(&cancelled, "run_cancel_requested");

        let foreign = fixture.request(
            "foreign",
            Method::RunStatus(RunStatusParams {
                capability: RunCapabilityRef {
                    sequence: 5,
                    ..capability
                },
                run_id: "run_other_workspace".to_string(),
            }),
        );
        assert_eq!(foreign["id"], "foreign");
        assert_eq!(foreign["error"]["code"], "run_not_found");
    }

    #[tokio::test]
    async fn wire_errors_preserve_request_id_without_echoing_each_caller_value() {
        for (index, value) in [
            "key sk live secret",
            "workspace-secret",
            "/tmp/checkout-secret\n",
            "pane secret",
            "agent secret",
            "session secret",
            "prompt-secret",
        ]
        .into_iter()
        .enumerate()
        {
            let temp = TempRegistry::new("wire-error");
            let mut fixture = live_run_fixture(&temp);
            let capability = fixture.issue_capability(vec![RunOperation::Submit]);
            let mut method = Method::RunSubmit(fixture.submit_params(capability, "key", "review"));
            let Method::RunSubmit(params) = &mut method else {
                unreachable!();
            };
            match index {
                0 => params.idempotency_key = value.to_string(),
                1 => params.workspace_id = value.to_string(),
                2 => params.checkout.path = value.to_string(),
                3 => params.target.pane_id = value.to_string(),
                4 => params.target.agent_name = value.to_string(),
                5 => params.target.agent_session_id = value.to_string(),
                6 => {
                    params.prompt =
                        format!("{}{}", "x".repeat(crate::runs::MAX_PROMPT_BYTES), value)
                }
                _ => unreachable!(),
            }
            let response: serde_json::Value =
                serde_json::from_str(&fixture.app.handle_api_request(Request {
                    id: "wire-error-id".to_string(),
                    method,
                }))
                .expect("wire response");
            assert_eq!(response["id"], "wire-error-id");
            assert!(response.get("error").is_some());
            assert!(!response["error"]["message"]
                .as_str()
                .expect("error message")
                .contains(value));
        }
    }

    #[tokio::test]
    async fn app_api_capability_ttl_scope_replay_and_persistence_contract() {
        let temp = TempRegistry::new("capability-contract");
        let mut fixture = live_run_fixture(&temp);
        let invalid_ttl = fixture.app.handle_api_request(Request {
            id: "ttl".to_string(),
            method: Method::RunCapabilityIssue(RunCapabilityIssueParams {
                workspace_id: fixture.workspace_id.clone(),
                ttl_ms: 999,
                operations: vec![crate::runs::auth::RunOperation::Submit],
            }),
        });
        let invalid_ttl: serde_json::Value = serde_json::from_str(&invalid_ttl).expect("response");
        assert_eq!(invalid_ttl["id"], "ttl");
        assert_eq!(invalid_ttl["error"]["code"], "run_invalid_request");

        let issued = fixture.app.handle_api_request(Request {
            id: "issue".to_string(),
            method: Method::RunCapabilityIssue(RunCapabilityIssueParams {
                workspace_id: fixture.workspace_id.clone(),
                ttl_ms: 60_000,
                operations: vec![crate::runs::auth::RunOperation::Status],
            }),
        });
        let issued: serde_json::Value = serde_json::from_str(&issued).expect("response");
        let capability_id = issued["result"]["capability"]["capability_id"]
            .as_str()
            .expect("issued capability")
            .to_string();

        let status = |id: &str, sequence: u64, run_id: &str| Request {
            id: id.to_string(),
            method: Method::RunStatus(RunStatusParams {
                capability: RunCapabilityRef {
                    capability_id: capability_id.clone(),
                    sequence,
                },
                run_id: run_id.to_string(),
            }),
        };
        let foreign: serde_json::Value = serde_json::from_str(
            &fixture
                .app
                .handle_api_request(status("foreign", 1, "run-w2")),
        )
        .expect("response");
        assert_eq!(foreign["id"], "foreign");
        assert_eq!(foreign["error"]["code"], "run_not_found");

        let replay: serde_json::Value = serde_json::from_str(
            &fixture
                .app
                .handle_api_request(status("replay", 1, "run-w2")),
        )
        .expect("response");
        assert_eq!(replay["id"], "replay");
        assert_eq!(replay["error"]["code"], "run_replay_rejected");

        let persisted: serde_json::Value = serde_json::from_str(
            &fixture
                .app
                .handle_api_request(status("persisted", 2, "run-w2")),
        )
        .expect("response");
        assert_eq!(persisted["id"], "persisted");
        assert_eq!(persisted["error"]["code"], "run_not_found");
    }

    #[test]
    fn no_session_app_has_no_durable_path_and_cannot_mutate_runs() {
        let mut app = app();
        app.state.workspaces = vec![crate::workspace::Workspace::test_new("run-api")];
        let workspace_id = app.state.workspaces[0].id.clone();
        assert!(app.run_service.run_registry_path.is_none());
        assert!(app.run_service.run_registry_load_error.is_none());

        let result = response(
            app.handle_run_capability_issue("no-session".to_string(), issue_params(workspace_id)),
        );
        assert_eq!(result["id"], "no-session");
        assert_eq!(result["error"]["code"], "run_persistence_unavailable");
        assert!(app.run_service.run_registry.is_empty());
    }

    #[test]
    fn injected_missing_path_is_ready_and_never_uses_user_configuration() {
        let temp = TempRegistry::new("missing");
        let (app, _) = app_with_workspace_and_path(&temp);
        assert_eq!(app.run_service.run_registry.len(), 0);
        assert_eq!(
            app.run_service.run_registry_path.as_deref(),
            Some(temp.path.as_path())
        );
        assert!(!temp.path.starts_with(crate::config::config_dir()));
    }

    #[test]
    fn valid_temporary_registry_loads_exactly() {
        let valid = TempRegistry::new("valid");
        let mut registry = RunRegistry::default();
        registry
            .issue_capability("w1", 60_000, &[RunOperation::Status], 1)
            .expect("capability");
        crate::persist::run_registry::save_to_path(&valid.path, &registry).expect("save valid");
        let (app, _) = app_with_workspace_and_path(&valid);
        assert_eq!(app.run_service.run_registry, registry);
        assert!(app.run_service.run_registry_load_error.is_none());
        assert!(!app
            .run_service
            .run_registry_path
            .as_deref()
            .expect("test registry path")
            .starts_with(crate::config::config_dir()));
    }

    #[tokio::test]
    async fn startup_reconciliation_persists_runs_without_a_complete_live_binding_as_lost() {
        let temp = TempRegistry::new("startup-reconciliation");
        let mut fixture = live_run_fixture(&temp);
        let mut registry = RunRegistry::default();
        let run = registry
            .submit(
                &RunSubmission {
                    idempotency_key: "restored-run".to_string(),
                    prompt: "review".to_string(),
                    binding: RunBinding {
                        workspace_id: fixture.workspace_id.clone(),
                        checkout_path: fixture.checkout_path.clone(),
                        pane_id: fixture.pane_id.clone(),
                        agent_name: Some(fixture.agent_name.clone()),
                        agent_session_id: "different-session".to_string(),
                    },
                },
                RunService::run_now_unix(),
            )
            .expect("restored run")
            .record;
        crate::persist::run_registry::save_to_path(&temp.path, &registry)
            .expect("save restored registry");
        fixture
            .app
            .set_run_registry_path_for_test(temp.path.clone());

        fixture.app.reconcile_run_registry_after_startup();

        let persisted = crate::persist::run_registry::load_from_path(&temp.path)
            .expect("load reconciled registry");
        let restored = persisted
            .records()
            .iter()
            .find(|record| record.run_id == run.run_id)
            .expect("restored record");
        assert_eq!(restored.state, RunState::Lost);
        assert_eq!(restored.failure, Some(RunFailureKind::ServerRestart));
        assert!(fixture.app.run_service.run_registry_load_error.is_none());
    }

    fn assert_disabled_run_methods(mut app: crate::app::App, workspace_id: String) {
        let issue =
            response(app.handle_run_capability_issue(
                "disabled-issue".to_string(),
                issue_params(workspace_id),
            ));
        assert_eq!(issue["error"]["code"], "run_persistence_unavailable");

        let status = response(app.handle_run_status(
            "disabled-status".to_string(),
            RunStatusParams {
                capability: capability(1),
                run_id: "run-a".to_string(),
            },
        ));
        assert_eq!(status["error"]["code"], "run_persistence_unavailable");
    }

    #[test]
    fn corrupt_temporary_registry_disables_run_methods() {
        let temp = TempRegistry::new("corrupt");
        std::fs::write(&temp.path, b"not-json").expect("corrupt registry");
        let (app, workspace_id) = app_with_workspace_and_path(&temp);
        assert!(app.run_service.run_registry_load_error.is_some());
        assert_disabled_run_methods(app, workspace_id);
    }

    #[test]
    fn future_temporary_registry_disables_run_methods() {
        let temp = TempRegistry::new("future");
        std::fs::write(&temp.path, br#"{"version":999,"runs":[]}"#).expect("future registry");
        let (app, workspace_id) = app_with_workspace_and_path(&temp);
        assert!(app.run_service.run_registry_load_error.is_some());
        assert_disabled_run_methods(app, workspace_id);
    }

    #[test]
    fn failed_save_leaves_app_registry_unchanged() {
        let temp = TempRegistry::new("save-failure");
        let blocked_parent = temp.directory.join("not-a-directory");
        std::fs::write(&blocked_parent, b"block parent creation").expect("blocked parent");
        let failed_path = blocked_parent.join("runs.json");
        let (mut app, workspace_id) = app_with_workspace_and_path(&temp);
        app.set_run_registry_path_for_test(failed_path);
        let before = app.run_service.run_registry.clone();

        let result = response(
            app.handle_run_capability_issue("save-failure".to_string(), issue_params(workspace_id)),
        );
        assert_eq!(result["error"]["code"], "run_persistence_unavailable");
        assert_eq!(app.run_service.run_registry, before);
    }

    #[test]
    fn capability_issue_requires_an_existing_workspace() {
        let temp = TempRegistry::new("workspace-required");
        let mut app = app();
        app.set_run_registry_path_for_test(temp.path.clone());

        let result = response(app.handle_run_capability_issue(
            "workspace-required".to_string(),
            issue_params("missing-workspace".to_string()),
        ));
        assert_eq!(result["id"], "workspace-required");
        assert_eq!(result["error"]["code"], "run_not_found");
        assert!(app.run_service.run_registry.is_empty());
    }

    #[test]
    fn capability_issue_persists_the_capability() {
        let temp = TempRegistry::new("capability-persist");
        let (mut app, workspace_id) = app_with_workspace_and_path(&temp);

        let result = response(
            app.handle_run_capability_issue("issue".to_string(), issue_params(workspace_id)),
        );
        assert_eq!(result["id"], "issue");
        assert_eq!(result["result"]["type"], "run_capability_issued");
        let persisted = crate::persist::run_registry::load_from_path(&temp.path)
            .expect("persisted capability registry");
        assert_eq!(persisted, app.run_service.run_registry);
    }

    #[test]
    fn status_burns_and_persists_the_capability_sequence() {
        let temp = TempRegistry::new("status-persist");
        let (mut app, workspace_id) = app_with_workspace_and_path(&temp);
        let (_, run_id, capability_id) = seed_status_run(&temp, &workspace_id);
        app.set_run_registry_path_for_test(temp.path.clone());

        let result = response(app.handle_run_status(
            "status".to_string(),
            RunStatusParams {
                capability: RunCapabilityRef {
                    capability_id: capability_id.clone(),
                    sequence: 1,
                },
                run_id,
            },
        ));
        assert_eq!(result["id"], "status");
        assert_eq!(result["result"]["type"], "run_status");

        let mut persisted =
            crate::persist::run_registry::load_from_path(&temp.path).expect("persisted sequence");
        assert_eq!(
            persisted.authorize(
                &crate::runs::auth::CapabilityRef {
                    capability_id,
                    sequence: 1,
                },
                RunOperation::Status,
                RunService::run_now_unix(),
            ),
            Err(RunError::ReplayRejected)
        );
    }

    #[test]
    fn replayed_status_returns_run_replay_rejected() {
        let temp = TempRegistry::new("status-replay");
        let (mut app, workspace_id) = app_with_workspace_and_path(&temp);
        let (_, run_id, capability_id) = seed_status_run(&temp, &workspace_id);
        app.set_run_registry_path_for_test(temp.path.clone());

        let status = RunStatusParams {
            capability: RunCapabilityRef {
                capability_id,
                sequence: 1,
            },
            run_id,
        };
        let first = response(app.handle_run_status("first".to_string(), status.clone()));
        assert_eq!(first["result"]["type"], "run_status");
        let replay = response(app.handle_run_status("replay".to_string(), status));
        assert_eq!(replay["id"], "replay");
        assert_eq!(replay["error"]["code"], "run_replay_rejected");
    }

    #[test]
    fn foreign_status_returns_run_not_found() {
        let temp = TempRegistry::new("status-foreign");
        let (mut app, workspace_id) = app_with_workspace_and_path(&temp);
        let (_, _, capability_id) = seed_status_run(&temp, &workspace_id);
        app.set_run_registry_path_for_test(temp.path.clone());

        let result = response(app.handle_run_status(
            "foreign".to_string(),
            RunStatusParams {
                capability: RunCapabilityRef {
                    capability_id,
                    sequence: 1,
                },
                run_id: "foreign-run".to_string(),
            },
        ));
        assert_eq!(result["id"], "foreign");
        assert_eq!(result["error"]["code"], "run_not_found");
    }

    fn assert_result_type(response: &serde_json::Value, expected: &str) {
        assert_eq!(response["result"]["type"], expected, "response: {response}");
    }

    #[tokio::test]
    async fn run_submit_resolves_the_complete_live_workspace_checkout_pane_agent_and_session_binding(
    ) {
        let temp = TempRegistry::new("submit-live-binding");
        let mut fixture = live_run_fixture(&temp);
        let capability = fixture.issue_capability(vec![RunOperation::Submit]);
        let params = fixture.submit_params(capability, "live-binding", "review this change");

        let submitted = fixture.request("submit", Method::RunSubmit(params));
        assert_result_type(&submitted, "run_submitted");
        let run = &submitted["result"]["run"];
        assert_eq!(run["workspace_id"], fixture.workspace_id);
        assert_eq!(run["checkout_path"], fixture.checkout_path);
        assert_eq!(run["pane_id"], fixture.pane_id);
        assert_eq!(run["agent_name"], fixture.agent_name);
        assert_eq!(run["agent_session_id"], fixture.agent_session_id);
    }

    #[tokio::test]
    async fn run_submit_persists_queued_before_fake_runtime_receives_prompt_bytes() {
        let temp = TempRegistry::new("submit-queued-before-write");
        let queued_when_prompt_was_written = Arc::new(AtomicBool::new(false));
        let observed_path = temp.path.clone();
        let observed_queued = queued_when_prompt_was_written.clone();
        let mut fixture = live_run_fixture_with_input_observer(&temp, move |_bytes| {
            let queued = crate::persist::run_registry::load_from_path(&observed_path)
                .ok()
                .is_some_and(|registry| {
                    registry
                        .records()
                        .iter()
                        .any(|run| run.state == RunState::Queued)
                });
            observed_queued.store(queued, Ordering::SeqCst);
        });
        let capability = fixture.issue_capability(vec![RunOperation::Submit]);

        let submitted = fixture.request(
            "submit",
            Method::RunSubmit(fixture.submit_params(capability, "queued-before-write", "review")),
        );
        assert_result_type(&submitted, "run_submitted");
        assert!(queued_when_prompt_was_written.load(Ordering::SeqCst));
        assert!(fixture
            .prompt_rx
            .as_mut()
            .expect("fake runtime receiver")
            .try_recv()
            .is_ok());
    }

    #[tokio::test]
    async fn successful_submit_delivery_marks_and_persists_running() {
        let temp = TempRegistry::new("submit-running");
        let mut fixture = live_run_fixture(&temp);
        let capability = fixture.issue_capability(vec![RunOperation::Submit]);

        let submitted = fixture.request(
            "submit",
            Method::RunSubmit(fixture.submit_params(capability, "running", "review")),
        );
        assert_result_type(&submitted, "run_submitted");
        assert_eq!(submitted["result"]["run"]["state"], "running");
        let persisted = crate::persist::run_registry::load_from_path(&temp.path)
            .expect("running submission is durable");
        assert_eq!(persisted, fixture.app.run_service.run_registry);
        assert_eq!(
            fixture
                .prompt_rx
                .as_mut()
                .expect("fake runtime receiver")
                .try_recv()
                .expect("prompt bytes"),
            Bytes::from_static(b"\x1b[200~review\x1b[201~")
        );
    }

    #[tokio::test]
    async fn submit_uses_the_request_timestamp_for_each_transition() {
        let temp = TempRegistry::new("submit-single-clock");
        let mut fixture = live_run_fixture(&temp);
        let capability = fixture.issue_capability(vec![RunOperation::Submit]);

        RunService::set_run_clock_for_test([1_700_000_000, 1_700_000_001]);
        let submitted = fixture.request(
            "submit-clock",
            Method::RunSubmit(fixture.submit_params(capability, "single-clock", "review")),
        );
        RunService::set_run_clock_for_test([]);

        assert_result_type(&submitted, "run_submitted");
        assert_eq!(submitted["result"]["run"]["created_at_unix"], 1_700_000_000);
        assert_eq!(submitted["result"]["run"]["updated_at_unix"], 1_700_000_000);
        assert_eq!(submitted["result"]["run"]["started_at_unix"], 1_700_000_000);
    }

    #[tokio::test]
    async fn identical_submit_retry_returns_the_original_id_without_a_second_prompt_write() {
        let temp = TempRegistry::new("submit-repeat");
        let mut fixture = live_run_fixture(&temp);
        let capability = fixture.issue_capability(vec![RunOperation::Submit]);
        let first = fixture.request(
            "first",
            Method::RunSubmit(fixture.submit_params(capability.clone(), "repeat", "review")),
        );
        assert_result_type(&first, "run_submitted");
        let first_id = first["result"]["run"]["run_id"]
            .as_str()
            .expect("first run id")
            .to_string();
        let _ = fixture
            .prompt_rx
            .as_mut()
            .expect("fake runtime receiver")
            .try_recv()
            .expect("first prompt bytes");

        let retry = fixture.request(
            "retry",
            Method::RunSubmit(fixture.submit_params(
                RunCapabilityRef {
                    sequence: 2,
                    ..capability
                },
                "repeat",
                "review",
            )),
        );
        assert_result_type(&retry, "run_submitted");
        assert_eq!(retry["result"]["run"]["run_id"], first_id);
        assert_eq!(retry["result"]["deduplicated"], true);
        assert!(fixture
            .prompt_rx
            .as_mut()
            .expect("fake runtime receiver")
            .try_recv()
            .is_err());
    }

    #[tokio::test]
    async fn reused_submit_key_with_different_work_fails_closed() {
        let temp = TempRegistry::new("submit-conflict");
        let mut fixture = live_run_fixture(&temp);
        let capability = fixture.issue_capability(vec![RunOperation::Submit]);
        let first = fixture.request(
            "first",
            Method::RunSubmit(fixture.submit_params(capability.clone(), "conflict", "review")),
        );
        assert_result_type(&first, "run_submitted");
        let _ = fixture
            .prompt_rx
            .as_mut()
            .expect("fake runtime receiver")
            .try_recv()
            .expect("first prompt bytes");

        let conflict = fixture.request(
            "conflict",
            Method::RunSubmit(fixture.submit_params(
                RunCapabilityRef {
                    sequence: 2,
                    ..capability
                },
                "conflict",
                "different work",
            )),
        );
        assert_eq!(conflict["error"]["code"], "run_idempotency_conflict");
        assert!(fixture
            .prompt_rx
            .as_mut()
            .expect("fake runtime receiver")
            .try_recv()
            .is_err());
    }

    #[tokio::test]
    async fn each_submit_binding_mismatch_stores_no_run_and_writes_no_prompt() {
        for mismatch in ["workspace", "checkout", "pane", "agent", "session"] {
            let temp = TempRegistry::new(mismatch);
            let mut fixture = live_run_fixture(&temp);
            let capability = fixture.issue_capability(vec![RunOperation::Submit]);
            let mut params = fixture.submit_params(capability, "mismatch", "review");
            let expected = match mismatch {
                "workspace" => {
                    params.workspace_id = "other-workspace".to_string();
                    "run_checkout_mismatch"
                }
                "checkout" => {
                    params.checkout.path = "/other/checkout".to_string();
                    "run_checkout_mismatch"
                }
                "pane" => {
                    params.target.pane_id = "w0:p999".to_string();
                    "run_target_unavailable"
                }
                "agent" => {
                    params.target.agent_name = "other-agent".to_string();
                    "run_target_unavailable"
                }
                "session" => {
                    params.target.agent_session_id = "other-session".to_string();
                    "run_target_unavailable"
                }
                _ => unreachable!(),
            };

            let rejected = fixture.request("mismatch", Method::RunSubmit(params));
            assert_eq!(rejected["error"]["code"], expected, "mismatch: {mismatch}");
            assert!(
                fixture.app.run_service.run_registry.is_empty(),
                "mismatch: {mismatch}"
            );
            assert!(fixture
                .prompt_rx
                .as_mut()
                .expect("fake runtime receiver")
                .try_recv()
                .is_err());
        }
    }

    #[tokio::test]
    async fn submit_persistence_failure_writes_no_prompt() {
        let temp = TempRegistry::new("submit-save-failure");
        let blocked_parent = temp.directory.join("not-a-directory");
        std::fs::write(&blocked_parent, b"block registry save").expect("blocked registry parent");
        let mut fixture = live_run_fixture(&temp);
        let capability = fixture.issue_capability(vec![RunOperation::Submit]);
        fixture.app.run_service.run_registry_path = Some(blocked_parent.join("runs.json"));

        let rejected = fixture.request(
            "submit",
            Method::RunSubmit(fixture.submit_params(capability, "save-failure", "review")),
        );
        assert_eq!(rejected["error"]["code"], "run_persistence_unavailable");
        assert!(fixture.app.run_service.run_registry.is_empty());
        assert!(fixture
            .prompt_rx
            .as_mut()
            .expect("fake runtime receiver")
            .try_recv()
            .is_err());
    }

    #[tokio::test]
    async fn running_save_failure_after_prompt_delivery_disables_run_retries() {
        let temp = TempRegistry::new("running-save-failure");
        let blocked_temporary = temp.path.with_extension("json.tmp");
        let observer_temporary = blocked_temporary.clone();
        let mut fixture = live_run_fixture_with_input_observer(&temp, move |_| {
            std::fs::create_dir(&observer_temporary).expect("block the next registry save");
        });
        let capability = fixture.issue_capability(vec![RunOperation::Submit]);
        let submitted = fixture.request(
            "submit",
            Method::RunSubmit(fixture.submit_params(capability.clone(), "save-failure", "review")),
        );
        assert_eq!(submitted["error"]["code"], "run_persistence_unavailable");
        assert_eq!(
            fixture.prompt_rx.as_mut().unwrap().try_recv().unwrap(),
            Bytes::from_static(b"\x1b[200~review\x1b[201~")
        );
        std::fs::remove_dir(&blocked_temporary).expect("restore persistence");
        let retry = fixture.request(
            "retry",
            Method::RunSubmit(fixture.submit_params(
                RunCapabilityRef {
                    sequence: 2,
                    ..capability
                },
                "save-failure",
                "review",
            )),
        );
        assert_eq!(retry["error"]["code"], "run_persistence_unavailable");
        assert!(fixture.app.run_service.run_registry_load_error.is_some());
        assert!(fixture.prompt_rx.as_mut().unwrap().try_recv().is_err());
        assert_eq!(
            fixture.app.run_service.run_registry.records()[0].state,
            RunState::Queued
        );
    }

    #[tokio::test]
    async fn prompt_delivery_failure_persists_a_typed_failed_run() {
        let temp = TempRegistry::new("submit-prompt-failure");
        let mut fixture = live_run_fixture(&temp);
        fixture.prompt_rx.take();
        let capability = fixture.issue_capability(vec![RunOperation::Submit]);

        let rejected = fixture.request(
            "submit",
            Method::RunSubmit(fixture.submit_params(capability, "prompt-failure", "review")),
        );
        assert_eq!(rejected["error"]["code"], "run_target_unavailable");
        let persisted = crate::persist::run_registry::load_from_path(&temp.path)
            .expect("failed delivery is durable");
        let failed = persisted.records().first().expect("failed run");
        assert_eq!(failed.state, RunState::Failed);
        assert_eq!(failed.failure, Some(RunFailureKind::PromptRejected));
    }

    #[tokio::test]
    async fn submit_uses_the_existing_bracketed_paste_safe_prompt_encoding() {
        let temp = TempRegistry::new("submit-bracketed-paste");
        let mut fixture = live_run_fixture(&temp);
        let capability = fixture.issue_capability(vec![RunOperation::Submit]);

        let submitted = fixture.request(
            "submit",
            Method::RunSubmit(fixture.submit_params(capability, "bracketed", "line one\nline two")),
        );
        assert_result_type(&submitted, "run_submitted");
        assert_eq!(
            fixture
                .prompt_rx
                .as_mut()
                .expect("fake runtime receiver")
                .try_recv()
                .expect("prompt bytes"),
            Bytes::from_static(b"\x1b[200~line one\nline two\x1b[201~")
        );
    }

    #[tokio::test]
    async fn cancel_addresses_one_run_and_signals_only_its_exact_runtime() {
        let temp = TempRegistry::new("cancel-exact-runtime");
        let mut fixture = live_run_fixture(&temp);
        let first = fixture.seed_run("cancel-first", RunState::Queued);
        let second_pane =
            fixture.app.state.workspaces[0].test_split(ratatui::layout::Direction::Horizontal);
        fixture.app.state.ensure_test_terminals();
        let second_terminal_id = fixture.app.state.workspaces[0]
            .terminal_id(second_pane)
            .expect("second terminal")
            .clone();
        let second_terminal = fixture
            .app
            .state
            .terminals
            .get_mut(&second_terminal_id)
            .expect("second terminal state");
        second_terminal.set_agent_name("reviewer".to_string());
        second_terminal.set_detected_state(
            Some(crate::detect::Agent::Codex),
            crate::detect::AgentState::Working,
        );
        second_terminal.set_persisted_agent_session(crate::agent_resume::PersistedAgentSession {
            source: "herdr:codex".to_string(),
            agent: "reviewer".to_string(),
            session_ref: crate::agent_resume::AgentSessionRef::id("session-b")
                .expect("second test session"),
        });
        let (second_runtime, mut second_rx) =
            crate::terminal::TerminalRuntime::test_with_channel(80, 24);
        fixture
            .app
            .state
            .insert_test_runtime(second_pane, second_runtime);
        let second_pane_id = fixture
            .app
            .public_pane_id(0, second_pane)
            .expect("second public pane id");
        let now_unix = RunService::run_now_unix();
        let mut registry = fixture.app.run_service.run_registry.clone();
        let second = registry
            .submit(
                &RunSubmission {
                    idempotency_key: "cancel-second".to_string(),
                    prompt: "seed prompt".to_string(),
                    binding: RunBinding {
                        workspace_id: fixture.workspace_id.clone(),
                        checkout_path: fixture.checkout_path.clone(),
                        pane_id: second_pane_id,
                        agent_name: Some(fixture.agent_name.clone()),
                        agent_session_id: "session-b".to_string(),
                    },
                },
                now_unix,
            )
            .expect("second seeded run")
            .record;
        crate::persist::run_registry::save_to_path(&temp.path, &registry)
            .expect("persist second seeded run");
        fixture.app.run_service.run_registry = registry;
        let capability = fixture.issue_capability(vec![RunOperation::Cancel]);

        let cancelled = fixture.request(
            "cancel",
            Method::RunCancel(RunCancelParams {
                capability,
                run_id: first.run_id.clone(),
            }),
        );
        assert_result_type(&cancelled, "run_cancel_requested");
        assert_eq!(cancelled["result"]["run"]["run_id"], first.run_id);
        assert_eq!(
            fixture
                .prompt_rx
                .as_mut()
                .expect("first fake runtime")
                .try_recv()
                .expect("cancellation bytes"),
            Bytes::from_static(b"\x03")
        );
        assert!(second_rx.try_recv().is_err());
        assert_eq!(
            fixture
                .app
                .run_service
                .run_registry
                .records()
                .iter()
                .find(|run| run.run_id == second.run_id)
                .expect("second run")
                .state,
            RunState::Queued
        );
    }

    #[tokio::test]
    async fn cancel_before_submit_delay_suppresses_that_runs_enter() {
        let temp = TempRegistry::new("cancel-pending-enter");
        let mut fixture = live_run_fixture(&temp);
        let submit_capability = fixture.issue_capability(vec![RunOperation::Submit]);
        let submitted = fixture.request(
            "submit",
            Method::RunSubmit(fixture.submit_params(submit_capability, "cancel-enter", "review")),
        );
        let run_id = submitted["result"]["run"]["run_id"]
            .as_str()
            .expect("submitted run ID")
            .to_string();
        assert!(
            fixture
                .prompt_rx
                .as_mut()
                .expect("fake runtime receiver")
                .try_recv()
                .is_ok(),
            "prompt body is sent immediately"
        );

        let cancel_capability = fixture.issue_capability(vec![RunOperation::Cancel]);
        let cancelled = fixture.request(
            "cancel",
            Method::RunCancel(RunCancelParams {
                capability: cancel_capability,
                run_id,
            }),
        );
        assert_result_type(&cancelled, "run_cancel_requested");
        assert!(
            fixture
                .prompt_rx
                .as_mut()
                .expect("fake runtime receiver")
                .try_recv()
                .is_ok(),
            "cancel reaches the exact runtime"
        );

        tokio::time::sleep(AGENT_PROMPT_SUBMIT_DELAY + std::time::Duration::from_millis(50)).await;
        assert!(
            fixture
                .prompt_rx
                .as_mut()
                .expect("fake runtime receiver")
                .try_recv()
                .is_err(),
            "a cancelled run must not send its delayed Enter"
        );
    }

    // Defect: `CancelRequested` used to persist unconditionally before the
    // interrupt write was attempted, so a failed runtime write left a
    // durable record claiming a cancellation that never reached the pane.
    // The fix commits `CancelRequested` durably before sending the
    // interrupt (so a successful interrupt is never left unrecorded, see
    // `cancel_persistence_failure_writes_no_interrupt` below) and reverts
    // that persisted state again if the interrupt write itself then fails,
    // which this test exercises end to end.
    #[tokio::test]
    async fn failed_interrupt_delivery_does_not_persist_cancel_requested_and_stays_retryable() {
        let temp = TempRegistry::new("cancel-interrupt-failure");
        let mut fixture = live_run_fixture(&temp);
        let run = fixture.seed_run("cancel-interrupt-failure", RunState::Running);
        // Close the runtime's input channel so the interrupt write fails,
        // the same way a dead or backed-up pane would fail it.
        fixture.prompt_rx.take();
        let capability = fixture.issue_capability(vec![RunOperation::Cancel]);

        let cancelled = fixture.request(
            "cancel",
            Method::RunCancel(RunCancelParams {
                capability,
                run_id: run.run_id.clone(),
            }),
        );
        assert_eq!(cancelled["error"]["code"], "run_target_unavailable");

        let persisted = crate::persist::run_registry::load_from_path(&temp.path)
            .expect("registry after failed interrupt");
        let after = persisted
            .records()
            .iter()
            .find(|record| record.run_id == run.run_id)
            .expect("run record survives a failed interrupt");
        assert_eq!(
            after.state,
            RunState::Running,
            "a failed interrupt write must not durably claim a cancellation was requested"
        );
        assert_eq!(
            after.updated_at_unix, run.updated_at_unix,
            "a failed interrupt write must leave the run record itself untouched"
        );
        let after_updated_at_unix = after.updated_at_unix;

        // Retryable: the registry's own guard must still accept a cancel for
        // this run, proving it was never stuck behind a false
        // `CancelRequested`/`NotCancellable` state.
        let mut retry_registry = persisted;
        let retry_scope = RunScope {
            workspace_id: fixture.workspace_id.clone(),
        };
        assert!(
            retry_registry
                .request_cancel(&run.run_id, &retry_scope, after_updated_at_unix)
                .is_ok(),
            "the run must remain retryable after a failed interrupt delivery"
        );
    }

    // Defect: a successful interrupt write could still leave no durable
    // cancellation record, because `CancelRequested` used to persist only
    // after the interrupt was already sent -- if that persist then failed,
    // the pane really was interrupted but the record never caught up, so a
    // later status read could report the run as still running, blocked, or
    // even succeeded. The fix commits `CancelRequested` durably first and
    // only sends the interrupt once that is safely on disk, so persistence
    // failure must leave the pane completely untouched.
    #[tokio::test]
    async fn cancel_persistence_failure_writes_no_interrupt() {
        let temp = TempRegistry::new("cancel-save-failure");
        let mut fixture = live_run_fixture(&temp);
        let run = fixture.seed_run("cancel-save-failure", RunState::Running);
        let capability = fixture.issue_capability(vec![RunOperation::Cancel]);
        let blocked_parent = temp.directory.join("not-a-directory");
        std::fs::write(&blocked_parent, b"block registry save").expect("blocked registry parent");
        fixture.app.run_service.run_registry_path = Some(blocked_parent.join("runs.json"));

        let cancelled = fixture.request(
            "cancel",
            Method::RunCancel(RunCancelParams {
                capability,
                run_id: run.run_id.clone(),
            }),
        );
        assert_eq!(cancelled["error"]["code"], "run_persistence_unavailable");
        assert!(
            fixture
                .prompt_rx
                .as_mut()
                .expect("fake runtime receiver")
                .try_recv()
                .is_err(),
            "the interrupt must never be sent when the pending CancelRequested cannot be persisted"
        );
    }

    // Defect: the compensating revert on a failed interrupt used to restore
    // the registry snapshot captured *before* `authorize()` ran, which
    // un-burned the capability's replay-protection sequence along with the
    // run's `CancelRequested` transition. A capability+sequence that
    // genuinely authorized must never become replayable just because the
    // interrupt delivery that followed happened to fail.
    #[tokio::test]
    async fn failed_interrupt_delivery_keeps_the_capability_sequence_permanently_burned() {
        let temp = TempRegistry::new("cancel-sequence-burn");
        let mut fixture = live_run_fixture(&temp);
        let run = fixture.seed_run("cancel-sequence-burn", RunState::Running);
        fixture.prompt_rx.take();
        let capability = fixture.issue_capability(vec![RunOperation::Cancel]);

        let cancelled = fixture.request(
            "cancel",
            Method::RunCancel(RunCancelParams {
                capability: capability.clone(),
                run_id: run.run_id.clone(),
            }),
        );
        assert_eq!(cancelled["error"]["code"], "run_target_unavailable");

        let mut persisted = crate::persist::run_registry::load_from_path(&temp.path)
            .expect("registry after failed interrupt");
        assert_eq!(
            persisted.authorize(
                &crate::runs::auth::CapabilityRef {
                    capability_id: capability.capability_id,
                    sequence: capability.sequence,
                },
                RunOperation::Cancel,
                RunService::run_now_unix(),
            ),
            Err(RunError::ReplayRejected),
            "a capability sequence consumed by a genuinely successful authorize() must stay \
             permanently burned even when the interrupt delivery that followed failed"
        );
    }

    // Defect: if the compensating persist itself failed, the code only
    // logged a warning and returned `TargetUnavailable`, leaving the
    // durable record standing at `CancelRequested` with no interrupt ever
    // delivered and no way for a caller to detect the inconsistency. The
    // fix must not let that lie stand silently.
    #[test]
    fn failed_compensating_revert_disables_durable_run_operations_instead_of_lying() {
        let temp = TempRegistry::new("cancel-revert-save-failure");
        let (mut app, _workspace_id) = app_with_workspace_and_path(&temp);
        let before = app.run_service.run_registry.clone();
        let blocked_parent = temp.directory.join("not-a-directory");
        std::fs::write(&blocked_parent, b"block registry save").expect("blocked registry parent");
        app.run_service.run_registry_path = Some(blocked_parent.join("runs.json"));

        let reverted_ok = app
            .run_service
            .revert_run_registry_after_failed_interrupt(RunRegistry::default());

        assert!(
            !reverted_ok,
            "a failed revert save must be reported as failed"
        );
        assert_eq!(
            app.run_service.run_registry, before,
            "the in-memory registry must not silently adopt an unpersisted revert"
        );
        assert!(
            app.run_service.run_registry_load_error.is_some(),
            "a failed compensating persist must disable durable run operations, not just log"
        );

        // Prove this is actually enforced, not merely an internal flag:
        // every subsequent run operation must now fail closed instead of
        // ever risking a read against the untrustworthy durable state.
        let status = response(app.handle_run_status(
            "status-after-broken-revert".to_string(),
            RunStatusParams {
                capability: capability(1),
                run_id: "run-a".to_string(),
            },
        ));
        assert_eq!(status["error"]["code"], "run_persistence_unavailable");
    }

    // Direct ordering proof for the same defect: at the exact moment the
    // interrupt bytes reach the runtime, the durable record must already
    // show `CancelRequested`, not whatever state the run was in beforehand.
    #[tokio::test]
    async fn cancel_requested_is_already_durable_when_the_interrupt_is_sent() {
        let temp = TempRegistry::new("cancel-durable-before-interrupt");
        let observed_path = temp.path.clone();
        let observed_state_at_send: Arc<std::sync::Mutex<Option<RunState>>> =
            Arc::new(std::sync::Mutex::new(None));
        let observed_state_for_observer = Arc::clone(&observed_state_at_send);
        let mut fixture = live_run_fixture_with_input_observer(&temp, move |_bytes| {
            let registry = crate::persist::run_registry::load_from_path(&observed_path)
                .expect("registry readable when the interrupt is sent");
            let state = registry
                .records()
                .first()
                .expect("run record present when the interrupt is sent")
                .state;
            *observed_state_for_observer.lock().unwrap() = Some(state);
        });
        let run = fixture.seed_run("cancel-durable-before-interrupt", RunState::Running);
        let capability = fixture.issue_capability(vec![RunOperation::Cancel]);

        let cancelled = fixture.request(
            "cancel",
            Method::RunCancel(RunCancelParams {
                capability,
                run_id: run.run_id.clone(),
            }),
        );
        assert_result_type(&cancelled, "run_cancel_requested");
        assert_eq!(
            *observed_state_at_send.lock().unwrap(),
            Some(RunState::CancelRequested),
            "CancelRequested must already be durable at the moment the interrupt is sent"
        );
    }

    #[tokio::test]
    async fn pane_close_marks_matching_active_runs_lost_and_persists() {
        let temp = TempRegistry::new("close-active-run");
        let mut fixture = live_run_fixture(&temp);
        let run = fixture.seed_run("close-active", RunState::Running);

        let closed = fixture.request(
            "close",
            Method::PaneClose(PaneTarget {
                pane_id: fixture.pane_id.clone(),
            }),
        );

        assert_eq!(closed["result"]["type"], "ok");
        let persisted = crate::persist::run_registry::load_from_path(&temp.path)
            .expect("closed run registry persists");
        let closed_run = persisted
            .records()
            .iter()
            .find(|record| record.run_id == run.run_id)
            .expect("closed run remains retained");
        assert_eq!(closed_run.state, RunState::Lost);
        assert_eq!(closed_run.failure, Some(RunFailureKind::AgentUnavailable));
    }

    #[tokio::test]
    async fn pane_close_continues_when_run_reconciliation_cannot_persist() {
        let temp = TempRegistry::new("close-persistence-failure");
        let mut fixture = live_run_fixture(&temp);
        fixture.seed_run("close-persistence-failure", RunState::Running);
        let blocked_parent = temp.directory.join("not-a-directory");
        std::fs::write(&blocked_parent, b"block registry parent").expect("block registry path");
        fixture.app.run_service.run_registry_path = Some(blocked_parent.join("runs.json"));

        let closed = fixture.request(
            "close",
            Method::PaneClose(PaneTarget {
                pane_id: fixture.pane_id.clone(),
            }),
        );

        assert_eq!(closed["result"]["type"], "ok");
        assert!(fixture.app.state.workspaces.is_empty());
    }

    #[tokio::test]
    async fn foreign_capability_cannot_cancel_a_run() {
        let temp = TempRegistry::new("cancel-foreign-capability");
        let mut fixture = live_run_fixture(&temp);
        let run = fixture.seed_run("foreign-cancel", RunState::Queued);
        let foreign_workspace = crate::workspace::Workspace::test_new("foreign");
        let foreign_workspace_id = foreign_workspace.id.clone();
        fixture.app.state.workspaces.push(foreign_workspace);
        let foreign_capability = response(fixture.app.handle_run_capability_issue(
            "issue-foreign".to_string(),
            RunCapabilityIssueParams {
                workspace_id: foreign_workspace_id,
                ttl_ms: 60_000,
                operations: vec![RunOperation::Cancel],
            },
        ));

        let cancelled = fixture.request(
            "cancel",
            Method::RunCancel(RunCancelParams {
                capability: RunCapabilityRef {
                    capability_id: foreign_capability["result"]["capability"]["capability_id"]
                        .as_str()
                        .expect("foreign capability")
                        .to_string(),
                    sequence: 1,
                },
                run_id: run.run_id,
            }),
        );
        assert_eq!(cancelled["error"]["code"], "run_not_found");
    }

    #[tokio::test]
    async fn terminal_run_cannot_cancel() {
        let temp = TempRegistry::new("cancel-terminal");
        let mut fixture = live_run_fixture(&temp);
        let run = fixture.seed_run("terminal-cancel", RunState::Failed);
        let capability = fixture.issue_capability(vec![RunOperation::Cancel]);

        let cancelled = fixture.request(
            "cancel",
            Method::RunCancel(RunCancelParams {
                capability,
                run_id: run.run_id,
            }),
        );
        assert_eq!(cancelled["error"]["code"], "run_not_cancellable");
    }

    #[tokio::test]
    async fn status_maps_typed_working_blocked_idle_and_gone_facts_without_terminal_text() {
        let temp = TempRegistry::new("status-lifecycle");
        let mut fixture = live_run_fixture(&temp);
        let run = fixture.seed_run("lifecycle", RunState::Queued);
        let capability = fixture.issue_capability(vec![RunOperation::Status]);

        fixture
            .app
            .state
            .terminals
            .get_mut(&fixture.terminal_id)
            .expect("live terminal")
            .set_detected_state(
                Some(crate::detect::Agent::Codex),
                crate::detect::AgentState::Working,
            );
        let working = fixture.request(
            "working",
            Method::RunStatus(RunStatusParams {
                capability: capability.clone(),
                run_id: run.run_id.clone(),
            }),
        );
        assert_eq!(working["result"]["run"]["state"], "running");

        fixture
            .app
            .state
            .terminals
            .get_mut(&fixture.terminal_id)
            .expect("live terminal")
            .set_detected_state(
                Some(crate::detect::Agent::Codex),
                crate::detect::AgentState::Blocked,
            );
        let blocked = fixture.request(
            "blocked",
            Method::RunStatus(RunStatusParams {
                capability: RunCapabilityRef {
                    sequence: 2,
                    ..capability.clone()
                },
                run_id: run.run_id.clone(),
            }),
        );
        assert_eq!(blocked["result"]["run"]["state"], "blocked");

        fixture
            .app
            .state
            .terminals
            .get_mut(&fixture.terminal_id)
            .expect("live terminal")
            .set_detected_state(
                Some(crate::detect::Agent::Codex),
                crate::detect::AgentState::Idle,
            );
        let idle = fixture.request(
            "idle",
            Method::RunStatus(RunStatusParams {
                capability: RunCapabilityRef {
                    sequence: 3,
                    ..capability
                },
                run_id: run.run_id.clone(),
            }),
        );
        assert_eq!(idle["result"]["run"]["state"], "succeeded");

        let temp = TempRegistry::new("status-gone");
        let mut gone_fixture = live_run_fixture(&temp);
        let gone_run = gone_fixture.seed_run("gone", RunState::Running);
        let gone_capability = gone_fixture.issue_capability(vec![RunOperation::Status]);
        gone_fixture
            .app
            .state
            .terminals
            .remove(&gone_fixture.terminal_id);
        let gone = gone_fixture.request(
            "gone",
            Method::RunStatus(RunStatusParams {
                capability: gone_capability,
                run_id: gone_run.run_id,
            }),
        );
        assert_eq!(gone["result"]["run"]["state"], "lost");
    }

    #[tokio::test]
    async fn status_after_cancel_maps_a_typed_stop_to_cancelled() {
        let temp = TempRegistry::new("status-cancelled");
        let mut fixture = live_run_fixture(&temp);
        let run = fixture.seed_run("cancelled", RunState::CancelRequested);
        let capability = fixture.issue_capability(vec![RunOperation::Status]);
        fixture
            .app
            .state
            .terminals
            .get_mut(&fixture.terminal_id)
            .expect("live terminal")
            .set_detected_state(
                Some(crate::detect::Agent::Codex),
                crate::detect::AgentState::Idle,
            );

        let status = fixture.request(
            "status",
            Method::RunStatus(RunStatusParams {
                capability,
                run_id: run.run_id,
            }),
        );
        assert_eq!(status["result"]["run"]["state"], "cancelled");
    }

    #[tokio::test]
    async fn submit_and_cancel_persist_each_consumed_capability_sequence() {
        let temp = TempRegistry::new("submit-cancel-replay");
        let mut fixture = live_run_fixture(&temp);
        let submit_capability = fixture.issue_capability(vec![RunOperation::Submit]);
        let submitted = fixture.request(
            "submit",
            Method::RunSubmit(fixture.submit_params(
                submit_capability.clone(),
                "persist-submit-sequence",
                "review",
            )),
        );
        assert_result_type(&submitted, "run_submitted");
        let mut after_submit = crate::persist::run_registry::load_from_path(&temp.path)
            .expect("submit replay state is durable");
        assert_eq!(
            after_submit.authorize(
                &crate::runs::auth::CapabilityRef {
                    capability_id: submit_capability.capability_id,
                    sequence: 1,
                },
                RunOperation::Submit,
                RunService::run_now_unix(),
            ),
            Err(RunError::ReplayRejected)
        );

        let run_id = submitted["result"]["run"]["run_id"]
            .as_str()
            .expect("submitted run ID")
            .to_string();
        let cancel_capability = fixture.issue_capability(vec![RunOperation::Cancel]);
        let cancelled = fixture.request(
            "cancel",
            Method::RunCancel(RunCancelParams {
                capability: cancel_capability.clone(),
                run_id,
            }),
        );
        assert_result_type(&cancelled, "run_cancel_requested");
        let mut after_cancel = crate::persist::run_registry::load_from_path(&temp.path)
            .expect("cancel replay state is durable");
        assert_eq!(
            after_cancel.authorize(
                &crate::runs::auth::CapabilityRef {
                    capability_id: cancel_capability.capability_id,
                    sequence: 1,
                },
                RunOperation::Cancel,
                RunService::run_now_unix(),
            ),
            Err(RunError::ReplayRejected)
        );
    }

    #[tokio::test]
    async fn final_run_success_envelope_stays_within_the_response_bound() {
        let temp = TempRegistry::new("response-bound");
        let mut fixture = live_run_fixture(&temp);
        let submit_capability = fixture.issue_capability(vec![RunOperation::Submit]);
        let success = fixture.app.handle_api_request(Request {
            id: "submit".to_string(),
            method: Method::RunSubmit(fixture.submit_params(
                submit_capability,
                "response-bound",
                "review",
            )),
        });
        assert!(success.len() <= crate::runs::MAX_RUN_RESULT_BYTES);
        let success = response(success);
        assert_result_type(&success, "run_submitted");
    }

    #[tokio::test]
    async fn final_run_error_envelope_stays_within_the_response_bound() {
        let temp = TempRegistry::new("error-response-bound");
        let mut fixture = live_run_fixture(&temp);
        let error = fixture.app.handle_api_request(Request {
            id: "r".repeat(crate::runs::MAX_RUN_RESULT_BYTES),
            method: Method::RunStatus(RunStatusParams {
                capability: capability(1),
                run_id: "missing".to_string(),
            }),
        });
        assert!(error.len() <= crate::runs::MAX_RUN_RESULT_BYTES);
    }
}

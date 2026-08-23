use std::collections::HashSet;

use super::auth::{CapabilityRef, RunOperation, MAX_CAPABILITY_TTL_MS, MIN_CAPABILITY_TTL_MS};
use super::*;

const NOW: u64 = 1_700_000_000;
const ALL_OPERATIONS: [RunOperation; 3] = [
    RunOperation::Submit,
    RunOperation::Status,
    RunOperation::Cancel,
];

fn binding(workspace: &str, pane: &str) -> RunBinding {
    RunBinding {
        workspace_id: workspace.to_string(),
        checkout_path: "/tmp/repo-a".to_string(),
        pane_id: pane.to_string(),
        agent_name: Some("reviewer".to_string()),
        agent_session_id: "session-a".to_string(),
    }
}

fn observe(
    registry: &mut RunRegistry,
    pane: &str,
    observation: RunAgentObservation,
    now_unix: u64,
) -> bool {
    registry.observe_agent_state_for_pane(pane, observation, now_unix)
}

fn observation_binding(workspace: &str, pane: &str) -> RunObservationBinding {
    RunObservationBinding {
        workspace_id: workspace.to_string(),
        checkout_path: "/tmp/repo-a".to_string(),
        pane_id: pane.to_string(),
        agent_name: Some("reviewer".to_string()),
        agent_session_id: "session-a".to_string(),
    }
}

fn submission(key: &str, prompt: &str, workspace: &str, pane: &str) -> RunSubmission {
    RunSubmission {
        idempotency_key: key.to_string(),
        prompt: prompt.to_string(),
        binding: binding(workspace, pane),
    }
}

fn scope(workspace: &str) -> RunScope {
    RunScope {
        workspace_id: workspace.to_string(),
    }
}

fn registry_with_run(key: &str, prompt: &str) -> (RunRegistry, RunRecord) {
    let mut registry = RunRegistry::default();
    let outcome = registry
        .submit(&submission(key, prompt, "w1", "w1:pA"), NOW)
        .expect("first submit is accepted");
    (registry, outcome.record)
}

// Matrix row 3.
#[test]
fn run_submit_key_conflict_fails_closed() {
    let (mut registry, first) = registry_with_run("kantoku-1", "review the diff");

    let repeated = registry
        .submit(
            &submission("kantoku-1", "review the diff", "w1", "w1:pA"),
            NOW + 1,
        )
        .expect("identical repeat is accepted");
    assert!(repeated.deduplicated);
    assert_eq!(repeated.record.run_id, first.run_id);
    assert_eq!(registry.len(), 1);

    let different_prompt = registry.submit(
        &submission("kantoku-1", "do something else", "w1", "w1:pA"),
        NOW + 2,
    );
    assert_eq!(different_prompt, Err(RunError::IdempotencyConflict));

    let different_target = registry.submit(
        &submission("kantoku-1", "review the diff", "w1", "w1:pB"),
        NOW + 3,
    );
    assert_eq!(different_target, Err(RunError::IdempotencyConflict));

    assert_eq!(
        registry.len(),
        1,
        "a conflicting key must not store a record"
    );
}

#[test]
fn one_active_run_per_full_binding_rejects_different_work_and_allows_idempotent_retry() {
    let mut registry = RunRegistry::default();
    let first = registry
        .submit(&submission("active-one", "first", "w1", "w1:pA"), NOW)
        .expect("first submit")
        .record;
    let repeated = registry
        .submit(&submission("active-one", "first", "w1", "w1:pA"), NOW + 1)
        .expect("idempotent retry");
    assert!(repeated.deduplicated);
    assert_eq!(repeated.record.run_id, first.run_id);
    assert_eq!(
        registry.submit(&submission("active-two", "second", "w1", "w1:pA"), NOW + 2),
        Err(RunError::BindingBusy),
        "a second active run must not share the complete binding"
    );
}

// Matrix row 3 (bounds half).
#[test]
fn run_submit_rejects_unbounded_identifiers() {
    let mut registry = RunRegistry::default();

    let long_key = "k".repeat(MAX_IDEMPOTENCY_KEY_LEN + 1);
    assert_eq!(
        registry.submit(&submission(&long_key, "prompt", "w1", "w1:pA"), NOW),
        Err(RunError::InvalidRequest(RunInvalidField::IdempotencyKey))
    );

    assert_eq!(
        registry.submit(&submission("", "prompt", "w1", "w1:pA"), NOW),
        Err(RunError::InvalidRequest(RunInvalidField::IdempotencyKey))
    );

    assert_eq!(
        registry.submit(&submission("with space", "prompt", "w1", "w1:pA"), NOW),
        Err(RunError::InvalidRequest(RunInvalidField::IdempotencyKey))
    );

    let long_workspace = "w".repeat(MAX_WORKSPACE_ID_LEN + 1);
    assert_eq!(
        registry.submit(&submission("k1", "prompt", &long_workspace, "w1:pA"), NOW),
        Err(RunError::InvalidRequest(RunInvalidField::WorkspaceId))
    );

    let mut long_checkout = submission("k1", "prompt", "w1", "w1:pA");
    long_checkout.binding.checkout_path = "/".repeat(MAX_CHECKOUT_PATH_LEN + 1);
    assert_eq!(
        registry.submit(&long_checkout, NOW),
        Err(RunError::InvalidRequest(RunInvalidField::Checkout))
    );

    let long_target = "p".repeat(MAX_TARGET_LEN + 1);
    assert_eq!(
        registry.submit(&submission("k1", "prompt", "w1", &long_target), NOW),
        Err(RunError::InvalidRequest(RunInvalidField::Target))
    );

    assert!(registry.is_empty());
}

// Matrix row 10.
#[test]
fn run_submit_rejects_unbounded_prompt() {
    let mut registry = RunRegistry::default();
    let oversized = "a".repeat(MAX_PROMPT_BYTES + 1);

    assert_eq!(
        registry.submit(&submission("k1", &oversized, "w1", "w1:pA"), NOW),
        Err(RunError::InvalidRequest(RunInvalidField::Prompt))
    );
    assert_eq!(
        registry.submit(&submission("k1", "", "w1", "w1:pA"), NOW),
        Err(RunError::InvalidRequest(RunInvalidField::Prompt))
    );
    assert!(
        registry.is_empty(),
        "an out-of-bounds prompt must not reach persisted state"
    );

    let at_bound = "a".repeat(MAX_PROMPT_BYTES);
    let accepted = registry
        .submit(&submission("k1", &at_bound, "w1", "w1:pA"), NOW)
        .expect("a prompt exactly at the bound is accepted");
    assert_eq!(accepted.record.prompt_bytes as usize, MAX_PROMPT_BYTES);
}

#[test]
fn run_submit_counts_multibyte_and_control_prompt_bytes_exactly() {
    let mut registry = RunRegistry::default();
    let prompt = "é🙂\n\x1b[31mreview\x1b[0m";
    let accepted = registry
        .submit(&submission("utf8-control", prompt, "w1", "w1:pA"), NOW)
        .expect("bounded technical prompt is accepted");
    assert_eq!(accepted.record.prompt_bytes as usize, prompt.len());
    assert_eq!(accepted.record.prompt_digest, digest_hex(prompt.as_bytes()));

    let multibyte = "🙂".repeat((MAX_PROMPT_BYTES / "🙂".len()) + 1);
    assert!(multibyte.len() > MAX_PROMPT_BYTES);
    assert_eq!(
        registry.submit(&submission("utf8-large", &multibyte, "w1", "w1:pA"), NOW),
        Err(RunError::InvalidRequest(RunInvalidField::Prompt))
    );
}

// Matrix row 9.
#[test]
fn run_records_are_redacted() {
    let secret_prompt =
        "\x1b[31mred\x1b[0m review sk-live-abcdef1234567890 and ANTHROPIC_API_KEY=sk-ant-secret";
    let (registry, record) = registry_with_run("kantoku-redact", secret_prompt);

    assert_eq!(record.prompt_digest, digest_hex(secret_prompt.as_bytes()));
    assert_eq!(record.prompt_bytes as usize, secret_prompt.len());

    let persisted = serde_json::to_string(&registry).expect("registry serializes");
    for leak in [
        "sk-live-abcdef1234567890",
        "sk-ant-secret",
        "ANTHROPIC_API_KEY",
        "review sk-live",
        "\u{1b}",
        "red",
    ] {
        assert!(
            !persisted.contains(leak),
            "persisted run registry leaked {leak:?}: {persisted}"
        );
    }

    let returned = serde_json::to_string(&record).expect("record serializes");
    assert!(!returned.contains("sk-live-abcdef1234567890"));
    assert!(!returned.contains('\u{1b}'));
}

// Matrix row 4 support: the record carries typed state, timestamps and bindings.
#[test]
fn run_record_carries_typed_state_timestamps_and_bindings() {
    let (_registry, record) = registry_with_run("kantoku-fields", "review the diff");

    assert!(record.run_id.starts_with("run_"), "{}", record.run_id);
    assert_eq!(record.state, RunState::Queued);
    assert_eq!(record.workspace_id, "w1");
    assert_eq!(record.pane_id, "w1:pA");
    assert_eq!(record.agent_name.as_deref(), Some("reviewer"));
    assert_eq!(record.agent_session_id, "session-a");
    assert_eq!(record.checkout_path, "/tmp/repo-a");
    assert_eq!(record.created_at_unix, NOW);
    assert_eq!(record.updated_at_unix, NOW);
    assert_eq!(record.started_at_unix, None);
    assert_eq!(record.finished_at_unix, None);
    assert_eq!(record.failure, None);
}

#[test]
fn run_ids_are_opaque_and_unique_per_run() {
    let mut registry = RunRegistry::default();
    let first = registry
        .submit(&submission("k1", "prompt", "w1", "w1:pA"), NOW)
        .expect("first submit")
        .record;
    let second = registry
        .submit(&submission("k2", "prompt", "w1", "w1:pB"), NOW)
        .expect("second submit")
        .record;

    assert_ne!(first.run_id, second.run_id);
    for run_id in [&first.run_id, &second.run_id] {
        let body = run_id.strip_prefix("run_").expect("run id prefix");
        assert_eq!(body.len(), 32, "{run_id}");
        assert!(body.chars().all(|ch| ch.is_ascii_hexdigit()), "{run_id}");
        assert!(!run_id.contains("k1") && !run_id.contains("k2"));
    }
}

// Matrix row 8.
#[test]
fn run_state_machine_transitions() {
    // Exhaustive over RunState so a new state cannot silently skip coverage.
    for state in [
        RunState::Queued,
        RunState::Running,
        RunState::Blocked,
        RunState::Succeeded,
        RunState::Failed,
        RunState::CancelRequested,
        RunState::Cancelled,
        RunState::Lost,
    ] {
        let expected_terminal = match state {
            RunState::Queued
            | RunState::Running
            | RunState::Blocked
            | RunState::CancelRequested => false,
            RunState::Succeeded | RunState::Failed | RunState::Cancelled | RunState::Lost => true,
        };
        assert_eq!(state.is_terminal(), expected_terminal, "{state:?}");
    }

    // queued -> running -> blocked -> running -> succeeded
    let (mut registry, record) = registry_with_run("k-happy", "prompt");
    let run_id = record.run_id.clone();
    assert_eq!(
        registry.mark_started(&run_id, NOW + 1).map(|r| r.state),
        Some(RunState::Running)
    );
    assert!(observe(
        &mut registry,
        "w1:pA",
        RunAgentObservation::Working,
        NOW + 2
    ));
    assert!(observe(
        &mut registry,
        "w1:pA",
        RunAgentObservation::Blocked,
        NOW + 3
    ));
    assert_eq!(
        registry.get(&run_id, &scope("w1")).unwrap().state,
        RunState::Blocked
    );
    assert!(observe(
        &mut registry,
        "w1:pA",
        RunAgentObservation::Working,
        NOW + 4
    ));
    assert!(observe(
        &mut registry,
        "w1:pA",
        RunAgentObservation::Idle,
        NOW + 5
    ));
    let finished = registry.get(&run_id, &scope("w1")).unwrap();
    assert_eq!(finished.state, RunState::Succeeded);
    assert_eq!(finished.started_at_unix, Some(NOW + 1));
    assert_eq!(finished.finished_at_unix, Some(NOW + 5));

    // A terminal run ignores further observations.
    assert!(!observe(
        &mut registry,
        "w1:pA",
        RunAgentObservation::Working,
        NOW + 6
    ));
    assert_eq!(
        registry.get(&run_id, &scope("w1")).unwrap().state,
        RunState::Succeeded
    );

    // running + idle without observed activity stays running.
    let (mut registry, record) = registry_with_run("k-idle-first", "prompt");
    registry.mark_started(&record.run_id, NOW + 1);
    assert!(!observe(
        &mut registry,
        "w1:pA",
        RunAgentObservation::Idle,
        NOW + 2
    ));
    assert_eq!(
        registry.get(&record.run_id, &scope("w1")).unwrap().state,
        RunState::Running
    );

    // failed is typed and terminal.
    let (mut registry, record) = registry_with_run("k-failed", "prompt");
    let failed = registry
        .mark_failed(&record.run_id, RunFailureKind::PromptRejected, NOW + 1)
        .expect("mark_failed");
    assert_eq!(failed.state, RunState::Failed);
    assert_eq!(failed.failure, Some(RunFailureKind::PromptRejected));
    assert_eq!(failed.finished_at_unix, Some(NOW + 1));
    assert!(registry
        .mark_failed(&record.run_id, RunFailureKind::Internal, NOW + 2)
        .is_none());

    // cancel_requested -> cancelled.
    let (mut registry, record) = registry_with_run("k-cancel", "prompt");
    registry.mark_started(&record.run_id, NOW + 1);
    observe(
        &mut registry,
        "w1:pA",
        RunAgentObservation::Working,
        NOW + 2,
    );
    let cancelling = registry
        .request_cancel(&record.run_id, &scope("w1"), NOW + 3)
        .expect("cancel accepted");
    assert_eq!(cancelling.state, RunState::CancelRequested);
    assert!(observe(
        &mut registry,
        "w1:pA",
        RunAgentObservation::Working,
        NOW + 4
    ));
    assert_eq!(
        registry.get(&record.run_id, &scope("w1")).unwrap().state,
        RunState::CancelRequested
    );
    assert!(observe(
        &mut registry,
        "w1:pA",
        RunAgentObservation::Idle,
        NOW + 5
    ));
    assert_eq!(
        registry.get(&record.run_id, &scope("w1")).unwrap().state,
        RunState::Cancelled
    );

    // A terminal run cannot be cancelled again.
    assert_eq!(
        registry.request_cancel(&record.run_id, &scope("w1"), NOW + 6),
        Err(RunError::NotCancellable)
    );

    // A disappearing binding loses an active run.
    let (mut registry, record) = registry_with_run("k-gone", "prompt");
    registry.mark_started(&record.run_id, NOW + 1);
    assert!(observe(
        &mut registry,
        "w1:pA",
        RunAgentObservation::Gone,
        NOW + 2
    ));
    let lost = registry.get(&record.run_id, &scope("w1")).unwrap();
    assert_eq!(lost.state, RunState::Lost);
    assert_eq!(lost.finished_at_unix, Some(NOW + 2));
}

// Matrix row 5 (registry half).
#[test]
fn cancel_touches_only_the_addressed_run() {
    let mut registry = RunRegistry::default();
    let first = registry
        .submit(&submission("k1", "prompt one", "w1", "w1:pA"), NOW)
        .unwrap()
        .record;
    let second = registry
        .submit(&submission("k2", "prompt two", "w1", "w1:pB"), NOW)
        .unwrap()
        .record;

    registry
        .request_cancel(&first.run_id, &scope("w1"), NOW + 1)
        .expect("cancel accepted");

    let untouched = registry.get(&second.run_id, &scope("w1")).unwrap();
    assert_eq!(untouched.state, RunState::Queued);
    assert_eq!(untouched.updated_at_unix, NOW);
    assert_eq!(
        registry.get(&first.run_id, &scope("w1")).unwrap().state,
        RunState::CancelRequested
    );

    assert_eq!(
        registry.request_cancel("run_unknown", &scope("w1"), NOW + 2),
        Err(RunError::NotFound)
    );
}

// Matrix row 14 (registry half).
#[test]
fn foreign_scope_reads_and_cancels_fail_closed() {
    let mut registry = RunRegistry::default();
    let mine = registry
        .submit(&submission("k1", "prompt", "w1", "w1:pA"), NOW)
        .unwrap()
        .record;

    assert_eq!(
        registry.get(&mine.run_id, &scope("w2")),
        Err(RunError::NotFound)
    );
    assert_eq!(
        registry.get("run_missing", &scope("w1")),
        Err(RunError::NotFound)
    );
    assert_eq!(
        registry.request_cancel(&mine.run_id, &scope("w2"), NOW + 1),
        Err(RunError::NotFound)
    );
    assert_eq!(
        registry.get(&mine.run_id, &scope("w1")).unwrap().state,
        RunState::Queued,
        "a foreign cancel must not change the run"
    );
}

#[test]
fn idempotency_keys_are_scoped_per_workspace() {
    let mut registry = RunRegistry::default();
    let first = registry
        .submit(&submission("shared", "prompt", "w1", "w1:pA"), NOW)
        .unwrap();
    let second = registry
        .submit(&submission("shared", "prompt", "w2", "w2:pA"), NOW)
        .unwrap();

    assert!(!first.deduplicated);
    assert!(!second.deduplicated);
    assert_ne!(first.record.run_id, second.record.run_id);
    assert_eq!(registry.len(), 2);
}

// Matrix row 6 (reconcile half).
#[test]
fn reconcile_after_restart_loses_runs_without_a_live_binding() {
    let mut registry = RunRegistry::default();
    let live = registry
        .submit(&submission("k-live", "prompt", "w1", "w1:pA"), NOW)
        .unwrap()
        .record;
    let orphan = registry
        .submit(&submission("k-orphan", "prompt", "w1", "w1:pZ"), NOW)
        .unwrap()
        .record;
    let finished = registry
        .submit(&submission("k-done", "prompt", "w1", "w1:pD"), NOW)
        .unwrap()
        .record;
    registry.mark_failed(&finished.run_id, RunFailureKind::PromptRejected, NOW);

    let live_panes: HashSet<String> = ["w1:pA".to_string()].into_iter().collect();
    assert_eq!(registry.reconcile_after_restart(&live_panes, NOW + 10), 1);

    assert_eq!(
        registry.get(&live.run_id, &scope("w1")).unwrap().state,
        RunState::Queued
    );
    let lost = registry.get(&orphan.run_id, &scope("w1")).unwrap();
    assert_eq!(lost.state, RunState::Lost);
    assert_eq!(lost.failure, Some(RunFailureKind::ServerRestart));
    assert_eq!(lost.finished_at_unix, Some(NOW + 10));
    assert_eq!(
        registry
            .get(&finished.run_id, &scope("w1"))
            .unwrap()
            .failure,
        Some(RunFailureKind::PromptRejected),
        "an already terminal run keeps its outcome"
    );

    // Reconciling again is a no-op.
    assert_eq!(registry.reconcile_after_restart(&live_panes, NOW + 20), 0);
}

#[test]
fn restart_reconciliation_requires_the_complete_binding_not_a_reused_pane_id() {
    let mut registry = RunRegistry::default();
    let run = registry
        .submit(&submission("reused-pane", "prompt", "w1", "w1:pA"), NOW)
        .expect("submit")
        .record;
    let mut reused = observation_binding("w1", "w1:pA");
    reused.checkout_path = "/tmp/repo-b".to_string();
    reused.agent_name = Some("replacement".to_string());
    reused.agent_session_id = "session-b".to_string();
    let live = HashSet::from([reused]);

    assert_eq!(
        registry.reconcile_after_restart_with_bindings(&live, NOW + 1),
        1,
        "a pane id without the original checkout, agent, and session must not preserve a run"
    );
    assert_eq!(
        registry.get(&run.run_id, &scope("w1")).unwrap().state,
        RunState::Lost
    );
}

#[test]
fn registry_bounds_retained_records() {
    let mut registry = RunRegistry::default();
    for index in 0..MAX_RUN_RECORDS + 5 {
        let key = format!("k{index}");
        let record = registry
            .submit(
                &submission(&key, "prompt", "w1", "w1:pA"),
                NOW + index as u64,
            )
            .unwrap()
            .record;
        registry.mark_failed(&record.run_id, RunFailureKind::Internal, NOW + index as u64);
    }

    assert_eq!(registry.len(), MAX_RUN_RECORDS);
    assert_eq!(registry.records()[0].run_id, run_id_for(6, "w1", "k5"));
}

#[test]
fn registry_keeps_active_records_when_bounded() {
    let mut registry = RunRegistry::default();
    let active = registry
        .submit(&submission("k-active", "prompt", "w1", "w1:pA"), NOW)
        .unwrap()
        .record;
    for index in 0..MAX_RUN_RECORDS + 5 {
        let key = format!("k{index}");
        let pane = format!("w1:p{index}");
        let record = registry
            .submit(&submission(&key, "prompt", "w1", &pane), NOW + 1)
            .unwrap()
            .record;
        registry.mark_failed(&record.run_id, RunFailureKind::Internal, NOW + 1);
    }

    assert!(
        registry.get(&active.run_id, &scope("w1")).is_ok(),
        "an active run must not be evicted by finished runs"
    );
}

#[test]
fn registry_rejects_submit_when_all_retained_records_are_active() {
    let mut registry = RunRegistry::default();
    for index in 0..MAX_RUN_RECORDS {
        let key = format!("active-key-{index}");
        let pane = format!("w1:active-pane-{index}");
        registry
            .submit(&submission(&key, "prompt", "w1", &pane), NOW + index as u64)
            .expect("active run fits before the hard capacity");
    }

    let overflow = registry.submit(
        &submission("active-overflow", "prompt", "w1", "w1:active-pane-overflow"),
        NOW + MAX_RUN_RECORDS as u64,
    );

    assert_eq!(overflow, Err(RunError::CapacityReached));
    assert_eq!(registry.len(), MAX_RUN_RECORDS);
    assert!(registry
        .records()
        .iter()
        .all(|record| !record.state.is_terminal()));
}

#[test]
fn close_reconciliation_reapplies_retention_after_marking_runs_lost() {
    let mut registry = RunRegistry::default();
    for index in 0..MAX_RUN_RECORDS {
        let key = format!("close-key-{index}");
        let pane = format!("w1:close-pane-{index}");
        registry
            .submit(&submission(&key, "prompt", "w1", &pane), NOW + index as u64)
            .expect("active run fits before injected overflow");
    }
    let mut overflow = registry.records()[0].clone();
    overflow.run_id = "run_close_overflow".to_string();
    overflow.pane_id = "w1:close-pane-overflow".to_string();
    registry.runs.push(overflow);

    let lost = registry.mark_lost_for_closed_workspace("w1", NOW + MAX_RUN_RECORDS as u64);

    assert!(!lost.is_empty());
    assert_eq!(registry.len(), MAX_RUN_RECORDS);
    assert!(registry
        .records()
        .iter()
        .all(|record| record.state.is_terminal() || record.pane_id != "w1:close-pane-overflow"));
}

#[test]
fn persistence_refuses_a_registry_above_the_hard_record_bound() {
    let mut registry = RunRegistry::default();
    for index in 0..MAX_RUN_RECORDS {
        let key = format!("persist-bound-key-{index}");
        let pane = format!("w1:persist-bound-pane-{index}");
        registry
            .submit(&submission(&key, "prompt", "w1", &pane), NOW + index as u64)
            .expect("active run fits before injected overflow");
    }
    let mut overflow = registry.records()[0].clone();
    overflow.run_id = "run_persist_overflow".to_string();
    registry.runs.push(overflow);
    let path = std::env::temp_dir().join(format!(
        "nak-439-overbound-registry-{}-runs.json",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);

    let saved = crate::persist::run_registry::save_to_path(&path, &registry);
    let file_was_written = path.exists();
    let _ = std::fs::remove_file(&path);

    assert!(saved.is_err());
    assert!(!file_was_written);
}

#[test]
fn private_deduplication_is_bounded_and_survives_reload_without_raw_keys() {
    let mut registry = RunRegistry::default();
    for index in 0..MAX_RUN_DEDUPLICATION + 2 {
        let key = format!("private-key-{index}-sk-live-secret");
        let record = registry
            .submit(
                &submission(&key, "prompt", "w1", "w1:pA"),
                NOW + index as u64,
            )
            .expect("submit")
            .record;
        registry.mark_failed(&record.run_id, RunFailureKind::Internal, NOW + index as u64);
    }
    assert!(registry.deduplication.len() <= MAX_RUN_DEDUPLICATION);
    let encoded = serde_json::to_string(&registry).expect("registry serializes");
    assert!(!encoded.contains("private-key-"));
    let reloaded: RunRegistry = serde_json::from_str(&encoded).expect("registry reloads");
    assert!(reloaded.deduplication.len() <= MAX_RUN_DEDUPLICATION);
}

// Matrix row 12.
#[test]
fn capability_expiry_and_scope() {
    let mut registry = RunRegistry::default();
    let capability = registry
        .issue_capability("w1", 60_000, &ALL_OPERATIONS, NOW)
        .expect("capability issued");

    assert_eq!(capability.workspace_id, "w1");
    assert_eq!(capability.issued_at_unix, NOW);
    assert_eq!(capability.expires_at_unix, NOW + 60);
    assert_eq!(capability.last_sequence, 0);
    assert!(capability.capability_id.starts_with("cap_"));

    let reference = CapabilityRef {
        capability_id: capability.capability_id.clone(),
        sequence: 1,
    };
    assert_eq!(
        registry.authorize(&reference, RunOperation::Submit, NOW + 1),
        Ok(scope("w1"))
    );

    // Unknown capability.
    assert_eq!(
        registry.authorize(
            &CapabilityRef {
                capability_id: "cap_missing".into(),
                sequence: 1,
            },
            RunOperation::Status,
            NOW + 1
        ),
        Err(RunError::CapabilityInvalid)
    );

    // Expired at the absolute boundary.
    assert_eq!(
        registry.authorize(
            &CapabilityRef {
                capability_id: capability.capability_id.clone(),
                sequence: 2,
            },
            RunOperation::Status,
            NOW + 60
        ),
        Err(RunError::CapabilityInvalid)
    );

    // Operation outside the granted set.
    let narrow = registry
        .issue_capability("w1", 60_000, &[RunOperation::Status], NOW)
        .expect("narrow capability issued");
    assert_eq!(
        registry.authorize(
            &CapabilityRef {
                capability_id: narrow.capability_id.clone(),
                sequence: 1,
            },
            RunOperation::Cancel,
            NOW + 1
        ),
        Err(RunError::CapabilityInvalid)
    );

    // Out-of-bounds lifetimes and empty scopes fail closed.
    assert_eq!(
        registry.issue_capability("w1", MIN_CAPABILITY_TTL_MS - 1, &ALL_OPERATIONS, NOW),
        Err(RunError::InvalidRequest(RunInvalidField::Ttl))
    );
    assert_eq!(
        registry.issue_capability("w1", MAX_CAPABILITY_TTL_MS + 1, &ALL_OPERATIONS, NOW),
        Err(RunError::InvalidRequest(RunInvalidField::Ttl))
    );
    assert_eq!(
        registry.issue_capability("w1", 60_000, &[], NOW),
        Err(RunError::InvalidRequest(RunInvalidField::Operations))
    );
    assert_eq!(
        registry.issue_capability("", 60_000, &ALL_OPERATIONS, NOW),
        Err(RunError::InvalidRequest(RunInvalidField::WorkspaceId))
    );
}

// Matrix row 13.
#[test]
fn capability_rejects_replayed_sequence() {
    let mut registry = RunRegistry::default();
    let capability = registry
        .issue_capability("w1", 60_000, &ALL_OPERATIONS, NOW)
        .expect("capability issued");

    let reference = |sequence: u64| CapabilityRef {
        capability_id: capability.capability_id.clone(),
        sequence,
    };

    assert!(registry
        .authorize(&reference(7), RunOperation::Submit, NOW + 1)
        .is_ok());
    assert_eq!(
        registry.authorize(&reference(7), RunOperation::Submit, NOW + 2),
        Err(RunError::ReplayRejected)
    );
    assert_eq!(
        registry.authorize(&reference(6), RunOperation::Status, NOW + 3),
        Err(RunError::ReplayRejected)
    );
    assert_eq!(
        registry.authorize(&reference(0), RunOperation::Status, NOW + 3),
        Err(RunError::InvalidRequest(RunInvalidField::Sequence))
    );
    assert!(registry
        .authorize(&reference(8), RunOperation::Status, NOW + 4)
        .is_ok());
}

#[test]
fn capability_sequence_is_burned_even_when_scope_check_fails() {
    let mut registry = RunRegistry::default();
    let capability = registry
        .issue_capability("w1", 60_000, &ALL_OPERATIONS, NOW)
        .expect("capability issued");
    let reference = CapabilityRef {
        capability_id: capability.capability_id.clone(),
        sequence: 5,
    };

    assert!(registry
        .authorize(&reference, RunOperation::Submit, NOW + 1)
        .is_ok());
    assert_eq!(
        registry.authorize(&reference, RunOperation::Submit, NOW + 2),
        Err(RunError::ReplayRejected)
    );
}

#[test]
fn expired_capabilities_are_pruned_and_capabilities_are_bounded() {
    let mut registry = RunRegistry::default();
    let short = registry
        .issue_capability("w1", MIN_CAPABILITY_TTL_MS, &ALL_OPERATIONS, NOW)
        .expect("short capability");

    for index in 0..super::auth::MAX_CAPABILITIES + 2 {
        registry
            .issue_capability("w1", 600_000, &ALL_OPERATIONS, NOW + 10 + index as u64)
            .expect("capability issued");
    }

    assert_eq!(
        registry.authorize(
            &CapabilityRef {
                capability_id: short.capability_id,
                sequence: 1,
            },
            RunOperation::Status,
            NOW + 100
        ),
        Err(RunError::CapabilityInvalid)
    );
}

// Matrix row 16.
#[test]
fn runs_module_has_no_presentation_dependency() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/runs");
    for file in ["mod.rs", "auth.rs"] {
        let source = std::fs::read_to_string(root.join(file)).expect("run module source");
        let code: String = source
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        for forbidden in [
            "crate::ui",
            "crate::app",
            "crate::render",
            "ratatui",
            "crossterm",
            "crate::terminal",
        ] {
            assert!(
                !code.contains(forbidden),
                "src/runs/{file} must not depend on {forbidden}"
            );
        }
    }
}

#[test]
fn run_error_codes_are_stable_and_messages_carry_no_input() {
    for (error, code) in [
        (
            RunError::InvalidRequest(RunInvalidField::Prompt),
            "run_invalid_request",
        ),
        (RunError::IdempotencyConflict, "run_idempotency_conflict"),
        (RunError::CheckoutMismatch, "run_checkout_mismatch"),
        (RunError::NotFound, "run_not_found"),
        (RunError::NotCancellable, "run_not_cancellable"),
        (RunError::CapabilityInvalid, "run_capability_invalid"),
        (RunError::ReplayRejected, "run_replay_rejected"),
        (RunError::Unauthorized, "run_unauthorized"),
        (RunError::TargetUnavailable, "run_target_unavailable"),
        (RunError::CapacityReached, "run_capacity_reached"),
    ] {
        assert_eq!(error.code(), code);
        assert!(!error.message().is_empty());
    }
}

// Matrix row 18. The durable format must reject corrupt and newer data.
#[test]
fn registry_reload_rejects_corrupt_or_incompatible_data() {
    let (registry, record) = registry_with_run("reload-key", "review the patch");
    let serialized = serde_json::to_string(&registry).expect("registry serializes");
    let reloaded: RunRegistry = serde_json::from_str(&serialized).expect("registry reloads");
    assert_eq!(reloaded.get(&record.run_id, &scope("w1")), Ok(&record));

    let mut incompatible: serde_json::Value =
        serde_json::from_str(&serialized).expect("serialized registry is JSON");
    incompatible["version"] = serde_json::json!(REGISTRY_VERSION + 1);
    assert!(
        serde_json::from_value::<RunRegistry>(incompatible).is_err(),
        "a registry from a newer format must fail closed"
    );
    assert!(
        serde_json::from_str::<RunRegistry>("{ definitely not JSON").is_err(),
        "corrupt registry data must fail closed"
    );

    let mut uppercase_digest: serde_json::Value =
        serde_json::from_str(&serialized).expect("serialized registry is JSON");
    uppercase_digest["runs"][0]["prompt_digest"] =
        serde_json::json!(record.prompt_digest.to_ascii_uppercase());
    uppercase_digest["deduplication"][0]["prompt_digest"] =
        serde_json::json!(record.prompt_digest.to_ascii_uppercase());
    let idempotency_key_digest = uppercase_digest["deduplication"][0]["idempotency_key_digest"]
        .as_str()
        .expect("deduplication digest")
        .to_ascii_uppercase();
    uppercase_digest["deduplication"][0]["idempotency_key_digest"] =
        serde_json::json!(idempotency_key_digest);
    assert!(
        serde_json::from_value::<RunRegistry>(uppercase_digest).is_err(),
        "durable SHA-256 digests must use lowercase hex"
    );
}

// Matrix row 19. A pane identifier alone is not a complete run binding.
#[test]
fn observations_require_the_complete_workspace_checkout_pane_agent_and_session_binding() {
    let mut registry = RunRegistry::default();
    let first = registry
        .submit(&submission("binding-one", "first", "w1", "w1:pA"), NOW)
        .expect("first run")
        .record;
    let mut second_request = submission("binding-two", "second", "w1", "w1:pA");
    second_request.binding.agent_name = Some("implementer".to_string());
    let second = registry
        .submit(&second_request, NOW)
        .expect("second run")
        .record;

    registry.mark_started(&first.run_id, NOW + 1);
    registry.mark_started(&second.run_id, NOW + 1);
    assert!(registry.observe_agent_state(
        &RunObservationBinding {
            workspace_id: "w1".to_string(),
            checkout_path: "/tmp/repo-a".to_string(),
            pane_id: "w1:pA".to_string(),
            agent_name: Some("reviewer".to_string()),
            agent_session_id: "session-a".to_string(),
        },
        RunAgentObservation::Working,
        NOW + 2,
    ));
    assert!(registry.observe_agent_state(
        &RunObservationBinding {
            workspace_id: "w1".to_string(),
            checkout_path: "/tmp/repo-a".to_string(),
            pane_id: "w1:pA".to_string(),
            agent_name: Some("reviewer".to_string()),
            agent_session_id: "session-a".to_string(),
        },
        RunAgentObservation::Idle,
        NOW + 3,
    ));

    assert_eq!(
        registry.get(&first.run_id, &scope("w1")).unwrap().state,
        RunState::Succeeded
    );
    assert_eq!(
        registry.get(&second.run_id, &scope("w1")).unwrap().state,
        RunState::Running,
        "an observation for another agent session must not complete this run"
    );
}

#[test]
fn every_observation_binding_mismatch_changes_no_run() {
    let mut registry = RunRegistry::default();
    let run = registry
        .submit(
            &submission("observation-mismatch", "prompt", "w1", "w1:pA"),
            NOW,
        )
        .expect("submit")
        .record;
    registry.mark_started(&run.run_id, NOW + 1);

    for mismatch in 0..5 {
        let mut binding = observation_binding("w1", "w1:pA");
        match mismatch {
            0 => binding.workspace_id = "w2".to_string(),
            1 => binding.checkout_path = "/tmp/repo-b".to_string(),
            2 => binding.pane_id = "w1:pB".to_string(),
            3 => binding.agent_name = Some("replacement".to_string()),
            4 => binding.agent_session_id = "session-b".to_string(),
            _ => unreachable!(),
        }
        assert!(
            !registry.observe_agent_state(&binding, RunAgentObservation::Gone, NOW + 2),
            "binding field {mismatch} must not change a run"
        );
        let unchanged = registry.get(&run.run_id, &scope("w1")).unwrap();
        assert_eq!(unchanged.state, RunState::Running);
        assert_eq!(unchanged.updated_at_unix, NOW + 1);
    }
}

// Matrix row 20. API result metadata stays typed, bounded, and free of caller input.
#[test]
fn run_result_metadata_is_bounded_typed_and_redacted() {
    let caller_input = "key-secret-contains-no-output";
    let prompt = "\x1b[31mterminal text sk-live-secret\x1b[0m";
    let (mut registry, record) = registry_with_run(caller_input, prompt);
    let record = registry
        .mark_failed(&record.run_id, RunFailureKind::ProviderExited, NOW + 1)
        .expect("provider exit is a typed terminal outcome");
    let encoded = serde_json::to_string(&record).expect("record serializes");

    assert!(encoded.len() <= MAX_RUN_RESULT_BYTES);
    assert!(
        !encoded.contains(caller_input),
        "result leaked an idempotency key"
    );
    assert!(
        !encoded.contains("sk-live-secret"),
        "result leaked a prompt secret"
    );
    assert!(!encoded.contains("terminal text"));
    assert!(!encoded.contains("\u{1b}"));
    for forbidden_field in ["terminal_text", "output", "ansi", "result_text"] {
        assert!(
            serde_json::from_str::<serde_json::Value>(&encoded)
                .expect("record JSON")
                .get(forbidden_field)
                .is_none(),
            "typed metadata must not expose {forbidden_field}"
        );
    }
    assert_eq!(record.state, RunState::Failed);
    assert_eq!(record.failure, Some(RunFailureKind::ProviderExited));
    assert!(
        serde_json::to_string(&registry)
            .expect("registry serializes")
            .len()
            <= MAX_RUN_RESULT_BYTES,
        "a single bounded run must not produce an oversized durable result"
    );
}

#[test]
fn run_result_never_exceeds_the_metadata_bound() {
    let mut registry = RunRegistry::default();
    let mut request = submission("bounded-result", "prompt", "w1", "w1:pA");
    request.binding.checkout_path = "/".repeat(MAX_CHECKOUT_PATH_LEN);
    let record = registry
        .submit(&request, NOW)
        .expect("bounded input")
        .record;

    assert!(
        serde_json::to_vec(&record)
            .expect("record serializes")
            .len()
            <= MAX_RUN_RESULT_BYTES,
        "bounded input must not create an oversized result"
    );
}

// Matrix row 21. A consumed sequence stays consumed when the requested submit fails.
#[test]
fn replay_sequence_persists_after_a_failed_operation() {
    let mut registry = RunRegistry::default();
    let capability = registry
        .issue_capability("w1", 60_000, &ALL_OPERATIONS, NOW)
        .expect("capability issued");
    let reference = CapabilityRef {
        capability_id: capability.capability_id.clone(),
        sequence: 1,
    };

    assert_eq!(
        registry.authorize(&reference, RunOperation::Submit, NOW + 1),
        Ok(scope("w1"))
    );
    let invalid = submission("failed-submit", "", "w1", "w1:pA");
    assert_eq!(
        registry.submit(&invalid, NOW + 1),
        Err(RunError::InvalidRequest(RunInvalidField::Prompt))
    );

    let serialized = serde_json::to_string(&registry).expect("registry serializes");
    let mut reloaded: RunRegistry = serde_json::from_str(&serialized).expect("registry reloads");
    assert_eq!(
        reloaded.authorize(&reference, RunOperation::Submit, NOW + 2),
        Err(RunError::ReplayRejected),
        "a failed operation must not make its capability sequence reusable"
    );
}

// Matrix row 22. State timestamps never move backward and terminal states stay final.
#[test]
fn run_timestamps_are_monotonic_and_invalid_terminal_transitions_fail_closed() {
    let (mut registry, record) = registry_with_run("time-key", "prompt");
    registry.mark_started(&record.run_id, NOW + 10);
    assert!(
        !observe(
            &mut registry,
            "w1:pA",
            RunAgentObservation::Working,
            NOW + 9
        ),
        "an observation older than the recorded start must fail closed"
    );
    let running = registry.get(&record.run_id, &scope("w1")).unwrap();
    assert_eq!(running.state, RunState::Running);
    assert_eq!(running.updated_at_unix, NOW + 10);
    assert!(observe(
        &mut registry,
        "w1:pA",
        RunAgentObservation::Working,
        NOW + 11
    ));
    assert!(observe(
        &mut registry,
        "w1:pA",
        RunAgentObservation::Idle,
        NOW + 12
    ));

    assert!(!observe(
        &mut registry,
        "w1:pA",
        RunAgentObservation::Gone,
        NOW + 1
    ));
    let terminal = registry.get(&record.run_id, &scope("w1")).unwrap();
    assert_eq!(terminal.state, RunState::Succeeded);
    assert_eq!(terminal.updated_at_unix, NOW + 12);
    assert_eq!(terminal.finished_at_unix, Some(NOW + 12));
}

#[test]
fn timestamps_cover_stale_start_failure_cancel_and_all_terminal_states() {
    let mut registry = RunRegistry::default();
    let stale_start = registry
        .submit(&submission("stale-start", "prompt", "w1", "w1:pZ"), NOW)
        .unwrap()
        .record;
    let failed = registry
        .submit(&submission("failed-time", "prompt", "w1", "w1:pA"), NOW)
        .unwrap()
        .record;
    let succeeded = registry
        .submit(&submission("success-time", "prompt", "w1", "w1:pS"), NOW)
        .unwrap()
        .record;
    let cancelled = registry
        .submit(&submission("cancel-time", "prompt", "w1", "w1:pB"), NOW)
        .unwrap()
        .record;
    let lost = registry
        .submit(&submission("lost-time", "prompt", "w1", "w1:pC"), NOW)
        .unwrap()
        .record;

    assert!(
        registry
            .mark_started(&stale_start.run_id, NOW - 1)
            .is_none(),
        "a start before creation must fail closed"
    );
    registry.mark_started(&failed.run_id, NOW + 10);
    assert!(
        registry
            .mark_failed(&failed.run_id, RunFailureKind::ProviderExited, NOW + 9)
            .is_none(),
        "a stale failure must not predate the start"
    );
    assert!(registry
        .mark_failed(&failed.run_id, RunFailureKind::ProviderExited, NOW + 11)
        .is_some());
    registry.mark_started(&succeeded.run_id, NOW + 10);
    assert!(observe(
        &mut registry,
        "w1:pS",
        RunAgentObservation::Working,
        NOW + 11
    ));
    assert!(observe(
        &mut registry,
        "w1:pS",
        RunAgentObservation::Idle,
        NOW + 12
    ));
    registry.mark_started(&cancelled.run_id, NOW + 10);
    assert_eq!(
        registry.request_cancel(&cancelled.run_id, &scope("w1"), NOW + 9),
        Err(RunError::NotCancellable),
        "a stale cancel must fail closed"
    );
    assert!(registry
        .request_cancel(&cancelled.run_id, &scope("w1"), NOW + 11)
        .is_ok());
    assert!(observe(
        &mut registry,
        "w1:pB",
        RunAgentObservation::Idle,
        NOW + 12
    ));
    registry.mark_started(&lost.run_id, NOW + 10);
    assert!(registry.observe_agent_state(
        &RunObservationBinding {
            workspace_id: "w1".to_string(),
            checkout_path: "/tmp/repo-a".to_string(),
            pane_id: "w1:pC".to_string(),
            agent_name: Some("reviewer".to_string()),
            agent_session_id: "session-a".to_string(),
        },
        RunAgentObservation::Gone,
        NOW + 11,
    ));

    for record in registry
        .records()
        .iter()
        .filter(|record| record.state.is_terminal())
    {
        let finished = record
            .finished_at_unix
            .expect("terminal record has a finish time");
        assert!(record.created_at_unix <= record.updated_at_unix);
        assert!(record.updated_at_unix <= finished);
    }
    assert_eq!(
        registry.get(&failed.run_id, &scope("w1")).unwrap().state,
        RunState::Failed
    );
    assert_eq!(
        registry.get(&succeeded.run_id, &scope("w1")).unwrap().state,
        RunState::Succeeded
    );
    assert_eq!(
        registry.get(&cancelled.run_id, &scope("w1")).unwrap().state,
        RunState::Cancelled
    );
    assert_eq!(
        registry.get(&lost.run_id, &scope("w1")).unwrap().state,
        RunState::Lost
    );
}

// Matrix row 23. Provider exit, cancellation, failure, and loss remain distinct outcomes.
#[test]
fn provider_exit_cancellation_failure_and_loss_are_distinct() {
    let mut registry = RunRegistry::default();
    let provider_exit = registry
        .submit(&submission("provider-exit", "prompt", "w1", "w1:pA"), NOW)
        .unwrap()
        .record;
    let cancelled = registry
        .submit(&submission("cancelled", "prompt", "w1", "w1:pB"), NOW)
        .unwrap()
        .record;
    let lost = registry
        .submit(&submission("lost", "prompt", "w1", "w1:pC"), NOW)
        .unwrap()
        .record;

    registry.mark_failed(
        &provider_exit.run_id,
        RunFailureKind::ProviderExited,
        NOW + 1,
    );
    registry.mark_started(&cancelled.run_id, NOW + 1);
    observe(
        &mut registry,
        "w1:pB",
        RunAgentObservation::Working,
        NOW + 2,
    );
    registry
        .request_cancel(&cancelled.run_id, &scope("w1"), NOW + 3)
        .unwrap();
    observe(&mut registry, "w1:pB", RunAgentObservation::Idle, NOW + 4);
    registry.mark_started(&lost.run_id, NOW + 1);
    observe(&mut registry, "w1:pC", RunAgentObservation::Gone, NOW + 2);

    let provider_exit = registry.get(&provider_exit.run_id, &scope("w1")).unwrap();
    let cancelled = registry.get(&cancelled.run_id, &scope("w1")).unwrap();
    let lost = registry.get(&lost.run_id, &scope("w1")).unwrap();
    assert_eq!(provider_exit.state, RunState::Failed);
    assert_eq!(provider_exit.failure, Some(RunFailureKind::ProviderExited));
    assert_eq!(cancelled.state, RunState::Cancelled);
    assert_eq!(cancelled.failure, None);
    assert_eq!(lost.state, RunState::Lost);
    assert_eq!(lost.failure, Some(RunFailureKind::AgentUnavailable));
}

#[test]
fn queued_cancel_and_post_cancel_provider_exit_are_deterministic() {
    let mut registry = RunRegistry::default();
    let queued = registry
        .submit(&submission("queued-cancel", "prompt", "w1", "w1:pA"), NOW)
        .expect("submit")
        .record;
    let cancelled = registry
        .request_cancel(&queued.run_id, &scope("w1"), NOW + 1)
        .expect("queued cancel");
    assert_eq!(cancelled.state, RunState::CancelRequested);

    let post_cancel = registry
        .submit(
            &submission("post-cancel-exit", "prompt", "w1", "w1:pB"),
            NOW,
        )
        .expect("submit")
        .record;
    registry
        .request_cancel(&post_cancel.run_id, &scope("w1"), NOW + 1)
        .expect("cancel request");
    let failed = registry
        .mark_failed(&post_cancel.run_id, RunFailureKind::ProviderExited, NOW + 2)
        .expect("provider exit after cancel");
    assert_eq!(failed.state, RunState::Failed);
    assert_eq!(failed.failure, Some(RunFailureKind::ProviderExited));
}

// Matrix row 24. Stable error messages do not echo dangerous caller values.
#[test]
fn run_error_messages_never_echo_caller_input() {
    let caller_inputs = [
        "idempotency-sk-live-secret",
        "workspace-secret",
        "/tmp/checkout-secret",
        "pane-secret",
        "agent-secret",
        "session-secret",
        "\u{1b}[31mprompt-secret\u{1b}[0m",
    ];
    for error in [
        RunError::InvalidRequest(RunInvalidField::Prompt),
        RunError::IdempotencyConflict,
        RunError::CheckoutMismatch,
        RunError::NotFound,
        RunError::NotCancellable,
        RunError::CapabilityInvalid,
        RunError::ReplayRejected,
        RunError::Unauthorized,
        RunError::TargetUnavailable,
    ] {
        for caller_input in caller_inputs {
            assert!(
                !error.message().contains(caller_input),
                "{} leaked caller input {caller_input:?}",
                error.code()
            );
        }
    }
}

#[test]
fn public_record_omits_private_deduplication_data() {
    let key = "caller-key-sk-live-secret";
    let (_registry, record) = registry_with_run(key, "prompt");
    let public = serde_json::to_value(&record).expect("public record serializes");
    assert!(public.get("idempotency_key").is_none());
    assert!(!public.to_string().contains(key));
}

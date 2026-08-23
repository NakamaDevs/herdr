use crate::runs::RunRegistry;
#[cfg(test)]
use crate::runs::{RunBinding, RunError, RunInvalidField, RunSubmission, REGISTRY_VERSION};

/// Atomically save the durable run registry to `path`.
pub fn save_to_path(path: &std::path::Path, registry: &RunRegistry) -> std::io::Result<()> {
    use std::io::Write as _;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let encoded = serde_json::to_vec(registry)?;
    serde_json::from_slice::<RunRegistry>(&encoded).map_err(std::io::Error::other)?;
    let temporary = path.with_extension("json.tmp");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)?;
    if let Err(error) = file
        .write_all(&encoded)
        .and_then(|()| file.flush())
        .and_then(|()| file.sync_all())
    {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    if let Err(error) = crate::platform::replace_file_atomically(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    crate::platform::sync_parent_directory(path)?;
    Ok(())
}

/// Load and validate the durable run registry from `path`.
pub fn load_from_path(path: &std::path::Path) -> std::io::Result<RunRegistry> {
    let encoded = std::fs::read(path)?;
    serde_json::from_slice(&encoded).map_err(std::io::Error::other)
}

/// Active-session durable registry path.
pub fn session_path() -> std::path::PathBuf {
    crate::session::data_dir().join("runs.json")
}

#[cfg(test)]
fn saved_registry() -> (RunRegistry, String) {
    let mut registry = RunRegistry::default();
    let record = registry
        .submit(
            &RunSubmission {
                idempotency_key: "persist-key".to_string(),
                prompt: "review the patch".to_string(),
                binding: RunBinding {
                    workspace_id: "w1".to_string(),
                    checkout_path: "/tmp/repo-a".to_string(),
                    pane_id: "w1:p1".to_string(),
                    agent_name: Some("reviewer".to_string()),
                    agent_session_id: "session-a".to_string(),
                },
            },
            1_700_000_000,
        )
        .expect("submit")
        .record;
    (registry, record.run_id)
}

// Matrix row 18. Registry data must reload exactly and reject corrupt or future formats.
#[test]
fn durable_registry_serializes_reloads_and_rejects_invalid_data() {
    let (registry, run_id) = saved_registry();
    let json = serde_json::to_string(&registry).expect("serialize registry");
    let reloaded: RunRegistry = serde_json::from_str(&json).expect("reload registry");
    assert_eq!(reloaded.records().len(), 1);
    assert_eq!(reloaded.records()[0].run_id, run_id);

    let mut incompatible: serde_json::Value = serde_json::from_str(&json).unwrap();
    incompatible["version"] = serde_json::json!(REGISTRY_VERSION + 1);
    assert!(
        serde_json::from_value::<RunRegistry>(incompatible).is_err(),
        "newer registry data must not load as a compatible registry"
    );
    assert!(serde_json::from_str::<RunRegistry>("not-json").is_err());
    assert_eq!(
        RunRegistry::default().submit(
            &RunSubmission {
                idempotency_key: "bad".to_string(),
                prompt: String::new(),
                binding: RunBinding {
                    workspace_id: "w1".to_string(),
                    checkout_path: "/tmp/repo-a".to_string(),
                    pane_id: "w1:p1".to_string(),
                    agent_name: Some("reviewer".to_string()),
                    agent_session_id: "session-a".to_string(),
                },
            },
            1,
        ),
        Err(RunError::InvalidRequest(RunInvalidField::Prompt))
    );
}

#[test]
fn durable_registry_atomic_save_load_reconcile_and_replay_contract() {
    let path = std::env::temp_dir().join(format!("nak-439-runs-{}", std::process::id()));
    let (mut registry, _) = saved_registry();
    let capability = registry
        .issue_capability(
            "w1",
            60_000,
            &[
                crate::runs::auth::RunOperation::Submit,
                crate::runs::auth::RunOperation::Status,
            ],
            1_700_000_000,
        )
        .expect("capability issued");
    let replay = crate::runs::auth::CapabilityRef {
        capability_id: capability.capability_id,
        sequence: 1,
    };
    assert!(registry
        .authorize(
            &replay,
            crate::runs::auth::RunOperation::Submit,
            1_700_000_001
        )
        .is_ok());
    save_to_path(&path, &registry).expect("atomic save");
    let mut loaded = load_from_path(&path).expect("atomic load");
    assert_eq!(loaded.records().len(), 1);
    assert_eq!(
        loaded.reconcile_after_restart(&Default::default(), 1_700_000_001),
        1
    );
    assert_eq!(
        loaded.authorize(
            &replay,
            crate::runs::auth::RunOperation::Submit,
            1_700_000_002
        ),
        Err(RunError::ReplayRejected),
        "persisted replay state must remain consumed after reload"
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn durable_registry_save_load_preserves_idempotent_retry_and_private_deduplication() {
    let path = std::env::temp_dir().join(format!("nak-439-retry-{}", std::process::id()));
    let (registry, run_id) = saved_registry();
    save_to_path(&path, &registry).expect("save registry");
    let mut loaded = load_from_path(&path).expect("load registry");
    let retry = loaded
        .submit(
            &RunSubmission {
                idempotency_key: "persist-key".to_string(),
                prompt: "review the patch".to_string(),
                binding: RunBinding {
                    workspace_id: "w1".to_string(),
                    checkout_path: "/tmp/repo-a".to_string(),
                    pane_id: "w1:p1".to_string(),
                    agent_name: Some("reviewer".to_string()),
                    agent_session_id: "session-a".to_string(),
                },
            },
            1_700_000_001,
        )
        .expect("retry after reload");
    assert!(retry.deduplicated);
    assert_eq!(retry.record.run_id, run_id);
    assert!(!serde_json::to_string(&loaded)
        .expect("registry serializes")
        .contains("persist-key"));
    let _ = std::fs::remove_file(path);
}

#[test]
fn atomic_registry_save_replaces_only_valid_data_and_ignores_stale_temporary_files() {
    let directory = std::env::temp_dir().join(format!("nak-439-atomic-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("temporary directory");
    let path = directory.join("runs.json");
    let stale = directory.join("runs.json.tmp-stale");
    std::fs::write(&stale, b"corrupt temporary data").expect("stale temporary file");

    let (first, first_id) = saved_registry();
    save_to_path(&path, &first).expect("initial atomic save");
    let mut replacement = RunRegistry::default();
    let replacement_id = replacement
        .submit(
            &RunSubmission {
                idempotency_key: "replacement-key".to_string(),
                prompt: "replacement".to_string(),
                binding: RunBinding {
                    workspace_id: "w1".to_string(),
                    checkout_path: "/tmp/repo-a".to_string(),
                    pane_id: "w1:p2".to_string(),
                    agent_name: Some("reviewer".to_string()),
                    agent_session_id: "session-b".to_string(),
                },
            },
            1,
        )
        .expect("replacement registry")
        .record
        .run_id;
    save_to_path(&path, &replacement).expect("replacement atomic save");
    let loaded = load_from_path(&path).expect("load replacement");
    assert_eq!(loaded.records()[0].run_id, replacement_id);
    assert_ne!(loaded.records()[0].run_id, first_id);

    let blocked_path = directory.join("blocked");
    std::fs::create_dir(&blocked_path).expect("blocked save target");
    assert!(save_to_path(&blocked_path, &replacement).is_err());
    assert!(
        load_from_path(&path).is_ok(),
        "a failed save must preserve the last valid file"
    );

    std::fs::write(&path, b"corrupt durable data").expect("corrupt durable file");
    assert!(
        load_from_path(&path).is_err(),
        "corrupt data must fail closed"
    );
    std::fs::write(
        &path,
        format!(r#"{{"version":{},"runs":[]}}"#, REGISTRY_VERSION + 1),
    )
    .expect("future durable file");
    assert!(
        load_from_path(&path).is_err(),
        "future data must fail closed"
    );
    let _ = std::fs::remove_dir_all(directory);
}

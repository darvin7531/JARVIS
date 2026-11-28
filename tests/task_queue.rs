use jarvis::storage::{SqliteStore, Task};
use rusqlite::Connection;

#[test]
fn queue_and_resource_claim_survive_restart_and_exclude_conflicts() {
    let dir = std::env::temp_dir().join(format!("jarvis-queue-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut store = SqliteStore::create(&dir).unwrap();
    for id in ["first", "second"] {
        store
            .create_task(
                &Task {
                    id: id.into(),
                    version: 1,
                    title: id.into(),
                },
                "created",
                &[],
            )
            .unwrap();
    }
    drop(store);
    let db = Connection::open(dir.join("jarvis.sqlite3")).unwrap();
    db.execute_batch("PRAGMA foreign_keys=ON; INSERT INTO task_queue (task_id, state, resource) VALUES ('first', 'running', 'resource'), ('second', 'queued', 'resource'); INSERT INTO task_resource_locks (resource, task_id) VALUES ('resource', 'first');").unwrap();
    assert!(db
        .execute(
            "INSERT INTO task_resource_locks (resource, task_id) VALUES ('resource', 'second')",
            [],
        )
        .is_err());
    drop(db);
    let store = SqliteStore::open(&dir).unwrap();
    assert!(store.task("second").unwrap().is_some());
    drop(store);
    let db = Connection::open(dir.join("jarvis.sqlite3")).unwrap();
    let count: i64 = db
        .query_row("SELECT count(*) FROM task_queue", [], |r| r.get(0))
        .unwrap();
    let holder: String = db
        .query_row(
            "SELECT task_id FROM task_resource_locks WHERE resource='resource'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!((count, holder.as_str()), (2, "first"));
    drop(db);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn queued_task_cannot_start_agent_without_claim() {
    let dir = std::env::temp_dir().join(format!("jarvis-queue-admission-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut store = SqliteStore::create(&dir).unwrap();
    store
        .create_task(
            &Task {
                id: "task".into(),
                version: 1,
                title: "task".into(),
            },
            "created",
            &[],
        )
        .unwrap();
    store.enqueue_task("task", "resource").unwrap();
    assert!(!store.agent_step("task", 1, 2, 2).unwrap());
    drop(store);
    let mut store = SqliteStore::open(&dir).unwrap();
    assert!(store.claim_task("task").unwrap());
    assert!(store.agent_step("task", 1, 2, 2).unwrap());
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn recovery_preserves_unknown_runtime_and_inference() {
    let dir = std::env::temp_dir().join(format!("jarvis-queue-recovery-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut store = SqliteStore::create(&dir).unwrap();
    for id in ["first", "second"] {
        store
            .create_task(
                &Task {
                    id: id.into(),
                    version: 1,
                    title: id.into(),
                },
                "created",
                &[],
            )
            .unwrap();
        store.enqueue_task(id, "resource").unwrap();
    }
    assert!(store.claim_task("first").unwrap());
    let reservation = store.reserve_tokens(100, 10).unwrap().unwrap();
    drop(store);
    let mut store = SqliteStore::open(&dir).unwrap();
    assert!(!store.recover_task("first").unwrap());
    assert!(!store.claim_task("second").unwrap());
    store.settle_tokens(reservation, 10).unwrap();
    assert!(store.recover_task("first").unwrap());
    assert!(store.claim_task("first").unwrap());
    assert!(store.agent_step("first", 1, 2, 2).unwrap());
    drop(store);
    let mut store = SqliteStore::open(&dir).unwrap();
    assert!(!store.recover_task("first").unwrap());
    assert!(!store.claim_task("second").unwrap());
    assert!(store
        .close_recovered_task(
            "first",
            jarvis::storage::ReconciliationOutcome::Succeeded,
            "checked"
        )
        .is_err());
    store
        .reconcile_agent_runtime(
            "first",
            1,
            jarvis::storage::ReconciliationOutcome::Succeeded,
            "checked",
        )
        .unwrap();
    assert!(!store.recover_task("first").unwrap());
    store
        .close_recovered_task(
            "first",
            jarvis::storage::ReconciliationOutcome::Succeeded,
            "checked",
        )
        .unwrap();
    assert!(store.enqueue_task("first", "resource").is_err());
    assert!(store.claim_task("second").unwrap());
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

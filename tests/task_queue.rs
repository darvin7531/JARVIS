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
fn waiting_states_survive_restart() {
    let dir = std::env::temp_dir().join(format!("jarvis-waiting-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut store = SqliteStore::create(&dir).unwrap();
    for id in ["approval", "event"] {
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
    db.execute("INSERT INTO task_queue (task_id, state, resource) VALUES ('approval', 'waiting_approval', 'a')", []).expect("waiting approval must be persistable");
    db.execute(
        "INSERT INTO task_queue (task_id, state, resource) VALUES ('event', 'waiting_event', 'b')",
        [],
    )
    .expect("waiting event must be persistable");
    drop(db);
    drop(SqliteStore::open(&dir).unwrap());
    let db = Connection::open(dir.join("jarvis.sqlite3")).unwrap();
    for (id, expected) in [("approval", "waiting_approval"), ("event", "waiting_event")] {
        let state: String = db
            .query_row(
                "SELECT state FROM task_queue WHERE task_id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, expected);
    }
    drop(db);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn owner_wait_pause_resume_cancel_survive_restart() {
    use jarvis::tool_policy::ToolRequest;
    use std::time::{Duration, SystemTime};
    let dir = std::env::temp_dir().join(format!("jarvis-lifecycle-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut store = SqliteStore::create(&dir).unwrap();
    for id in ["a", "b", "c", "d"] {
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
    db.execute_batch("INSERT INTO task_queue (task_id,state,resource) VALUES ('a','queued','a'),('b','queued','b'),('c','queued','c'),('d','queued','d')").unwrap();
    drop(db);
    let request: ToolRequest = serde_json::from_value(serde_json::json!({"tool":"inert","action":"echo","resource":"task/item","data_class":"public","arguments":{"text":"hello"}})).unwrap();
    let mut store = SqliteStore::open(&dir).unwrap();
    store
        .queue_wait("a", 1, "approval", Some(&request))
        .unwrap();
    store.queue_wait("b", 1, "event", None).unwrap();
    store.queue_wait("c", 1, "wrong", Some(&request)).unwrap();
    store.queue_wait("d", 1, "event", None).unwrap();
    let wrong: ToolRequest = serde_json::from_value(serde_json::json!({"tool":"inert","action":"echo","resource":"task/item","data_class":"public","arguments":{"text":"different"}})).unwrap();
    store
        .issue_tool_approval(
            "wrong",
            "c",
            1,
            &wrong,
            SystemTime::now() + Duration::from_secs(60),
        )
        .unwrap();
    assert!(store.queue_signal("c", 1, "wrong").is_err());
    assert!(store.queue_signal("a", 1, "approval").is_err());
    assert!(store.queue_signal("b", 1, "other").is_err());
    assert!(store.queue_signal("b", 2, "event").is_err());
    store.queue_pause("a", 1, "checkpoint").unwrap();
    drop(store);
    let mut store = SqliteStore::open(&dir).unwrap();
    store.queue_signal("d", 1, "event").unwrap();
    assert!(store.queue_signal("a", 1, "approval").is_err());
    store.queue_resume("a", 1).unwrap();
    store
        .issue_tool_approval(
            "approval",
            "a",
            1,
            &request,
            SystemTime::now() + Duration::from_secs(60),
        )
        .unwrap();
    store.queue_signal("a", 1, "approval").unwrap();
    assert!(store.queue_signal("a", 1, "approval").is_err());
    store.queue_cancel("b", 1).unwrap();
    drop(store);
    drop(SqliteStore::open(&dir).unwrap());
    let db = Connection::open(dir.join("jarvis.sqlite3")).unwrap();
    let a: (String, String) = db
        .query_row(
            "SELECT state,checkpoint FROM task_queue WHERE task_id='a'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let b: String = db
        .query_row("SELECT state FROM task_queue WHERE task_id='b'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let consumed: i64 = db
        .query_row(
            "SELECT consumed FROM tool_approvals WHERE id='approval'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(a, ("queued".into(), "checkpoint".into()));
    assert_eq!(b, "cancelled");
    assert_eq!(consumed, 0);
    drop(db);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn migration_preserves_existing_queue_and_locks() {
    let dir = std::env::temp_dir().join(format!("jarvis-queue-migrate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut store = SqliteStore::create(&dir).unwrap();
    store
        .create_task(
            &Task {
                id: "old".into(),
                version: 1,
                title: "old".into(),
            },
            "created",
            &[],
        )
        .unwrap();
    drop(store);
    let db = Connection::open(dir.join("jarvis.sqlite3")).unwrap();
    db.execute_batch("DROP TABLE task_capacity; DROP TABLE task_resource_locks; DROP TABLE task_queue; CREATE TABLE task_queue (task_id TEXT PRIMARY KEY NOT NULL REFERENCES tasks(id), state TEXT NOT NULL CHECK(state IN ('queued', 'running')), resource TEXT NOT NULL CHECK(length(resource) > 0)); CREATE TABLE task_resource_locks (resource TEXT PRIMARY KEY NOT NULL, task_id TEXT NOT NULL UNIQUE REFERENCES task_queue(task_id)); INSERT INTO task_queue VALUES ('old','running','res'); INSERT INTO task_resource_locks VALUES ('res','old'); PRAGMA user_version=15;").unwrap();
    drop(db);
    drop(SqliteStore::open(&dir).unwrap());
    drop(SqliteStore::open(&dir).unwrap());
    let db = Connection::open(dir.join("jarvis.sqlite3")).unwrap();
    let row: (String,String) = db.query_row("SELECT q.state,l.resource FROM task_queue q JOIN task_resource_locks l ON q.task_id=l.task_id WHERE q.task_id='old'", [], |r| Ok((r.get(0)?,r.get(1)?))).unwrap();
    assert_eq!(row, ("running".into(), "res".into()));
    db.execute_batch("DROP TABLE task_capacity; ALTER TABLE task_queue DROP COLUMN retry_limit; ALTER TABLE task_queue DROP COLUMN foreground; ALTER TABLE task_queue DROP COLUMN deadline; ALTER TABLE task_queue DROP COLUMN claim_budget_id; PRAGMA user_version=16;")
        .unwrap();
    drop(db);
    drop(SqliteStore::open(&dir).unwrap());
    let db = Connection::open(dir.join("jarvis.sqlite3")).unwrap();
    let row: (String, String) = db.query_row("SELECT q.state,l.resource FROM task_queue q JOIN task_resource_locks l ON q.task_id=l.task_id WHERE q.task_id='old'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!(row, ("running".into(), "res".into()));
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
    assert!(store.recover_task("task").unwrap());
    let claim = store.claim_task_identity("task").unwrap().unwrap();
    assert!(store
        .agent_step_with_claim("task", 1, 2, 2, Some(&claim))
        .unwrap());
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn stale_claim_and_closed_unstarted_task_cannot_execute_or_requeue() {
    let dir = std::env::temp_dir().join(format!("jarvis-stale-claim-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut store = SqliteStore::create(&dir).unwrap();
    let mut task = Task {
        id: "task".into(),
        version: 1,
        title: "task".into(),
    };
    store.create_task(&task, "created", &[]).unwrap();
    store.enqueue_task("task", "resource").unwrap();
    assert!(store.claim_task("task").unwrap());
    task.version = 2;
    store.update_task(&task, "updated").unwrap();
    assert!(!store.agent_step("task", 2, 2, 2).unwrap());
    assert!(!store.recover_task("task").unwrap());
    store
        .close_recovered_task(
            "task",
            jarvis::storage::ReconciliationOutcome::Failed,
            "owner checked",
        )
        .unwrap();
    assert!(store.enqueue_task("task", "resource").is_err());
    assert!(!store.agent_step("task", 2, 2, 2).unwrap());
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
    assert!(!store.recover_task("first").unwrap());
    store
        .close_recovered_task(
            "first",
            jarvis::storage::ReconciliationOutcome::Succeeded,
            "checked",
        )
        .unwrap();
    assert!(store.enqueue_task("first", "resource").is_err());
    let claim = store.claim_task_identity("second").unwrap().unwrap();
    assert!(store
        .agent_step_with_claim("second", 1, 2, 2, Some(&claim))
        .unwrap());
    drop(store);
    let mut store = SqliteStore::open(&dir).unwrap();
    assert!(!store.recover_task("second").unwrap());
    assert!(store
        .close_recovered_task(
            "second",
            jarvis::storage::ReconciliationOutcome::Succeeded,
            "checked"
        )
        .is_err());
    store
        .reconcile_agent_runtime(
            "second",
            1,
            jarvis::storage::ReconciliationOutcome::Succeeded,
            "checked",
        )
        .unwrap();
    assert!(!store.recover_task("second").unwrap());
    store
        .close_recovered_task(
            "second",
            jarvis::storage::ReconciliationOutcome::Succeeded,
            "checked",
        )
        .unwrap();
    assert!(store.enqueue_task("second", "resource").is_err());
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn recovery_is_bounded_across_restart() {
    let dir = std::env::temp_dir().join(format!("jarvis-retry-bound-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut store = SqliteStore::create(&dir).unwrap();
    store
        .create_task(
            &Task {
                id: "t".into(),
                version: 1,
                title: "t".into(),
            },
            "created",
            &[],
        )
        .unwrap();
    store.enqueue_task("t", "r").unwrap();
    for _ in 0..3 {
        assert!(store.claim_task("t").unwrap());
        assert!(store.recover_task("t").unwrap());
        drop(store);
        store = SqliteStore::open(&dir).unwrap();
    }
    assert!(store.claim_task("t").unwrap());
    assert!(!store.recover_task("t").unwrap());
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn owner_admission_controls_survive_restart_and_contend_atomically() {
    use std::sync::{Arc, Barrier};
    use std::time::{SystemTime, UNIX_EPOCH};
    let dir = std::env::temp_dir().join(format!("jarvis-owner-controls-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut store = SqliteStore::create(&dir).unwrap();
    for id in ["background", "foreground", "expired", "one", "two"] {
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
        store.enqueue_task(id, id).unwrap();
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    store
        .set_task_policy("foreground", Some(now + 3600), true, 1)
        .unwrap();
    store
        .set_task_policy("expired", Some(now - 1), true, 0)
        .unwrap();
    assert!(store.set_task_capacity(0).is_err());
    drop(store);
    let mut store = SqliteStore::open(&dir).unwrap();
    assert!(!store.claim_task("expired").unwrap());
    assert!(!store.claim_task("background").unwrap());
    assert!(store.claim_task("foreground").unwrap());
    assert!(!store.claim_task("background").unwrap());
    assert!(store.recover_task("foreground").unwrap());
    assert!(store.claim_task("foreground").unwrap());
    assert!(!store.recover_task("foreground").unwrap());
    store
        .close_recovered_task(
            "foreground",
            jarvis::storage::ReconciliationOutcome::NotExecuted,
            "owner confirmed",
        )
        .unwrap();
    store.queue_cancel("background", 1).unwrap();
    drop(store);
    let barrier = Arc::new(Barrier::new(3));
    let handles: Vec<_> = ["one", "two"]
        .into_iter()
        .map(|id| {
            let dir = dir.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut store = SqliteStore::open(&dir).unwrap();
                barrier.wait();
                store.claim_task(id).unwrap()
            })
        })
        .collect();
    barrier.wait();
    let wins = handles
        .into_iter()
        .map(|h| h.join().unwrap())
        .filter(|won| *won)
        .count();
    assert_eq!(wins, 1);
    let mut store = SqliteStore::open(&dir).unwrap();
    store.set_task_capacity(2).unwrap();
    let db = Connection::open(dir.join("jarvis.sqlite3")).unwrap();
    let remaining: String = db
        .query_row(
            "SELECT task_id FROM task_queue WHERE task_id IN ('one','two') AND state='queued'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(store.claim_task(&remaining).unwrap());
    drop(db);
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

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
    for id in ["a", "b"] {
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
    db.execute_batch("INSERT INTO task_queue (task_id,state,resource) VALUES ('a','queued','a'),('b','queued','b')").unwrap();
    drop(db);
    let request: ToolRequest = serde_json::from_value(serde_json::json!({"tool":"inert","action":"echo","resource":"task/item","data_class":"public","arguments":{"text":"hello"}})).unwrap();
    let mut store = SqliteStore::open(&dir).unwrap();
    store
        .queue_wait("a", 1, "approval", Some(&request))
        .unwrap();
    store.queue_wait("b", 1, "event", None).unwrap();
    assert!(store.queue_signal("a", 1, "approval").is_err());
    assert!(store.queue_signal("b", 1, "other").is_err());
    assert!(store.queue_signal("b", 2, "event").is_err());
    store.queue_pause("a", 1, "checkpoint").unwrap();
    drop(store);
    let mut store = SqliteStore::open(&dir).unwrap();
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
    db.execute_batch("DROP TABLE task_resource_locks; DROP TABLE task_queue; CREATE TABLE task_queue (task_id TEXT PRIMARY KEY NOT NULL REFERENCES tasks(id), state TEXT NOT NULL CHECK(state IN ('queued', 'running')), resource TEXT NOT NULL CHECK(length(resource) > 0)); CREATE TABLE task_resource_locks (resource TEXT PRIMARY KEY NOT NULL, task_id TEXT NOT NULL UNIQUE REFERENCES task_queue(task_id)); INSERT INTO task_queue VALUES ('old','running','res'); INSERT INTO task_resource_locks VALUES ('res','old'); PRAGMA user_version=15;").unwrap();
    drop(db);
    drop(SqliteStore::open(&dir).unwrap());
    drop(SqliteStore::open(&dir).unwrap());
    let db = Connection::open(dir.join("jarvis.sqlite3")).unwrap();
    let row: (String,String) = db.query_row("SELECT q.state,l.resource FROM task_queue q JOIN task_resource_locks l ON q.task_id=l.task_id WHERE q.task_id='old'", [], |r| Ok((r.get(0)?,r.get(1)?))).unwrap();
    assert_eq!(row, ("running".into(), "res".into()));
    drop(db);
    std::fs::remove_dir_all(dir).unwrap();
}

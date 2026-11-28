use jarvis::storage::{SqliteStore, Task};
use rusqlite::Connection;

#[test]
fn queue_and_resource_claim_survive_restart_and_exclude_conflicts() {
    let dir = std::env::temp_dir().join(format!("jarvis-queue-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut store = SqliteStore::create(&dir).unwrap();
    for id in ["first", "second"] {
        store.create_task(&Task { id: id.into(), version: 1, title: id.into() }, "created", &[]).unwrap();
    }
    drop(store);
    let db = Connection::open(dir.join("jarvis.sqlite3")).unwrap();
    db.execute_batch("PRAGMA foreign_keys=ON; INSERT INTO task_queue (task_id, state, resource) VALUES ('first', 'running', 'resource'), ('second', 'queued', 'resource'); INSERT INTO task_resource_locks (resource, task_id) VALUES ('resource', 'first');").unwrap();
    assert!(db.execute("INSERT INTO task_resource_locks (resource, task_id) VALUES ('resource', 'second')", []).is_err());
    drop(db);
    let store = SqliteStore::open(&dir).unwrap();
    assert!(store.task("second").unwrap().is_some());
    drop(store);
    let db = Connection::open(dir.join("jarvis.sqlite3")).unwrap();
    let count: i64 = db.query_row("SELECT count(*) FROM task_queue", [], |r| r.get(0)).unwrap();
    let holder: String = db.query_row("SELECT task_id FROM task_resource_locks WHERE resource='resource'", [], |r| r.get(0)).unwrap();
    assert_eq!((count, holder.as_str()), (2, "first"));
    drop(db);
    std::fs::remove_dir_all(dir).unwrap();
}

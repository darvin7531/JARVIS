use jarvis::storage::{SqliteStore, Task};

#[test]
fn task_survives_reopen() {
    let dir = std::env::temp_dir().join(format!("jarvis-r02-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let task = Task {
        id: "task-1".into(),
        version: 1,
        title: "first".into(),
    };
    {
        let mut store = SqliteStore::open(&dir).unwrap();
        store.create_task(&task, "created", &[]).unwrap();
    }
    let store = SqliteStore::open(&dir).unwrap();
    assert_eq!(store.task("task-1").unwrap(), Some(task));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn evidence_is_persisted_with_task_and_event() {
    let dir = std::env::temp_dir().join(format!("jarvis-evidence-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let task = Task {
        id: "t".into(),
        version: 1,
        title: "test".into(),
    };
    {
        let mut store = SqliteStore::open(&dir).unwrap();
        store
            .create_task(&task, "created", &["evidence-1".into()])
            .unwrap();
    }
    let db = rusqlite::Connection::open(dir.join("jarvis.sqlite3")).unwrap();
    let event: String = db
        .query_row("SELECT event FROM events WHERE task_id = 't'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(event, "created");
    let evidence: String = db
        .query_row(
            "SELECT reference FROM evidence WHERE task_id = 't'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(evidence, "evidence-1");
    drop(db);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn rejects_unknown_future_schema_without_changing_it() {
    let dir = std::env::temp_dir().join(format!("jarvis-version-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("jarvis.sqlite3");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.pragma_update(None, "user_version", 999).unwrap();
    drop(db);
    assert!(SqliteStore::open(&dir).is_err());
    let db = rusqlite::Connection::open(path).unwrap();
    let version: i64 = db
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(version, 999);
    drop(db);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn backup_restores_a_consistent_workspace() {
    let root = std::env::temp_dir().join(format!("jarvis-backup-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let source = root.join("source");
    let restored = root.join("restored");
    let task = Task {
        id: "backup-task".into(),
        version: 1,
        title: "snapshot".into(),
    };
    let mut store = SqliteStore::open(&source).unwrap();
    store.create_task(&task, "created", &[]).unwrap();
    store.backup(&restored).unwrap();
    drop(store);
    let restored_store = SqliteStore::open(&restored).unwrap();
    assert_eq!(restored_store.task("backup-task").unwrap(), Some(task));
    drop(restored_store);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn audit_events_are_append_only_and_survive_reopen() {
    let dir = std::env::temp_dir().join(format!("jarvis-audit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let task = Task {
        id: "audit".into(),
        version: 1,
        title: "audit test".into(),
    };
    let mut store = SqliteStore::open(&dir).unwrap();
    store.create_task(&task, "created", &[]).unwrap();
    store.append_event("audit", "checked").unwrap();
    drop(store);
    let store = SqliteStore::open(&dir).unwrap();
    assert_eq!(store.events("audit").unwrap(), ["created", "checked"]);
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn failed_event_write_rolls_back_task_creation() {
    let dir = std::env::temp_dir().join(format!("jarvis-rollback-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut store = SqliteStore::open(&dir).unwrap();
    let db = rusqlite::Connection::open(dir.join("jarvis.sqlite3")).unwrap();
    db.execute_batch(
        "CREATE TRIGGER fail_event BEFORE INSERT ON events BEGIN SELECT RAISE(ABORT, 'fail'); END;",
    )
    .unwrap();
    let task = Task {
        id: "rolled-back".into(),
        version: 1,
        title: "test".into(),
    };
    assert!(store.create_task(&task, "created", &[]).is_err());
    assert_eq!(store.task("rolled-back").unwrap(), None);
    drop(db);
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn opens_existing_schema_idempotently() {
    let dir = std::env::temp_dir().join(format!("jarvis-migration-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    drop(SqliteStore::open(&dir).unwrap());
    drop(SqliteStore::open(&dir).unwrap());
    let db = rusqlite::Connection::open(dir.join("jarvis.sqlite3")).unwrap();
    let version: i64 = db
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(version, 1);
    drop(db);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn corrupt_database_fails_without_exposing_path() {
    let root = std::env::temp_dir().join(format!("jarvis-secret-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("jarvis.sqlite3"), b"not a sqlite database").unwrap();
    let result = SqliteStore::open(&root);
    let err = match result {
        Err(err) => err,
        Ok(_) => panic!("corrupt database unexpectedly opened"),
    };
    assert_eq!(err.to_string(), "storage operation failed");
    assert!(!format!("{err:?}").contains("secret"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn task_can_be_read_through_storage_port() {
    fn read(store: &dyn jarvis::storage::TaskStore, id: &str) -> Option<Task> {
        store.read_task(id).unwrap()
    }
    let dir = std::env::temp_dir().join(format!("jarvis-port-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut store = SqliteStore::open(&dir).unwrap();
    let task = Task {
        id: "port".into(),
        version: 1,
        title: "test".into(),
    };
    store.create_task(&task, "created", &[]).unwrap();
    assert_eq!(read(&store, "port"), Some(task));
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn rejects_unversioned_database_with_existing_tables_without_modification() {
    let dir = std::env::temp_dir().join(format!("jarvis-foreign-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("jarvis.sqlite3");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE tasks (unexpected TEXT);")
        .unwrap();
    drop(db);
    assert!(SqliteStore::open(&dir).is_err());
    let db = rusqlite::Connection::open(path).unwrap();
    let count: i64 = db
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type='table' AND name='events'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
    drop(db);
    std::fs::remove_dir_all(dir).unwrap();
}

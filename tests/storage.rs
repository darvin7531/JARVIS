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

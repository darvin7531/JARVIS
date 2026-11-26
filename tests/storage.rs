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
        let mut store = SqliteStore::create(&dir).unwrap();
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
        let mut store = SqliteStore::create(&dir).unwrap();
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
    let mut store = SqliteStore::create(&source).unwrap();
    store
        .create_task(&task, "created", &["backup-ref".into()])
        .unwrap();
    store.backup(&restored).unwrap();
    assert!(store.backup(&restored).is_err());
    store.append_event("backup-task", "after-snapshot").unwrap();
    drop(store);
    let restored_store = SqliteStore::open(&restored).unwrap();
    assert_eq!(restored_store.task("backup-task").unwrap(), Some(task));
    assert_eq!(restored_store.events("backup-task").unwrap(), ["created"]);
    let port: &dyn jarvis::storage::TaskStore = &restored_store;
    assert_eq!(port.read_evidence("backup-task").unwrap(), ["backup-ref"]);
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
    let mut store = SqliteStore::create(&dir).unwrap();
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
    let mut store = SqliteStore::create(&dir).unwrap();
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
    drop(SqliteStore::create(&dir).unwrap());
    drop(SqliteStore::open(&dir).unwrap());
    let db = rusqlite::Connection::open(dir.join("jarvis.sqlite3")).unwrap();
    let version: i64 = db
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(version, 3);
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
    let mut store = SqliteStore::create(&dir).unwrap();
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

#[test]
fn task_and_audit_writes_work_through_storage_port() {
    let dir = std::env::temp_dir().join(format!("jarvis-port-write-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut store = SqliteStore::create(&dir).unwrap();
    let port: &mut dyn jarvis::storage::TaskStore = &mut store;
    let task = Task {
        id: "written".into(),
        version: 1,
        title: "port".into(),
    };
    port.write_task(&task, "created", &[]).unwrap();
    port.write_event("written", "reviewed").unwrap();
    assert_eq!(port.read_task("written").unwrap(), Some(task));
    assert_eq!(
        port.read_events("written").unwrap(),
        ["created", "reviewed"]
    );
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn updating_task_checks_version_and_records_audit_atomically() {
    let dir = std::env::temp_dir().join(format!("jarvis-revision-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut store = SqliteStore::create(&dir).unwrap();
    let initial = Task {
        id: "versioned".into(),
        version: 1,
        title: "first".into(),
    };
    let revised = Task {
        id: "versioned".into(),
        version: 2,
        title: "second".into(),
    };
    store.create_task(&initial, "created", &[]).unwrap();
    store.update_task(&revised, "updated").unwrap();
    assert!(store.update_task(&revised, "duplicate").is_err());
    drop(store);
    let store = SqliteStore::open(&dir).unwrap();
    assert_eq!(store.task("versioned").unwrap(), Some(revised));
    assert_eq!(store.events("versioned").unwrap(), ["created", "updated"]);
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn evidence_references_are_readable_through_port_after_reopen() {
    let dir = std::env::temp_dir().join(format!("jarvis-refs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut store = SqliteStore::create(&dir).unwrap();
    let task = Task {
        id: "evidence".into(),
        version: 1,
        title: "test".into(),
    };
    store
        .create_task(&task, "created", &["artifact:1".into()])
        .unwrap();
    drop(store);
    let store = SqliteStore::open(&dir).unwrap();
    let port: &dyn jarvis::storage::TaskStore = &store;
    assert_eq!(port.read_evidence("evidence").unwrap(), ["artifact:1"]);
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn migrates_v1_database_forward_without_losing_records() {
    let dir = std::env::temp_dir().join(format!("jarvis-v1-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = rusqlite::Connection::open(dir.join("jarvis.sqlite3")).unwrap();
    db.execute_batch("CREATE TABLE tasks (id TEXT PRIMARY KEY, version INTEGER NOT NULL, title TEXT NOT NULL); CREATE TABLE events (id INTEGER PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tasks(id), event TEXT NOT NULL); CREATE TABLE evidence (task_id TEXT NOT NULL REFERENCES tasks(id), reference TEXT NOT NULL); INSERT INTO tasks VALUES ('old', 1, 'preserved'); INSERT INTO events (task_id, event) VALUES ('old', 'created'); PRAGMA user_version = 1;").unwrap();
    drop(db);
    let store = SqliteStore::open(&dir).unwrap();
    assert_eq!(store.task("old").unwrap().unwrap().title, "preserved");
    assert_eq!(store.events("old").unwrap(), ["created"]);
    drop(store);
    let db = rusqlite::Connection::open(dir.join("jarvis.sqlite3")).unwrap();
    let version: i64 = db
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(version, 3);
    drop(db);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn failed_audit_write_rolls_back_task_update() {
    let dir = std::env::temp_dir().join(format!("jarvis-update-rollback-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut store = SqliteStore::create(&dir).unwrap();
    let original = Task {
        id: "task".into(),
        version: 1,
        title: "original".into(),
    };
    store.create_task(&original, "created", &[]).unwrap();
    let db = rusqlite::Connection::open(dir.join("jarvis.sqlite3")).unwrap();
    db.execute_batch(
        "CREATE TRIGGER fail_update BEFORE INSERT ON events BEGIN SELECT RAISE(ABORT, 'fail'); END;",
    )
    .unwrap();
    let changed = Task {
        id: "task".into(),
        version: 2,
        title: "changed".into(),
    };
    assert!(store.update_task(&changed, "updated").is_err());
    assert_eq!(store.task("task").unwrap(), Some(original));
    assert_eq!(store.events("task").unwrap(), ["created"]);
    drop(db);
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn versioned_update_is_available_through_storage_port() {
    let dir = std::env::temp_dir().join(format!("jarvis-port-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut store = SqliteStore::create(&dir).unwrap();
    let port: &mut dyn jarvis::storage::TaskStore = &mut store;
    let mut task = Task {
        id: "task".into(),
        version: 1,
        title: "first".into(),
    };
    port.write_task(&task, "created", &[]).unwrap();
    task.version = 2;
    task.title = "revised".into();
    port.revise_task(&task, "updated").unwrap();
    assert_eq!(port.read_task("task").unwrap(), Some(task));
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn unsupported_schema_has_a_distinct_redacted_error() {
    let dir = std::env::temp_dir().join(format!("jarvis-unsupported-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = rusqlite::Connection::open(dir.join("jarvis.sqlite3")).unwrap();
    db.pragma_update(None, "user_version", 999).unwrap();
    drop(db);
    let error = match SqliteStore::open(&dir) {
        Err(error) => error,
        Ok(_) => panic!("unsupported version accepted"),
    };
    assert_eq!(format!("{error:?}"), "UnsupportedSchema");
    assert_eq!(error.to_string(), "unsupported schema version");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn failed_forward_migration_preserves_existing_version_and_data() {
    let dir = std::env::temp_dir().join(format!("jarvis-failed-migration-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("jarvis.sqlite3");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE tasks (id TEXT PRIMARY KEY, version INTEGER NOT NULL, title TEXT NOT NULL); CREATE TABLE events (id INTEGER PRIMARY KEY, task_id TEXT NOT NULL, event TEXT NOT NULL); INSERT INTO tasks VALUES ('keep', 1, 'data'); PRAGMA user_version = 1;").unwrap();
    drop(db);
    assert!(SqliteStore::open(&dir).is_err());
    let db = rusqlite::Connection::open(path).unwrap();
    let version: i64 = db
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    let title: String = db
        .query_row("SELECT title FROM tasks WHERE id='keep'", [], |r| r.get(0))
        .unwrap();
    let indexes: i64 = db
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name='events_task_id'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!((version, title.as_str(), indexes), (1, "data", 0));
    drop(db);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn sqlite_failures_redact_task_values() {
    let dir = std::env::temp_dir().join(format!("jarvis-redact-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut store = SqliteStore::create(&dir).unwrap();
    let task = Task {
        id: "secret-token-value".into(),
        version: 1,
        title: "secret-token-value".into(),
    };
    store.create_task(&task, "created", &[]).unwrap();
    let error = store
        .create_task(&task, "secret-token-value", &[])
        .unwrap_err();
    assert!(!format!("{error} {error:?}").contains("secret-token-value"));
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn bundled_sqlite_supports_fts5() {
    let db = rusqlite::Connection::open_in_memory().unwrap();
    db.execute_batch("CREATE VIRTUAL TABLE probe USING fts5(content);")
        .unwrap();
}

#[test]
fn audit_records_revision_and_timestamp() {
    let dir = std::env::temp_dir().join(format!("jarvis-audit-metadata-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut store = SqliteStore::create(&dir).unwrap();
    let mut task = Task {
        id: "t".into(),
        version: 1,
        title: "first".into(),
    };
    store.create_task(&task, "created", &[]).unwrap();
    task.version = 2;
    store.update_task(&task, "updated").unwrap();
    store.append_event("t", "checked").unwrap();
    drop(store);
    let db = rusqlite::Connection::open(dir.join("jarvis.sqlite3")).unwrap();
    let mut query = db
        .prepare("SELECT revision, occurred_at FROM events ORDER BY id")
        .unwrap();
    let rows: Vec<(i64, String)> = query
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(rows.iter().map(|r| r.0).collect::<Vec<_>>(), [1, 2, 2]);
    assert!(rows.iter().all(|r| r.1.ends_with('Z')));
    drop(query);
    drop(db);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn opening_a_workspace_with_missing_database_must_not_create_one() {
    let dir = std::env::temp_dir().join(format!("jarvis-missing-db-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    assert!(SqliteStore::open(&dir).is_err());
    assert!(!dir.join("jarvis.sqlite3").exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn rejects_forged_known_schema_without_modification() {
    let dir = std::env::temp_dir().join(format!("jarvis-forged-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("jarvis.sqlite3");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE tasks (id TEXT PRIMARY KEY, version INTEGER NOT NULL, title TEXT NOT NULL); CREATE TABLE events (id INTEGER PRIMARY KEY, task_id TEXT NOT NULL, event TEXT NOT NULL); CREATE TABLE evidence (task_id TEXT NOT NULL, reference TEXT NOT NULL); PRAGMA user_version = 1;").unwrap();
    drop(db);
    assert!(SqliteStore::open(&dir).is_err());
    let db = rusqlite::Connection::open(path).unwrap();
    let version: i64 = db
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(version, 1);
    drop(db);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn backup_times_out_under_exclusive_lock_without_publishing() {
    let root = std::env::temp_dir().join(format!("jarvis-backup-busy-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let source = root.join("source");
    let dest = root.join("snapshot");
    let store = SqliteStore::create(&source).unwrap();
    let db = rusqlite::Connection::open(source.join("jarvis.sqlite3")).unwrap();
    db.execute_batch("BEGIN EXCLUSIVE").unwrap();
    let started = std::time::Instant::now();
    assert!(store.backup(&dest).is_err());
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    assert!(!dest.exists());
    db.execute_batch("ROLLBACK").unwrap();
    drop(db);
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn workspace_creation_is_private_and_never_replaces_existing_data() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("jarvis-private-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let store = SqliteStore::create(&dir).unwrap();
    assert_eq!(
        std::fs::metadata(&dir).unwrap().permissions().mode() & 0o077,
        0
    );
    assert!(SqliteStore::create(&dir).is_err());
    drop(store);
    assert!(SqliteStore::open(&dir).is_ok());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn evidence_metadata_columns_exist() {
    let dir = std::env::temp_dir().join(format!("jarvis-evidence-schema-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    drop(SqliteStore::create(&dir).unwrap());
    let db = rusqlite::Connection::open(dir.join("jarvis.sqlite3")).unwrap();
    assert!(db.prepare("SELECT provenance, observed_at, classification FROM evidence").is_ok());
    drop(db);
    std::fs::remove_dir_all(dir).unwrap();
}

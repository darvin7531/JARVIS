use jarvis::storage::{SqliteStore, Task};

fn private_dir(path: &std::path::Path) {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().mode(0o700).create(path).unwrap();
}

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
    private_dir(&dir);
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
    assert_eq!(version, 14);
    drop(db);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn corrupt_database_fails_without_exposing_path() {
    let root = std::env::temp_dir().join(format!("jarvis-secret-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    private_dir(&root);
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
    private_dir(&dir);
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
fn task_revision_cannot_bypass_unreconciled_agent_step() {
    let dir = std::env::temp_dir().join(format!("jarvis-agent-revision-{}", std::process::id()));
    let mut store = SqliteStore::create(&dir).unwrap();
    let task = Task {
        id: "guarded".into(),
        version: 1,
        title: "first".into(),
    };
    store.create_task(&task, "created", &[]).unwrap();
    assert!(store.agent_step("guarded", 1, 1, 1).unwrap());
    let revised = Task { version: 2, ..task };
    assert!(store.update_task(&revised, "updated").is_err());
    store.finish_agent_step("guarded", 1, false, true).unwrap();
    assert!(store.update_task(&revised, "updated").is_err());
}

#[test]
fn agent_runtime_has_durable_closure_separate_from_terminal() {
    let dir = std::env::temp_dir().join(format!("jarvis-agent-closure-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let store = SqliteStore::create(&dir).unwrap();
    let db = rusqlite::Connection::open(dir.join("jarvis.sqlite3")).unwrap();
    let count: i64 = db
        .query_row(
            "SELECT count(*) FROM pragma_table_info('agent_runtime') WHERE name = 'closed'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 1,
        "runtime closure must be durable and independent of terminal"
    );
    drop(db);
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
    private_dir(&dir);
    let db = rusqlite::Connection::open(dir.join("jarvis.sqlite3")).unwrap();
    db.execute_batch("CREATE TABLE tasks (id TEXT PRIMARY KEY, version INTEGER NOT NULL, title TEXT NOT NULL); CREATE TABLE events (id INTEGER PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tasks(id), event TEXT NOT NULL); CREATE TABLE evidence (task_id TEXT NOT NULL REFERENCES tasks(id), reference TEXT NOT NULL); INSERT INTO tasks VALUES ('old', 1, 'preserved'); INSERT INTO events (task_id, event) VALUES ('old', 'created'); INSERT INTO evidence VALUES ('old', 'legacy'); PRAGMA user_version = 1;").unwrap();
    drop(db);
    let store = SqliteStore::open(&dir).unwrap();
    assert_eq!(store.task("old").unwrap().unwrap().title, "preserved");
    assert_eq!(store.events("old").unwrap(), ["created"]);
    let port: &dyn jarvis::storage::TaskStore = &store;
    let evidence = port.read_evidence_metadata("old").unwrap();
    assert_eq!(evidence[0].reference, "legacy");
    assert_eq!(evidence[0].provenance, None);
    assert_eq!(evidence[0].observed_at, None);
    assert_eq!(evidence[0].classification, None);
    let audit = port.read_audit_metadata("old").unwrap();
    assert_eq!(audit[0].event, "created");
    assert_eq!(audit[0].revision, None);
    assert_eq!(audit[0].occurred_at, None);
    drop(store);
    let db = rusqlite::Connection::open(dir.join("jarvis.sqlite3")).unwrap();
    let version: i64 = db
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(version, 14);
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
    private_dir(&dir);
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
    private_dir(&dir);
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
    private_dir(&dir);
    assert!(SqliteStore::open(&dir).is_err());
    assert!(!dir.join("jarvis.sqlite3").exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn rejects_forged_known_schema_without_modification() {
    let dir = std::env::temp_dir().join(format!("jarvis-forged-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    private_dir(&dir);
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
    assert!(db
        .prepare("SELECT provenance, observed_at, classification FROM evidence")
        .is_ok());
    drop(db);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn typed_evidence_and_audit_survive_reopen_through_port() {
    use jarvis::storage::{Evidence, TaskStore};
    let dir = std::env::temp_dir().join(format!("jarvis-typed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut store = SqliteStore::create(&dir).unwrap();
    let task = Task {
        id: "typed".into(),
        version: 7,
        title: "test".into(),
    };
    let evidence = Evidence {
        reference: "ref".into(),
        provenance: Some("origin".into()),
        observed_at: Some("2026-11-26T12:00:00Z".into()),
        classification: Some("internal".into()),
    };
    store
        .write_task_with_evidence(&task, "created", &[evidence])
        .unwrap();
    drop(store);
    let store = SqliteStore::open(&dir).unwrap();
    let port: &dyn TaskStore = &store;
    let rows = port.read_evidence_metadata("typed").unwrap();
    assert_eq!(rows[0].reference, "ref");
    assert_eq!(rows[0].provenance.as_deref(), Some("origin"));
    assert_eq!(rows[0].observed_at.as_deref(), Some("2026-11-26T12:00:00Z"));
    assert_eq!(rows[0].classification.as_deref(), Some("internal"));
    let audit = port.read_audit_metadata("typed").unwrap();
    assert_eq!(audit[0].event, "created");
    assert_eq!(audit[0].revision, Some(7));
    assert!(audit[0].occurred_at.as_deref().unwrap().ends_with('Z'));
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn wal_backup_with_concurrent_writes_is_consistent_and_refuses_live_destination() {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    };
    let root = std::env::temp_dir().join(format!("jarvis-wal-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let source = root.join("source");
    let snapshot = root.join("snapshot");
    let mut store = SqliteStore::create(&source).unwrap();
    let db = rusqlite::Connection::open(source.join("jarvis.sqlite3")).unwrap();
    let mode: String = db
        .query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
    drop(db);
    let task = Task {
        id: "wal".into(),
        version: 1,
        title: "initial".into(),
    };
    store.create_task(&task, "created", &[]).unwrap();
    let running = Arc::new(AtomicBool::new(true));
    let flag = running.clone();
    let path = source.join("jarvis.sqlite3");
    let (ready_tx, ready_rx) = mpsc::channel();
    let writer = std::thread::spawn(move || {
        let mut db = rusqlite::Connection::open(path).unwrap();
        db.busy_timeout(std::time::Duration::from_secs(2)).unwrap();
        let mut revision = 1;
        while flag.load(Ordering::Relaxed) && revision < 500 {
            revision += 1;
            let tx = db.transaction().unwrap();
            tx.execute("UPDATE tasks SET version = ?1 WHERE id = 'wal'", [revision])
                .unwrap();
            tx.execute("INSERT INTO events (task_id, event, revision, occurred_at) VALUES ('wal', 'write', ?1, 'now')", [revision]).unwrap();
            tx.commit().unwrap();
            if revision == 2 {
                ready_tx.send(()).unwrap();
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    });
    ready_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    let start = std::time::Instant::now();
    let result = store.backup(&snapshot);
    running.store(false, Ordering::Relaxed);
    writer.join().unwrap();
    result.unwrap();
    assert!(start.elapsed() < std::time::Duration::from_secs(10));
    let restored = SqliteStore::open(&snapshot).unwrap();
    let version = restored.task("wal").unwrap().unwrap().version;
    let audit = restored.audit_metadata("wal").unwrap();
    assert_eq!(audit.last().unwrap().revision, Some(version));
    assert!(version >= 2);
    assert!(store.backup(&source).is_err());
    assert!(SqliteStore::open(&source)
        .unwrap()
        .task("wal")
        .unwrap()
        .is_some());
    drop(restored);
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn storage_error_output_does_not_leak_secrets() {
    const SECRET: &str = "sensitive-token-739284";
    if std::env::var_os("JARVIS_ERROR_CHILD").is_some() {
        let dir =
            std::env::temp_dir().join(format!("jarvis-error-{}-{}", SECRET, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut store = SqliteStore::create(&dir).unwrap();
        let db = rusqlite::Connection::open(dir.join("jarvis.sqlite3")).unwrap();
        db.execute_batch("CREATE TRIGGER fail_secret BEFORE INSERT ON events BEGIN SELECT RAISE(ABORT, 'sensitive-token-739284'); END;").unwrap();
        let task = Task {
            id: SECRET.into(),
            version: 1,
            title: SECRET.into(),
        };
        let error = store.create_task(&task, SECRET, &[]).unwrap_err();
        eprintln!("{error} {error:?}");
        drop(db);
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
        return;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "storage_error_output_does_not_leak_secrets",
            "--nocapture",
        ])
        .env("JARVIS_ERROR_CHILD", "1")
        .output()
        .unwrap();
    assert!(output.status.success());
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.contains("storage operation failed"));
    assert!(!output.contains(SECRET));
}

#[test]
fn reopen_rejects_extra_schema_objects_and_foreign_key_violations() {
    for (name, sql) in [
        ("trigger", "CREATE TRIGGER evil AFTER INSERT ON tasks BEGIN UPDATE tasks SET title='changed'; END;"),
        ("sqlite-prefix-trigger", "CREATE TRIGGER sqliteXevil AFTER INSERT ON tasks BEGIN UPDATE tasks SET title='changed'; END;"),
        ("view", "CREATE VIEW evil AS SELECT * FROM tasks;"),
        ("foreign-key", "PRAGMA foreign_keys=OFF; INSERT INTO events (task_id, event) VALUES ('missing', 'orphan');"),
    ] {
        let dir = std::env::temp_dir().join(format!("jarvis-invalid-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        drop(SqliteStore::create(&dir).unwrap());
        let db = rusqlite::Connection::open(dir.join("jarvis.sqlite3")).unwrap();
        db.execute_batch(sql).unwrap();
        drop(db);
        assert!(SqliteStore::open(&dir).is_err(), "accepted {name}");
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn reopen_rejects_public_workspace_without_changing_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("jarvis-public-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    drop(SqliteStore::create(&dir).unwrap());
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(SqliteStore::open(&dir).is_err());
    assert_eq!(
        std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o755
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn invalid_evidence_metadata_is_rejected_atomically() {
    use jarvis::storage::Evidence;
    let dir = std::env::temp_dir().join(format!("jarvis-invalid-metadata-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut store = SqliteStore::create(&dir).unwrap();
    let task = Task {
        id: "metadata".into(),
        version: 1,
        title: "test".into(),
    };
    for (provenance, observed_at, classification) in [
        (Some(""), None, None),
        (None, Some("not-a-time"), None),
        (None, Some("2026-02-30T12:00:00Z"), None),
        (None, None, Some("")),
    ] {
        let evidence = Evidence {
            reference: "ref".into(),
            provenance: provenance.map(str::to_owned),
            observed_at: observed_at.map(str::to_owned),
            classification: classification.map(str::to_owned),
        };
        assert!(store
            .create_task_with_evidence(&task, "created", &[evidence])
            .is_err());
        assert_eq!(store.task("metadata").unwrap(), None);
    }
    drop(store);
    assert_eq!(
        SqliteStore::open(&dir).unwrap().task("metadata").unwrap(),
        None
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn reopen_rejects_integrity_errors_without_repairing_data() {
    let dir = std::env::temp_dir().join(format!("jarvis-integrity-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    drop(SqliteStore::create(&dir).unwrap());
    let path = dir.join("jarvis.sqlite3");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("PRAGMA writable_schema=ON; UPDATE sqlite_schema SET sql=replace(sql, 'title TEXT NOT NULL', 'title TEXT') WHERE name='tasks';").unwrap();
    drop(db);
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("INSERT INTO tasks VALUES ('broken', 1, NULL); PRAGMA writable_schema=ON; UPDATE sqlite_schema SET sql=replace(sql, 'title TEXT)', 'title TEXT NOT NULL)') WHERE name='tasks';").unwrap();
    drop(db);
    let db = rusqlite::Connection::open(&path).unwrap();
    let check: String = db
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .unwrap();
    assert_ne!(check, "ok");
    drop(db);
    assert!(SqliteStore::open(&dir).is_err());
    let db = rusqlite::Connection::open(&path).unwrap();
    let count: i64 = db
        .query_row("SELECT count(*) FROM tasks WHERE id='broken'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 1);
    drop(db);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn backup_accepts_bare_relative_destination() {
    const CHILD: &str = "JARVIS_RELATIVE_BACKUP_TEST";
    if std::env::var_os(CHILD).is_some() {
        let store = SqliteStore::create(std::path::Path::new("source")).unwrap();
        store.backup(std::path::Path::new("snapshot")).unwrap();
        assert!(store.backup(std::path::Path::new("snapshot")).is_err());
        drop(store);
        assert!(SqliteStore::open(std::path::Path::new("snapshot")).is_ok());
        return;
    }
    let root = std::env::temp_dir().join(format!("jarvis-relative-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    private_dir(&root);
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "backup_accepts_bare_relative_destination"])
        .env(CHILD, "1")
        .current_dir(&root)
        .status()
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
    assert!(status.success());
}

#[test]
fn backup_rejects_shared_writable_parent() {
    use std::os::unix::fs::PermissionsExt;
    let root = std::env::temp_dir().join(format!("jarvis-backup-parent-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir(&root).unwrap();
    let store = SqliteStore::create(&root.join("source")).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o777)).unwrap();
    assert!(store.backup(&root.join("snapshot")).is_err());
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn restored_backup_blocks_budget_even_without_pending_reservations() {
    let root = std::env::temp_dir().join(format!("jarvis-backup-budget-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir(&root).unwrap();
    let mut store = SqliteStore::create(&root.join("source")).unwrap();
    let id = store.reserve_tokens(100, 10).unwrap().unwrap();
    store.settle_tokens(id, 5).unwrap();
    store.backup(&root.join("snapshot")).unwrap();
    let mut restored = SqliteStore::open(&root.join("snapshot")).unwrap();
    assert!(restored.reserve_tokens(100, 1).unwrap().is_none());
    assert!(restored.settle_tokens(-1, 0).is_err());
    assert!(restored.reserve_tokens(100, 1).unwrap().is_none());
    drop(restored);
    // Operator reconciliation preserves the snapshot charge and adds omitted usage.
    let db = rusqlite::Connection::open(root.join("snapshot/jarvis.sqlite3")).unwrap();
    db.execute(
        "UPDATE inference_budget SET tokens = 20, pending = 0 WHERE id = -1",
        [],
    )
    .unwrap();
    drop(db);
    let mut restored = SqliteStore::open(&root.join("snapshot")).unwrap();
    assert!(restored.reserve_tokens(100, 76).unwrap().is_none());
    assert!(restored.reserve_tokens(100, 75).unwrap().is_some());
    drop(restored);
    std::fs::remove_dir_all(root).unwrap();
}

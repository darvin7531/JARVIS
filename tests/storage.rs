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
        .query_row("SELECT reference FROM evidence WHERE task_id = 't'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(evidence, "evidence-1");
    drop(db);
    std::fs::remove_dir_all(dir).unwrap();
}

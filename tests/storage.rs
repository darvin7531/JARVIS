use jarvis::storage::{SqliteStore, Task};

#[test]
fn task_survives_reopen() {
    let dir = std::env::temp_dir().join(format!("jarvis-r02-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let task = Task { id: "task-1".into(), version: 1, title: "first".into() };
    {
        let mut store = SqliteStore::open(&dir).unwrap();
        store.create_task(&task, "created", &[]).unwrap();
    }
    let store = SqliteStore::open(&dir).unwrap();
    assert_eq!(store.task("task-1").unwrap(), Some(task));
    std::fs::remove_dir_all(dir).unwrap();
}

use rusqlite::{params, Connection, OptionalExtension};
use std::{fmt, path::Path};

#[derive(Debug, PartialEq)]
pub struct Task {
    pub id: String,
    pub version: i64,
    pub title: String,
}

#[derive(Debug)]
pub struct StorageError;

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("storage operation failed")
    }
}

impl std::error::Error for StorageError {}

impl From<rusqlite::Error> for StorageError {
    fn from(_: rusqlite::Error) -> Self {
        Self
    }
}

pub struct SqliteStore {
    db: Connection,
}

impl SqliteStore {
    pub fn open(path: &Path) -> Result<Self, StorageError> {
        std::fs::create_dir_all(path).map_err(|_| StorageError)?;
        let mut db = Connection::open(path.join("jarvis.sqlite3"))?;
        db.execute_batch("PRAGMA foreign_keys = ON;")?;
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let version: i64 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if !(0..=1).contains(&version) {
            return Err(StorageError);
        }
        if version == 0 {
            let objects: i64 = tx.query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
                [],
                |row| row.get(0),
            )?;
            if objects != 0 {
                return Err(StorageError);
            }
            tx.execute_batch("CREATE TABLE IF NOT EXISTS tasks (id TEXT PRIMARY KEY, version INTEGER NOT NULL, title TEXT NOT NULL); CREATE TABLE IF NOT EXISTS events (id INTEGER PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tasks(id), event TEXT NOT NULL); CREATE TABLE IF NOT EXISTS evidence (task_id TEXT NOT NULL REFERENCES tasks(id), reference TEXT NOT NULL); PRAGMA user_version = 1;")?;
        }
        tx.commit()?;
        Ok(Self { db })
    }

    pub fn create_task(
        &mut self,
        task: &Task,
        event: &str,
        evidence: &[String],
    ) -> Result<(), StorageError> {
        let tx = self.db.transaction()?;
        tx.execute(
            "INSERT INTO tasks (id, version, title) VALUES (?1, ?2, ?3)",
            params![task.id, task.version, task.title],
        )?;
        tx.execute(
            "INSERT INTO events (task_id, event) VALUES (?1, ?2)",
            params![task.id, event],
        )?;
        for reference in evidence {
            tx.execute(
                "INSERT INTO evidence (task_id, reference) VALUES (?1, ?2)",
                params![task.id, reference],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn update_task(&mut self, task: &Task, event: &str) -> Result<(), StorageError> {
        let previous = task.version.checked_sub(1).ok_or(StorageError)?;
        let tx = self.db.transaction()?;
        let changed = tx.execute(
            "UPDATE tasks SET version = ?1, title = ?2 WHERE id = ?3 AND version = ?4",
            params![task.version, task.title, task.id, previous],
        )?;
        if changed != 1 {
            return Err(StorageError);
        }
        tx.execute(
            "INSERT INTO events (task_id, event) VALUES (?1, ?2)",
            params![task.id, event],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn append_event(&mut self, task_id: &str, event: &str) -> Result<(), StorageError> {
        self.db.execute(
            "INSERT INTO events (task_id, event) VALUES (?1, ?2)",
            params![task_id, event],
        )?;
        Ok(())
    }

    pub fn events(&self, task_id: &str) -> Result<Vec<String>, StorageError> {
        let mut statement = self
            .db
            .prepare("SELECT event FROM events WHERE task_id = ?1 ORDER BY id")?;
        let events = statement
            .query_map([task_id], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(events)
    }

    pub fn backup(&self, path: &Path) -> Result<(), StorageError> {
        std::fs::create_dir(path).map_err(|_| StorageError)?;
        let result = (|| {
            let mut destination = Connection::open(path.join("jarvis.sqlite3"))?;
            let backup = rusqlite::backup::Backup::new(&self.db, &mut destination)?;
            backup.run_to_completion(100, std::time::Duration::from_millis(10), None)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_dir_all(path);
        }
        result
    }

    pub fn task(&self, id: &str) -> Result<Option<Task>, StorageError> {
        Ok(self
            .db
            .query_row(
                "SELECT id, version, title FROM tasks WHERE id = ?1",
                [id],
                |row| {
                    Ok(Task {
                        id: row.get(0)?,
                        version: row.get(1)?,
                        title: row.get(2)?,
                    })
                },
            )
            .optional()?)
    }
}

pub trait TaskStore {
    fn write_task(
        &mut self,
        task: &Task,
        event: &str,
        evidence: &[String],
    ) -> Result<(), StorageError>;
    fn write_event(&mut self, task_id: &str, event: &str) -> Result<(), StorageError>;
    fn read_events(&self, task_id: &str) -> Result<Vec<String>, StorageError>;
    fn read_task(&self, id: &str) -> Result<Option<Task>, StorageError>;
}

impl TaskStore for SqliteStore {
    fn write_task(
        &mut self,
        task: &Task,
        event: &str,
        evidence: &[String],
    ) -> Result<(), StorageError> {
        self.create_task(task, event, evidence)
    }
    fn write_event(&mut self, task_id: &str, event: &str) -> Result<(), StorageError> {
        self.append_event(task_id, event)
    }
    fn read_events(&self, task_id: &str) -> Result<Vec<String>, StorageError> {
        self.events(task_id)
    }
    fn read_task(&self, id: &str) -> Result<Option<Task>, StorageError> {
        self.task(id)
    }
}

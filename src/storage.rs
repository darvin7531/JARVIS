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
        let db = Connection::open(path.join("jarvis.sqlite3"))?;
        db.execute_batch("PRAGMA foreign_keys = ON; CREATE TABLE IF NOT EXISTS tasks (id TEXT PRIMARY KEY, version INTEGER NOT NULL, title TEXT NOT NULL); CREATE TABLE IF NOT EXISTS events (id INTEGER PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tasks(id), event TEXT NOT NULL);")?;
        Ok(Self { db })
    }

    pub fn create_task(
        &mut self,
        task: &Task,
        event: &str,
        _evidence: &[String],
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
        tx.commit()?;
        Ok(())
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

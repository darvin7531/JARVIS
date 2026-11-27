use rusqlite::{params, Connection, OptionalExtension};
use std::{fmt, path::Path};

#[derive(Debug, PartialEq)]
pub struct Task {
    pub id: String,
    pub version: i64,
    pub title: String,
}

#[derive(Debug, PartialEq)]
pub struct Evidence {
    pub reference: String,
    pub provenance: Option<String>,
    pub observed_at: Option<String>,
    pub classification: Option<String>,
}

#[derive(Debug, PartialEq)]
pub struct AuditEvent {
    pub event: String,
    pub revision: Option<i64>,
    pub occurred_at: Option<String>,
}

#[derive(Debug)]
pub enum StorageError {
    OperationFailed,
    UnsupportedSchema,
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::OperationFailed => "storage operation failed",
            Self::UnsupportedSchema => "unsupported schema version",
        })
    }
}

impl std::error::Error for StorageError {}

impl From<rusqlite::Error> for StorageError {
    fn from(_: rusqlite::Error) -> Self {
        Self::OperationFailed
    }
}

fn validate_schema(db: &Connection, version: i64) -> Result<(), StorageError> {
    let expected = Connection::open_in_memory()?;
    expected.execute_batch("CREATE TABLE tasks (id TEXT PRIMARY KEY, version INTEGER NOT NULL, title TEXT NOT NULL); CREATE TABLE events (id INTEGER PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tasks(id), event TEXT NOT NULL); CREATE TABLE evidence (task_id TEXT NOT NULL REFERENCES tasks(id), reference TEXT NOT NULL);")?;
    if version >= 2 {
        expected.execute_batch("CREATE INDEX events_task_id ON events(task_id, id); CREATE INDEX evidence_task_id ON evidence(task_id);")?;
    }
    if version >= 3 {
        expected.execute_batch("ALTER TABLE events ADD COLUMN revision INTEGER; ALTER TABLE events ADD COLUMN occurred_at TEXT;")?;
    }
    if version >= 4 {
        expected.execute_batch("ALTER TABLE evidence ADD COLUMN provenance TEXT; ALTER TABLE evidence ADD COLUMN observed_at TEXT; ALTER TABLE evidence ADD COLUMN classification TEXT;")?;
    }
    if version >= 5 {
        expected.execute_batch("CREATE TABLE inference_budget (id INTEGER PRIMARY KEY, tokens INTEGER NOT NULL CHECK(tokens >= 0));")?;
    }
    if version >= 6 {
        expected.execute_batch("CREATE TABLE inference_budget_limit (id INTEGER PRIMARY KEY CHECK(id = 1), tokens INTEGER CHECK(tokens >= 0));")?;
    }
    if version >= 7 {
        expected.execute_batch("ALTER TABLE inference_budget ADD COLUMN pending INTEGER NOT NULL DEFAULT 1 CHECK(pending IN (0, 1));")?;
    }
    if version >= 8 {
        expected.execute_batch("CREATE TABLE tool_approvals (id TEXT PRIMARY KEY NOT NULL, task_id TEXT NOT NULL, revision INTEGER NOT NULL, request TEXT NOT NULL, expires INTEGER NOT NULL, consumed INTEGER NOT NULL DEFAULT 0 CHECK(consumed IN (0, 1)));")?;
    }
    if version >= 9 {
        expected.execute_batch("CREATE TABLE tool_budget (id INTEGER PRIMARY KEY CHECK(id = 1), call_limit INTEGER NOT NULL CHECK(call_limit >= 0), used INTEGER NOT NULL DEFAULT 0 CHECK(used >= 0 AND used <= call_limit));")?;
    }
    if version >= 10 {
        expected.execute_batch("CREATE TABLE tool_executions (approval_id TEXT PRIMARY KEY NOT NULL REFERENCES tool_approvals(id), pending INTEGER NOT NULL CHECK(pending IN (0, 1))); ")?;
    }
    fn definitions(db: &Connection) -> Result<Vec<(String, String)>, StorageError> {
        let mut query = db.prepare(
            "SELECT name, sql FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*' ORDER BY name",
        )?;
        let definitions = query
            .query_map([], |row| {
                let sql: String = row.get(1)?;
                Ok((
                    row.get(0)?,
                    sql.split_whitespace().collect::<String>().to_lowercase(),
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(definitions)
    }
    if definitions(db)? != definitions(&expected)? {
        return Err(StorageError::UnsupportedSchema);
    }
    Ok(())
}

pub struct SqliteStore {
    db: Connection,
}

impl SqliteStore {
    pub fn create(path: &Path) -> Result<Self, StorageError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|_| StorageError::OperationFailed)?;
        }
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(path)
            .map_err(|_| StorageError::OperationFailed)?;
        let result = (|| {
            let db = Connection::open(path.join("jarvis.sqlite3"))?;
            Self::initialize(db, true)
        })();
        if result.is_err() {
            let _ = std::fs::remove_dir_all(path);
        }
        result
    }

    pub fn open(path: &Path) -> Result<Self, StorageError> {
        use std::os::unix::fs::PermissionsExt;
        let metadata =
            std::fs::symlink_metadata(path).map_err(|_| StorageError::OperationFailed)?;
        if !metadata.is_dir() || metadata.permissions().mode() & 0o077 != 0 {
            return Err(StorageError::OperationFailed);
        }
        let db_metadata = std::fs::symlink_metadata(path.join("jarvis.sqlite3"))
            .map_err(|_| StorageError::OperationFailed)?;
        if !db_metadata.is_file() {
            return Err(StorageError::OperationFailed);
        }
        let db = Connection::open_with_flags(
            path.join("jarvis.sqlite3"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )?;
        Self::initialize(db, false)
    }

    fn initialize(mut db: Connection, creating: bool) -> Result<Self, StorageError> {
        db.busy_timeout(std::time::Duration::from_millis(100))?;
        db.execute_batch("PRAGMA foreign_keys = ON;")?;
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let version: i64 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if !(0..=10).contains(&version) {
            return Err(StorageError::UnsupportedSchema);
        }
        if version > 0 {
            validate_schema(&tx, version)?;
            let integrity: String = tx.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
            let foreign_key_error = tx
                .prepare("PRAGMA foreign_key_check")?
                .query([])?
                .next()?
                .is_some();
            if integrity != "ok" || foreign_key_error {
                return Err(StorageError::OperationFailed);
            }
        }
        if version == 0 {
            if !creating {
                return Err(StorageError::UnsupportedSchema);
            }
            let objects: i64 = tx.query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*'",
                [],
                |row| row.get(0),
            )?;
            if objects != 0 {
                return Err(StorageError::OperationFailed);
            }
            tx.execute_batch("CREATE TABLE IF NOT EXISTS tasks (id TEXT PRIMARY KEY, version INTEGER NOT NULL, title TEXT NOT NULL); CREATE TABLE IF NOT EXISTS events (id INTEGER PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tasks(id), event TEXT NOT NULL); CREATE TABLE IF NOT EXISTS evidence (task_id TEXT NOT NULL REFERENCES tasks(id), reference TEXT NOT NULL); PRAGMA user_version = 1;")?;
        }
        if version < 2 {
            tx.execute_batch("CREATE INDEX events_task_id ON events(task_id, id); CREATE INDEX evidence_task_id ON evidence(task_id); PRAGMA user_version = 2;")?;
        }
        if version < 3 {
            tx.execute_batch("ALTER TABLE events ADD COLUMN revision INTEGER; ALTER TABLE events ADD COLUMN occurred_at TEXT; PRAGMA user_version = 3;")?;
        }
        if version < 4 {
            tx.execute_batch("ALTER TABLE evidence ADD COLUMN provenance TEXT; ALTER TABLE evidence ADD COLUMN observed_at TEXT; ALTER TABLE evidence ADD COLUMN classification TEXT; PRAGMA user_version = 4;")?;
        }
        if version < 5 {
            tx.execute_batch("CREATE TABLE inference_budget (id INTEGER PRIMARY KEY, tokens INTEGER NOT NULL CHECK(tokens >= 0)); PRAGMA user_version = 5;")?;
        }
        if version < 6 {
            tx.execute_batch("CREATE TABLE inference_budget_limit (id INTEGER PRIMARY KEY CHECK(id = 1), tokens INTEGER CHECK(tokens >= 0)); INSERT INTO inference_budget_limit SELECT 1, NULL WHERE EXISTS (SELECT 1 FROM inference_budget); PRAGMA user_version = 6;")?;
        }
        if version < 7 {
            tx.execute_batch("ALTER TABLE inference_budget ADD COLUMN pending INTEGER NOT NULL DEFAULT 1 CHECK(pending IN (0, 1)); PRAGMA user_version = 7;")?;
        }
        if version < 8 {
            tx.execute_batch("CREATE TABLE tool_approvals (id TEXT PRIMARY KEY NOT NULL, task_id TEXT NOT NULL, revision INTEGER NOT NULL, request TEXT NOT NULL, expires INTEGER NOT NULL, consumed INTEGER NOT NULL DEFAULT 0 CHECK(consumed IN (0, 1))); PRAGMA user_version = 8;")?;
        }
        if version < 9 {
            tx.execute_batch("CREATE TABLE tool_budget (id INTEGER PRIMARY KEY CHECK(id = 1), call_limit INTEGER NOT NULL CHECK(call_limit >= 0), used INTEGER NOT NULL DEFAULT 0 CHECK(used >= 0 AND used <= call_limit)); PRAGMA user_version = 9;")?;
        }
        if version < 10 {
            tx.execute_batch("CREATE TABLE tool_executions (approval_id TEXT PRIMARY KEY NOT NULL REFERENCES tool_approvals(id), pending INTEGER NOT NULL CHECK(pending IN (0, 1))); INSERT INTO tool_executions SELECT id, 1 FROM tool_approvals WHERE consumed = 1; PRAGMA user_version = 10;")?;
        }
        tx.commit()?;
        Ok(Self { db })
    }

    // Trusted owner only: untrusted requests must never reach this method.
    pub fn issue_tool_approval(
        &mut self,
        id: &str,
        task: &str,
        revision: i64,
        request: &crate::tool_policy::ToolRequest,
        expires: std::time::SystemTime,
    ) -> Result<(), StorageError> {
        let expires = i64::try_from(
            expires
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| StorageError::OperationFailed)?
                .as_secs(),
        )
        .map_err(|_| StorageError::OperationFailed)?;
        if id.is_empty()
            || task.is_empty()
            || revision < 0
            || request.tool != "inert"
            || request.action != "echo"
            || request.resource.is_empty()
            || request.resource.len() > 128
            || crate::tool_policy::validate_tool(&request.tool, &request.arguments).is_err()
        {
            return Err(StorageError::OperationFailed);
        }
        let request = serde_json::to_string(request).map_err(|_| StorageError::OperationFailed)?;
        let changed = self.db.execute("INSERT INTO tool_approvals (id, task_id, revision, request, expires) SELECT ?1, id, version, ?4, ?5 FROM tasks WHERE id = ?2 AND version = ?3", params![id, task, revision, request, expires])?;
        if changed != 1 {
            return Err(StorageError::OperationFailed);
        }
        Ok(())
    }

    /// Trusted owner establishes a workspace-wide immutable call limit once.
    pub fn initialize_tool_budget(&mut self, limit: u64) -> Result<(), StorageError> {
        let limit = i64::try_from(limit).map_err(|_| StorageError::OperationFailed)?;
        self.db.execute(
            "INSERT INTO tool_budget (id, call_limit) VALUES (1, ?1)",
            [limit],
        )?;
        Ok(())
    }

    pub(crate) fn consume_tool_approval(
        &mut self,
        id: &str,
        task: &str,
        revision: i64,
        request: &crate::tool_policy::ToolRequest,
        contract_expires: std::time::SystemTime,
    ) -> Result<bool, StorageError> {
        let request = serde_json::to_string(request).map_err(|_| StorageError::OperationFailed)?;
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let now = i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| StorageError::OperationFailed)?
                .as_secs(),
        )
        .map_err(|_| StorageError::OperationFailed)?;
        if std::time::SystemTime::now() >= contract_expires {
            return Ok(false);
        }
        let changed = tx.execute("UPDATE tool_approvals SET consumed = 1 WHERE id = ?1 AND task_id = ?2 AND revision = ?3 AND request = ?4 AND expires > ?5 AND consumed = 0 AND EXISTS (SELECT 1 FROM tasks WHERE id = ?2 AND version = ?3)", params![id, task, revision, request, now])?;
        if changed != 1 {
            return Ok(false);
        }
        // A backup marker blocks admission until omitted work is reconciled.
        let reserved = tx.execute(
            "UPDATE tool_budget SET used = used + 1 WHERE id = 1 AND used < call_limit AND NOT EXISTS (SELECT 1 FROM inference_budget WHERE id = -1 AND pending = 1) AND NOT EXISTS (SELECT 1 FROM tool_executions WHERE pending = 1)",
            [],
        )?;
        if reserved != 1 {
            return Ok(false);
        }
        // ponytail: one in-flight tool call per workspace; use per-resource
        // reconciliation before allowing concurrent executions.
        tx.execute(
            "INSERT INTO tool_executions (approval_id, pending) VALUES (?1, 1)",
            [id],
        )?;
        let event =
            serde_json::json!({"type": "tool_execution_intent", "approval_id": id}).to_string();
        tx.execute(
            "INSERT INTO events (task_id, event, revision, occurred_at) VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![task, event, revision],
        )?;
        tx.commit()?;
        Ok(true)
    }

    pub(crate) fn tool_approval_is_fresh(&self, id: &str) -> Result<bool, StorageError> {
        let now = i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| StorageError::OperationFailed)?
                .as_secs(),
        )
        .map_err(|_| StorageError::OperationFailed)?;
        Ok(self.db.query_row(
            "SELECT expires > ?2 FROM tool_approvals WHERE id = ?1 AND consumed = 1",
            params![id, now],
            |row| row.get(0),
        )?)
    }

    pub(crate) fn record_tool_outcome(
        &mut self,
        id: &str,
        task: &str,
        revision: i64,
    ) -> Result<(), StorageError> {
        let event = serde_json::json!({"type": "tool_execution_outcome", "approval_id": id, "outcome": "succeeded"}).to_string();
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if tx.execute(
            "UPDATE tool_executions SET pending = 0 WHERE approval_id = ?1 AND pending = 1",
            [id],
        )? != 1
        {
            return Err(StorageError::OperationFailed);
        }
        tx.execute(
            "INSERT INTO events (task_id, event, revision, occurred_at) VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![task, event, revision],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Trusted owner resolves an uncertain tool execution without replay.
    pub fn reconcile_tool_execution(&mut self, id: &str) -> Result<(), StorageError> {
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let (task, revision): (String, i64) = tx.query_row(
            "SELECT a.task_id, a.revision FROM tool_approvals a JOIN tool_executions e ON e.approval_id = a.id WHERE a.id = ?1 AND a.consumed = 1 AND e.pending = 1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if tx.execute(
            "UPDATE tool_executions SET pending = 0 WHERE approval_id = ?1 AND pending = 1",
            [id],
        )? != 1
        {
            return Err(StorageError::OperationFailed);
        }
        let event = serde_json::json!({"type": "tool_execution_reconciliation", "approval_id": id, "outcome": "unknown"}).to_string();
        tx.execute(
            "INSERT INTO events (task_id, event, revision, occurred_at) VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![task, event, revision],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Atomically reserve a conservative token allowance across all processes.
    pub fn reserve_tokens(&mut self, limit: u64, amount: u64) -> Result<Option<i64>, StorageError> {
        let limit = i64::try_from(limit).map_err(|_| StorageError::OperationFailed)?;
        let amount = i64::try_from(amount).map_err(|_| StorageError::OperationFailed)?;
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT OR IGNORE INTO inference_budget_limit (id, tokens) VALUES (1, ?1)",
            [limit],
        )?;
        let matches: bool = tx.query_row(
            "SELECT coalesce(tokens = ?1, 0) FROM inference_budget_limit WHERE id = 1",
            [limit],
            |r| r.get(0),
        )?;
        if !matches {
            return Err(StorageError::OperationFailed);
        }
        // ponytail: one unresolved request per workspace; add a durable per-request
        // accounting protocol before allowing concurrent provider calls.
        let pending: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM inference_budget WHERE pending = 1)",
            [],
            |r| r.get(0),
        )?;
        if pending {
            tx.commit()?;
            return Ok(None);
        }
        let used: i64 = tx.query_row(
            "SELECT coalesce(sum(tokens), 0) FROM inference_budget",
            [],
            |r| r.get(0),
        )?;
        if amount > limit || used > limit - amount {
            tx.commit()?;
            return Ok(None);
        }
        tx.execute(
            "INSERT INTO inference_budget (tokens) VALUES (?1)",
            [amount],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(Some(id))
    }

    /// Replace a reservation only after terminal usage has been validated.
    pub fn settle_tokens(&mut self, id: i64, tokens: u64) -> Result<(), StorageError> {
        let tokens = i64::try_from(tokens).map_err(|_| StorageError::OperationFailed)?;
        if self.db.execute(
            "UPDATE inference_budget SET tokens = ?1, pending = 0 WHERE id = ?2 AND id != -1 AND pending = 1",
            params![tokens, id],
        )? != 1
        {
            return Err(StorageError::OperationFailed);
        }
        Ok(())
    }

    pub fn create_task(
        &mut self,
        task: &Task,
        event: &str,
        evidence: &[String],
    ) -> Result<(), StorageError> {
        let evidence = evidence
            .iter()
            .map(|reference| Evidence {
                reference: reference.clone(),
                provenance: None,
                observed_at: None,
                classification: None,
            })
            .collect::<Vec<_>>();
        self.create_task_with_evidence(task, event, &evidence)
    }

    pub fn create_task_with_evidence(
        &mut self,
        task: &Task,
        event: &str,
        evidence: &[Evidence],
    ) -> Result<(), StorageError> {
        for item in evidence {
            if [&item.provenance, &item.classification]
                .into_iter()
                .flatten()
                .any(|value| value.trim().is_empty() || value.len() > 1024)
            {
                return Err(StorageError::OperationFailed);
            }
            if let Some(time) = &item.observed_at {
                let valid: bool = self.db.query_row(
                    "SELECT coalesce(strftime('%Y-%m-%dT%H:%M:%SZ', ?1, '+0 days') = ?1 OR strftime('%Y-%m-%dT%H:%M:%fZ', ?1, '+0 days') = ?1, 0)",
                    [time],
                    |row| row.get(0),
                )?;
                if !valid {
                    return Err(StorageError::OperationFailed);
                }
            }
        }
        let tx = self.db.transaction()?;
        tx.execute(
            "INSERT INTO tasks (id, version, title) VALUES (?1, ?2, ?3)",
            params![task.id, task.version, task.title],
        )?;
        tx.execute(
            "INSERT INTO events (task_id, event, revision, occurred_at) VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![task.id, event, task.version],
        )?;
        for item in evidence {
            tx.execute(
                "INSERT INTO evidence (task_id, reference, provenance, observed_at, classification) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![task.id, item.reference, item.provenance, item.observed_at, item.classification],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn evidence_metadata(&self, task_id: &str) -> Result<Vec<Evidence>, StorageError> {
        let mut statement = self.db.prepare(
            "SELECT reference, provenance, observed_at, classification FROM evidence WHERE task_id = ?1 ORDER BY rowid",
        )?;
        let rows = statement.query_map([task_id], |row| {
            Ok(Evidence {
                reference: row.get(0)?,
                provenance: row.get(1)?,
                observed_at: row.get(2)?,
                classification: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn audit_metadata(&self, task_id: &str) -> Result<Vec<AuditEvent>, StorageError> {
        let mut statement = self.db.prepare(
            "SELECT event, revision, occurred_at FROM events WHERE task_id = ?1 ORDER BY id",
        )?;
        let rows = statement.query_map([task_id], |row| {
            Ok(AuditEvent {
                event: row.get(0)?,
                revision: row.get(1)?,
                occurred_at: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn update_task(&mut self, task: &Task, event: &str) -> Result<(), StorageError> {
        let previous = task
            .version
            .checked_sub(1)
            .ok_or(StorageError::OperationFailed)?;
        let tx = self.db.transaction()?;
        let changed = tx.execute(
            "UPDATE tasks SET version = ?1, title = ?2 WHERE id = ?3 AND version = ?4 AND NOT EXISTS (SELECT 1 FROM tool_executions e JOIN tool_approvals a ON a.id = e.approval_id WHERE e.pending = 1 AND a.task_id = ?3)",
            params![task.version, task.title, task.id, previous],
        )?;
        if changed != 1 {
            return Err(StorageError::OperationFailed);
        }
        tx.execute(
            "INSERT INTO events (task_id, event, revision, occurred_at) VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![task.id, event, task.version],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn append_event(&mut self, task_id: &str, event: &str) -> Result<(), StorageError> {
        let changed = self.db.execute(
            "INSERT INTO events (task_id, event, revision, occurred_at) SELECT id, ?2, version, strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM tasks WHERE id = ?1",
            params![task_id, event],
        )?;
        if changed != 1 {
            return Err(StorageError::OperationFailed);
        }
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
        self.backup_with_step(path, || {})
    }

    fn backup_with_step(
        &self,
        path: &Path,
        mut after_step: impl FnMut(),
    ) -> Result<(), StorageError> {
        use std::os::unix::{
            ffi::OsStrExt,
            fs::{DirBuilderExt, MetadataExt},
        };
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let parent = path.parent().ok_or(StorageError::OperationFailed)?;
        let parent = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };
        let parent_metadata =
            std::fs::symlink_metadata(parent).map_err(|_| StorageError::OperationFailed)?;
        if !parent_metadata.is_dir()
            || parent_metadata.uid() != unsafe { libc::geteuid() }
            || parent_metadata.mode() & 0o022 != 0
        {
            return Err(StorageError::OperationFailed);
        }
        let temporary = parent.join(format!(
            ".jarvis-backup-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&temporary)
            .map_err(|_| StorageError::OperationFailed)?;
        let result = (|| {
            {
                let mut destination = Connection::open(temporary.join("jarvis.sqlite3"))?;
                destination.busy_timeout(std::time::Duration::from_millis(100))?;
                {
                    let backup = rusqlite::backup::Backup::new(&self.db, &mut destination)?;
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
                    loop {
                        match backup.step(100)? {
                            rusqlite::backup::StepResult::Done => break,
                            _ if std::time::Instant::now() >= deadline => {
                                return Err(StorageError::OperationFailed);
                            }
                            rusqlite::backup::StepResult::More => {
                                after_step();
                                std::thread::sleep(std::time::Duration::from_millis(10));
                            }
                            _ => std::thread::sleep(std::time::Duration::from_millis(10)),
                        }
                    }
                }
                // A snapshot cannot establish how much was spent after it was taken.
                destination.execute_batch("BEGIN IMMEDIATE; INSERT INTO inference_budget (id, tokens, pending) VALUES (-1, 0, 1) ON CONFLICT(id) DO UPDATE SET pending = 1; UPDATE tool_approvals SET consumed = 1 WHERE consumed = 0; COMMIT;")?;
            }
            std::fs::File::open(temporary.join("jarvis.sqlite3"))
                .and_then(|file| file.sync_all())
                .map_err(|_| StorageError::OperationFailed)?;
            std::fs::File::open(&temporary)
                .and_then(|dir| dir.sync_all())
                .map_err(|_| StorageError::OperationFailed)?;
            let from = std::ffi::CString::new(temporary.as_os_str().as_bytes())
                .map_err(|_| StorageError::OperationFailed)?;
            let to = std::ffi::CString::new(path.as_os_str().as_bytes())
                .map_err(|_| StorageError::OperationFailed)?;
            unsafe extern "C" {
                fn renameat2(
                    olddirfd: i32,
                    oldpath: *const i8,
                    newdirfd: i32,
                    newpath: *const i8,
                    flags: u32,
                ) -> i32;
            }
            // RENAME_NOREPLACE publishes the completed directory without replacing a workspace.
            if unsafe { renameat2(-100, from.as_ptr(), -100, to.as_ptr(), 1) } != 0 {
                return Err(StorageError::OperationFailed);
            }
            std::fs::File::open(parent)
                .and_then(|dir| dir.sync_all())
                .map_err(|_| StorageError::OperationFailed)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_dir_all(&temporary);
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
    fn revise_task(&mut self, task: &Task, event: &str) -> Result<(), StorageError>;
    fn read_events(&self, task_id: &str) -> Result<Vec<String>, StorageError>;
    fn read_evidence(&self, task_id: &str) -> Result<Vec<String>, StorageError>;
    fn read_task(&self, id: &str) -> Result<Option<Task>, StorageError>;
    fn write_task_with_evidence(
        &mut self,
        task: &Task,
        event: &str,
        evidence: &[Evidence],
    ) -> Result<(), StorageError>;
    fn read_evidence_metadata(&self, task_id: &str) -> Result<Vec<Evidence>, StorageError>;
    fn read_audit_metadata(&self, task_id: &str) -> Result<Vec<AuditEvent>, StorageError>;
}

impl TaskStore for SqliteStore {
    fn write_task_with_evidence(
        &mut self,
        task: &Task,
        event: &str,
        evidence: &[Evidence],
    ) -> Result<(), StorageError> {
        self.create_task_with_evidence(task, event, evidence)
    }
    fn read_evidence_metadata(&self, task_id: &str) -> Result<Vec<Evidence>, StorageError> {
        self.evidence_metadata(task_id)
    }
    fn read_audit_metadata(&self, task_id: &str) -> Result<Vec<AuditEvent>, StorageError> {
        self.audit_metadata(task_id)
    }

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
    fn revise_task(&mut self, task: &Task, event: &str) -> Result<(), StorageError> {
        self.update_task(task, event)
    }
    fn read_events(&self, task_id: &str) -> Result<Vec<String>, StorageError> {
        self.events(task_id)
    }
    fn read_evidence(&self, task_id: &str) -> Result<Vec<String>, StorageError> {
        let mut statement = self
            .db
            .prepare("SELECT reference FROM evidence WHERE task_id = ?1 ORDER BY rowid")?;
        let references = statement
            .query_map([task_id], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(references)
    }
    fn read_task(&self, id: &str) -> Result<Option<Task>, StorageError> {
        self.task(id)
    }
}

#[cfg(test)]
mod backup_tests {
    use super::*;
    use std::{sync::mpsc, time::Duration};

    #[test]
    fn wal_backup_restores_complete_history_after_mid_backup_commits() {
        let root = std::env::temp_dir().join(format!("jarvis-wal-steps-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let source = root.join("source");
        let snapshot = root.join("snapshot");
        let mut store = SqliteStore::create(&source).unwrap();
        store.db.pragma_update(None, "journal_mode", "WAL").unwrap();
        let task = Task {
            id: "wal".into(),
            version: 1,
            title: "x".repeat(600_000),
        };
        let initial = Evidence {
            reference: "ref-1".into(),
            provenance: Some("source-1".into()),
            observed_at: Some("2026-11-26T12:00:00Z".into()),
            classification: Some("internal".into()),
        };
        store
            .create_task_with_evidence(&task, "created", &[initial])
            .unwrap();
        let (start_tx, start_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let path = source.join("jarvis.sqlite3");
        let writer = std::thread::spawn(move || {
            let mut db = Connection::open(path).unwrap();
            db.busy_timeout(Duration::from_secs(2)).unwrap();
            start_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            for revision in 2..=4 {
                let tx = db.transaction().unwrap();
                tx.execute("UPDATE tasks SET version = ?1 WHERE id = 'wal'", [revision])
                    .unwrap();
                tx.execute("INSERT INTO events (task_id, event, revision, occurred_at) VALUES ('wal', 'write', ?1, 'now')", [revision]).unwrap();
                tx.execute("INSERT INTO evidence (task_id, reference, provenance, observed_at, classification) VALUES ('wal', ?1, ?2, '2026-11-26T12:00:00Z', 'internal')", params![format!("ref-{revision}"), format!("source-{revision}")]).unwrap();
                tx.commit().unwrap();
            }
            done_tx.send(()).unwrap();
        });
        let mut started = false;
        store
            .backup_with_step(&snapshot, || {
                if !started {
                    started = true;
                    start_tx.send(()).unwrap();
                    done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
                }
            })
            .unwrap();
        writer.join().unwrap();
        assert!(
            started,
            "backup must perform an incomplete step before the commits"
        );
        let restored = SqliteStore::open(&snapshot).unwrap();
        assert_eq!(restored.task("wal").unwrap().unwrap().version, 4);
        let audit = restored.audit_metadata("wal").unwrap();
        assert_eq!(audit.len(), 4);
        for (index, row) in audit.iter().enumerate() {
            assert_eq!(row.revision, Some(index as i64 + 1));
            assert_eq!(row.event, if index == 0 { "created" } else { "write" });
            assert!(row.occurred_at.as_deref().is_some_and(|s| !s.is_empty()));
        }
        let evidence = restored.evidence_metadata("wal").unwrap();
        assert_eq!(evidence.len(), 4);
        for (index, row) in evidence.iter().enumerate() {
            assert_eq!(row.reference, format!("ref-{}", index + 1));
            assert_eq!(
                row.provenance.as_deref(),
                Some(format!("source-{}", index + 1).as_str())
            );
            assert_eq!(row.observed_at.as_deref(), Some("2026-11-26T12:00:00Z"));
            assert_eq!(row.classification.as_deref(), Some("internal"));
        }
        drop(restored);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}

use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Lifecycle {
    Started,
    Stopped,
}

#[derive(Serialize)]
pub struct Event {
    pub event: Lifecycle,
}

pub mod storage {
    use std::path::Path;

    #[derive(Debug, PartialEq)]
    pub struct Task {
        pub id: String,
        pub version: i64,
        pub title: String,
    }
    pub struct SqliteStore;
    impl SqliteStore {
        pub fn open(_: &Path) -> Result<Self, &'static str> {
            Ok(Self)
        }
        pub fn create_task(&mut self, _: &Task, _: &str, _: &[String]) -> Result<(), &'static str> {
            Ok(())
        }
        pub fn task(&self, _: &str) -> Result<Option<Task>, &'static str> {
            Ok(None)
        }
    }
}

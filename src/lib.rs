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

pub mod inference;
pub mod storage;

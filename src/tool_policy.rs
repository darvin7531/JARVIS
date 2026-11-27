#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Read,
    Write,
}

pub fn permits(tool: &str, allowed: &[Scope]) -> bool {
    let _ = (tool, allowed);
    false
}

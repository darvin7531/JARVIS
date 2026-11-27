#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Read,
    Write,
}

pub fn permits(tool: &str, allowed: &[Scope]) -> bool {
    match tool {
        "read" => allowed.contains(&Scope::Read),
        "write" => allowed.contains(&Scope::Write),
        _ => false,
    }
}

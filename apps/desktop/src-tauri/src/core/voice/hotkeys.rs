#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hotkey(pub String);

impl Hotkey {
    pub fn parse(value: &str) -> Option<Self> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return None;
        }
        Some(Self(trimmed.to_string()))
    }
}

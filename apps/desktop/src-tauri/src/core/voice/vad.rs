#![allow(dead_code)]

#[derive(Debug, Clone, Copy)]
pub struct VoiceActivityDetector {
    threshold: f32,
}

impl VoiceActivityDetector {
    pub const fn new(threshold: f32) -> Self {
        Self { threshold }
    }

    pub fn is_speaking(&self, level: f32) -> bool {
        level >= self.threshold
    }
}

impl Default for VoiceActivityDetector {
    fn default() -> Self {
        Self::new(0.25)
    }
}

#![allow(dead_code)]

#[derive(Debug, Clone)]
pub struct VoiceActivityDetector {
    on_threshold: f32,
    off_threshold: f32,
    hold_frames: u32,
    hold_remaining: u32,
    speaking: bool,
}

impl VoiceActivityDetector {
    pub const fn new(threshold: f32) -> Self {
        let off_threshold = threshold * 0.7;
        Self {
            on_threshold: threshold,
            off_threshold,
            hold_frames: 3,
            hold_remaining: 0,
            speaking: false,
        }
    }

    pub fn is_speaking(&mut self, level: f32) -> bool {
        if self.speaking {
            if level >= self.off_threshold {
                self.hold_remaining = self.hold_frames;
                return true;
            }
            if self.hold_remaining > 0 {
                self.hold_remaining -= 1;
                return true;
            }
            self.speaking = false;
            return false;
        }

        if level >= self.on_threshold {
            self.speaking = true;
            self.hold_remaining = self.hold_frames;
            return true;
        }

        false
    }
}

impl Default for VoiceActivityDetector {
    fn default() -> Self {
        Self::new(0.25)
    }
}

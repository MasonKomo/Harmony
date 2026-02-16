pub mod audio_in;
pub mod audio_out;
pub mod client;
pub mod codec;
pub mod hotkeys;
pub mod vad;

pub use client::{VoiceService, VoiceSharedState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
}

pub fn list_input_devices() -> Vec<AudioDevice> {
    audio_in::list_input_devices()
}

pub fn list_output_devices() -> Vec<AudioDevice> {
    audio_out::list_output_devices()
}

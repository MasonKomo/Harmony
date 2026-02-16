use cpal::traits::{DeviceTrait, HostTrait};

use super::AudioDevice;

pub fn list_output_devices() -> Vec<AudioDevice> {
    let host = cpal::default_host();

    host.output_devices()
        .ok()
        .map(|devices| {
            devices
                .enumerate()
                .map(|(idx, device)| {
                    let name = device
                        .name()
                        .unwrap_or_else(|_| format!("Output Device {}", idx + 1));
                    AudioDevice {
                        id: name.clone(),
                        name,
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

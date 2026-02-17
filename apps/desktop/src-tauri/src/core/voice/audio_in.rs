use std::sync::mpsc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, StreamConfig};

use super::AudioDevice;

pub struct InputCapture {
    _stream: cpal::Stream,
    sample_rate: u32,
    receiver: mpsc::Receiver<Vec<f32>>,
}

impl InputCapture {
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn drain_samples(&self, target: &mut Vec<f32>) {
        while let Ok(chunk) = self.receiver.try_recv() {
            target.extend(chunk);
        }
    }
}

pub fn list_input_devices() -> Vec<AudioDevice> {
    let host = cpal::default_host();

    host.input_devices()
        .ok()
        .map(|devices| {
            devices
                .enumerate()
                .map(|(idx, device)| {
                    let name = device
                        .name()
                        .unwrap_or_else(|_| format!("Input Device {}", idx + 1));
                    AudioDevice {
                        id: name.clone(),
                        name,
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

pub fn start_input_capture(selected_device_id: Option<&str>) -> Result<InputCapture, String> {
    let host = cpal::default_host();
    let device = resolve_input_device(&host, selected_device_id)?;
    let supported = device
        .default_input_config()
        .map_err(|err| format!("failed to query default input config: {err}"))?;
    let sample_rate = supported.sample_rate().0;
    let sample_format = supported.sample_format();
    let stream_config: StreamConfig = supported.into();

    let channels = usize::from(stream_config.channels);
    let (sender, receiver) = mpsc::channel::<Vec<f32>>();
    let err_fn = move |err| {
        log::warn!("input stream error: {err}");
    };

    let stream = match sample_format {
        SampleFormat::I8 => {
            build_input_stream::<i8>(&device, &stream_config, channels, sender, err_fn)?
        }
        SampleFormat::I16 => {
            build_input_stream::<i16>(&device, &stream_config, channels, sender, err_fn)?
        }
        SampleFormat::I32 => {
            build_input_stream::<i32>(&device, &stream_config, channels, sender, err_fn)?
        }
        SampleFormat::U8 => {
            build_input_stream::<u8>(&device, &stream_config, channels, sender, err_fn)?
        }
        SampleFormat::U16 => {
            build_input_stream::<u16>(&device, &stream_config, channels, sender, err_fn)?
        }
        SampleFormat::U32 => {
            build_input_stream::<u32>(&device, &stream_config, channels, sender, err_fn)?
        }
        SampleFormat::F32 => {
            build_input_stream::<f32>(&device, &stream_config, channels, sender, err_fn)?
        }
        SampleFormat::F64 => {
            build_input_stream::<f64>(&device, &stream_config, channels, sender, err_fn)?
        }
        other => {
            return Err(format!("unsupported input sample format: {other:?}"));
        }
    };

    stream
        .play()
        .map_err(|err| format!("failed to start input stream: {err}"))?;

    Ok(InputCapture {
        _stream: stream,
        sample_rate,
        receiver,
    })
}

fn resolve_input_device(
    host: &cpal::Host,
    selected_device_id: Option<&str>,
) -> Result<cpal::Device, String> {
    if let Some(target_id) = selected_device_id {
        let devices = host
            .input_devices()
            .map_err(|err| format!("failed to enumerate input devices: {err}"))?;
        for device in devices {
            let Ok(name) = device.name() else {
                continue;
            };
            if name == target_id {
                return Ok(device);
            }
        }
    }

    host.default_input_device()
        .ok_or_else(|| "no input device available".to_string())
}

fn build_input_stream<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    channels: usize,
    sender: mpsc::Sender<Vec<f32>>,
    err_fn: impl Fn(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream, String>
where
    T: Sample + cpal::SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                if channels == 0 {
                    return;
                }

                let frames = data.len() / channels;
                let mut mono = Vec::with_capacity(frames);
                for frame in data.chunks(channels) {
                    let mut sum = 0.0_f32;
                    for sample in frame {
                        sum += f32::from_sample(*sample);
                    }
                    mono.push(sum / channels as f32);
                }

                let _ = sender.send(mono);
            },
            err_fn,
            None,
        )
        .map_err(|err| format!("failed to build input stream: {err}"))
}

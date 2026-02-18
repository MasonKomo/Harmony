use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, StreamConfig};

use super::AudioDevice;

const CLIP_THRESHOLD: f32 = 0.995;

#[derive(Debug, Clone, Copy, Default)]
pub struct InputCaptureStats {
    pub clipped_frames: u64,
    pub delivered_chunks: u64,
    pub dropped_chunks: u64,
}

#[derive(Default)]
struct InputStatsAtomic {
    clipped_frames: AtomicU64,
    delivered_chunks: AtomicU64,
    dropped_chunks: AtomicU64,
}

impl InputStatsAtomic {
    fn snapshot(&self) -> InputCaptureStats {
        InputCaptureStats {
            clipped_frames: self.clipped_frames.load(Ordering::Relaxed),
            delivered_chunks: self.delivered_chunks.load(Ordering::Relaxed),
            dropped_chunks: self.dropped_chunks.load(Ordering::Relaxed),
        }
    }
}

pub struct InputCapture {
    _stream: cpal::Stream,
    sample_rate: u32,
    device_name: String,
    stats: Arc<InputStatsAtomic>,
    receiver: mpsc::Receiver<Vec<f32>>,
}

impl InputCapture {
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    pub fn stats_snapshot(&self) -> InputCaptureStats {
        self.stats.snapshot()
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
    let device_name = device
        .name()
        .unwrap_or_else(|_| "Unknown Input".to_string());
    let supported = device
        .default_input_config()
        .map_err(|err| format!("failed to query default input config: {err}"))?;
    let sample_rate = supported.sample_rate().0;
    let sample_format = supported.sample_format();
    let stream_config: StreamConfig = supported.into();

    let channels = usize::from(stream_config.channels);
    let (sender, receiver) = mpsc::channel::<Vec<f32>>();
    let stats = Arc::new(InputStatsAtomic::default());
    let err_fn = move |err| {
        log::warn!("input stream error: {err}");
    };

    let stream = match sample_format {
        SampleFormat::I8 => build_input_stream::<i8>(
            &device,
            &stream_config,
            channels,
            sender,
            Arc::clone(&stats),
            err_fn,
        )?,
        SampleFormat::I16 => build_input_stream::<i16>(
            &device,
            &stream_config,
            channels,
            sender,
            Arc::clone(&stats),
            err_fn,
        )?,
        SampleFormat::I32 => build_input_stream::<i32>(
            &device,
            &stream_config,
            channels,
            sender,
            Arc::clone(&stats),
            err_fn,
        )?,
        SampleFormat::U8 => build_input_stream::<u8>(
            &device,
            &stream_config,
            channels,
            sender,
            Arc::clone(&stats),
            err_fn,
        )?,
        SampleFormat::U16 => build_input_stream::<u16>(
            &device,
            &stream_config,
            channels,
            sender,
            Arc::clone(&stats),
            err_fn,
        )?,
        SampleFormat::U32 => build_input_stream::<u32>(
            &device,
            &stream_config,
            channels,
            sender,
            Arc::clone(&stats),
            err_fn,
        )?,
        SampleFormat::F32 => build_input_stream::<f32>(
            &device,
            &stream_config,
            channels,
            sender,
            Arc::clone(&stats),
            err_fn,
        )?,
        SampleFormat::F64 => build_input_stream::<f64>(
            &device,
            &stream_config,
            channels,
            sender,
            Arc::clone(&stats),
            err_fn,
        )?,
        other => {
            return Err(format!("unsupported input sample format: {other:?}"));
        }
    };

    stream
        .play()
        .map_err(|err| format!("failed to start input stream: {err}"))?;

    log::info!(
        "input stream started: device=\"{}\" sample_rate={} channels={} format={:?}",
        device_name,
        sample_rate,
        stream_config.channels,
        sample_format
    );

    Ok(InputCapture {
        _stream: stream,
        sample_rate,
        device_name,
        stats,
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
    stats: Arc<InputStatsAtomic>,
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
                    let mut frame_clipped = false;
                    for sample in frame {
                        let value = f32::from_sample(*sample);
                        frame_clipped = frame_clipped || value.abs() >= CLIP_THRESHOLD;
                        sum += value;
                    }
                    mono.push(sum / channels as f32);
                    if frame_clipped {
                        stats.clipped_frames.fetch_add(1, Ordering::Relaxed);
                    }
                }

                if sender.send(mono).is_ok() {
                    stats.delivered_chunks.fetch_add(1, Ordering::Relaxed);
                } else {
                    stats.dropped_chunks.fetch_add(1, Ordering::Relaxed);
                }
            },
            err_fn,
            None,
        )
        .map_err(|err| format!("failed to build input stream: {err}"))
}

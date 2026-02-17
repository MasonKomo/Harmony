use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, StreamConfig};

use super::AudioDevice;

const OUTPUT_QUEUE_LIMIT: usize = 48_000 * 3;
const UNDERFLOW_FADE: f32 = 0.96;
const QUEUE_LOG_WINDOW_PUSHES: u32 = 120;

pub struct OutputPlayback {
    _stream: cpal::Stream,
    state: Arc<Mutex<PlaybackState>>,
}

impl OutputPlayback {
    pub fn push_mono_48k(&self, samples: &[f32]) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        state.samples.extend(samples.iter().copied());
        if state.samples.len() > OUTPUT_QUEUE_LIMIT {
            let overflow = state.samples.len() - OUTPUT_QUEUE_LIMIT;
            state.samples.drain(..overflow);
        }

        let depth = state.samples.len();
        if state.pushes_since_log == 0 {
            state.window_min_depth = depth;
            state.window_max_depth = depth;
        } else {
            state.window_min_depth = state.window_min_depth.min(depth);
            state.window_max_depth = state.window_max_depth.max(depth);
        }
        state.pushes_since_log = state.pushes_since_log.saturating_add(1);
        if state.pushes_since_log >= QUEUE_LOG_WINDOW_PUSHES {
            log::debug!(
                "output queue depth window={}..{} samples underflows={}",
                state.window_min_depth,
                state.window_max_depth,
                state.underflow_events
            );
            state.pushes_since_log = 0;
        }
    }
}

struct PlaybackState {
    samples: VecDeque<f32>,
    phase: f32,
    step: f32,
    last_sample: f32,
    underflowing: bool,
    underflow_events: u64,
    window_min_depth: usize,
    window_max_depth: usize,
    pushes_since_log: u32,
}

impl PlaybackState {
    fn next_sample(&mut self) -> f32 {
        if self.samples.len() < 2 {
            if !self.underflowing {
                self.underflowing = true;
                self.underflow_events = self.underflow_events.saturating_add(1);
                log::debug!("output stream underflow: queue depth={}", self.samples.len());
            }
            self.last_sample *= UNDERFLOW_FADE;
            if self.last_sample.abs() < 0.0002 {
                self.last_sample = 0.0;
            }
            return self.last_sample;
        }
        if self.underflowing {
            self.underflowing = false;
        }

        let idx = self.phase.floor() as usize;
        let frac = self.phase - idx as f32;

        let left = *self.samples.get(idx).unwrap_or(&0.0);
        let right = *self.samples.get(idx + 1).unwrap_or(&left);
        let out = left + (right - left) * frac;

        self.phase += self.step;
        let consume = self.phase.floor() as usize;
        if consume > 0 {
            let to_drop = consume.min(self.samples.len());
            self.samples.drain(..to_drop);
            self.phase -= to_drop as f32;
        }

        self.last_sample = out;
        out
    }
}

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

pub fn start_output_playback(selected_device_id: Option<&str>) -> Result<OutputPlayback, String> {
    let host = cpal::default_host();
    let device = resolve_output_device(&host, selected_device_id)?;
    let supported = device
        .default_output_config()
        .map_err(|err| format!("failed to query default output config: {err}"))?;

    let sample_rate = supported.sample_rate().0 as f32;
    let sample_format = supported.sample_format();
    let config: StreamConfig = supported.into();
    let channels = usize::from(config.channels);
    let step = 48_000.0 / sample_rate.max(1.0);

    let state = Arc::new(Mutex::new(PlaybackState {
        samples: VecDeque::with_capacity(OUTPUT_QUEUE_LIMIT),
        phase: 0.0,
        step,
        last_sample: 0.0,
        underflowing: false,
        underflow_events: 0,
        window_min_depth: 0,
        window_max_depth: 0,
        pushes_since_log: 0,
    }));
    let state_for_callback = Arc::clone(&state);
    let err_fn = move |err| {
        log::warn!("output stream error: {err}");
    };

    let stream = match sample_format {
        SampleFormat::I8 => {
            build_output_stream::<i8>(&device, &config, channels, state_for_callback, err_fn)?
        }
        SampleFormat::I16 => {
            build_output_stream::<i16>(&device, &config, channels, state_for_callback, err_fn)?
        }
        SampleFormat::I32 => {
            build_output_stream::<i32>(&device, &config, channels, state_for_callback, err_fn)?
        }
        SampleFormat::U8 => {
            build_output_stream::<u8>(&device, &config, channels, state_for_callback, err_fn)?
        }
        SampleFormat::U16 => {
            build_output_stream::<u16>(&device, &config, channels, state_for_callback, err_fn)?
        }
        SampleFormat::U32 => {
            build_output_stream::<u32>(&device, &config, channels, state_for_callback, err_fn)?
        }
        SampleFormat::F32 => {
            build_output_stream::<f32>(&device, &config, channels, state_for_callback, err_fn)?
        }
        SampleFormat::F64 => {
            build_output_stream::<f64>(&device, &config, channels, state_for_callback, err_fn)?
        }
        other => return Err(format!("unsupported output sample format: {other:?}")),
    };

    stream
        .play()
        .map_err(|err| format!("failed to start output stream: {err}"))?;

    Ok(OutputPlayback {
        _stream: stream,
        state,
    })
}

fn resolve_output_device(
    host: &cpal::Host,
    selected_device_id: Option<&str>,
) -> Result<cpal::Device, String> {
    if let Some(target_id) = selected_device_id {
        let devices = host
            .output_devices()
            .map_err(|err| format!("failed to enumerate output devices: {err}"))?;
        for device in devices {
            let Ok(name) = device.name() else {
                continue;
            };
            if name == target_id {
                return Ok(device);
            }
        }
    }

    host.default_output_device()
        .ok_or_else(|| "no output device available".to_string())
}

fn build_output_stream<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    channels: usize,
    state: Arc<Mutex<PlaybackState>>,
    err_fn: impl Fn(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream, String>
where
    T: Sample + cpal::SizedSample + FromSample<f32> + Send + 'static,
{
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _| {
                let Ok(mut playback) = state.lock() else {
                    for sample in data.iter_mut() {
                        *sample = T::EQUILIBRIUM;
                    }
                    return;
                };

                for frame in data.chunks_mut(channels.max(1)) {
                    let mono = playback.next_sample();
                    let converted = T::from_sample(mono);
                    for sample in frame {
                        *sample = converted;
                    }
                }
            },
            err_fn,
            None,
        )
        .map_err(|err| format!("failed to build output stream: {err}"))
}

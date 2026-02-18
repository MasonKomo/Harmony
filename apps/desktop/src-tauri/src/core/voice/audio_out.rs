use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, StreamConfig};
use crossbeam_queue::ArrayQueue;

use super::resampler::MonoResampler;
use super::AudioDevice;

const OUTPUT_QUEUE_SECONDS: f32 = 1.2;
const OUTPUT_QUEUE_MIN_CAPACITY: usize = 9_600;
const CLIP_THRESHOLD: f32 = 0.995;
const QUEUE_LOG_WINDOW_PUSHES: u32 = 120;
const PLAYOUT_PREFILL_MS: usize = 45;

#[derive(Debug, Clone, Copy, Default)]
pub struct OutputPlaybackStats {
    pub underflow_events: u64,
    pub overflow_dropped_samples: u64,
    pub callback_overruns: u64,
    pub callback_max_duration_us: u64,
    pub clipped_samples: u64,
    pub queued_samples: usize,
    pub peak_queued_samples: usize,
}

#[derive(Default)]
struct PlaybackStatsAtomic {
    underflow_events: AtomicU64,
    overflow_dropped_samples: AtomicU64,
    callback_overruns: AtomicU64,
    callback_max_duration_us: AtomicU64,
    clipped_samples: AtomicU64,
    peak_queued_samples: AtomicUsize,
}

impl PlaybackStatsAtomic {
    fn observe_peak_depth(&self, queued: usize) {
        let mut current = self.peak_queued_samples.load(Ordering::Relaxed);
        while queued > current {
            match self.peak_queued_samples.compare_exchange(
                current,
                queued,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return,
                Err(latest) => current = latest,
            }
        }
    }

    fn observe_callback_duration(&self, duration_us: u64) {
        let mut current = self.callback_max_duration_us.load(Ordering::Relaxed);
        while duration_us > current {
            match self.callback_max_duration_us.compare_exchange(
                current,
                duration_us,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return,
                Err(latest) => current = latest,
            }
        }
    }

    fn snapshot(&self, queued_samples: usize) -> OutputPlaybackStats {
        OutputPlaybackStats {
            underflow_events: self.underflow_events.load(Ordering::Relaxed),
            overflow_dropped_samples: self.overflow_dropped_samples.load(Ordering::Relaxed),
            callback_overruns: self.callback_overruns.load(Ordering::Relaxed),
            callback_max_duration_us: self.callback_max_duration_us.load(Ordering::Relaxed),
            clipped_samples: self.clipped_samples.load(Ordering::Relaxed),
            queued_samples,
            peak_queued_samples: self.peak_queued_samples.load(Ordering::Relaxed),
        }
    }
}

#[derive(Default)]
struct PushWindowState {
    pushes_since_log: u32,
    window_min_depth: usize,
    window_max_depth: usize,
}

pub struct OutputPlayback {
    _stream: cpal::Stream,
    device_name: String,
    sample_rate: u32,
    queue: Arc<ArrayQueue<f32>>,
    resampler: Mutex<MonoResampler>,
    stats: Arc<PlaybackStatsAtomic>,
    push_window: Mutex<PushWindowState>,
}

impl OutputPlayback {
    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn stats_snapshot(&self) -> OutputPlaybackStats {
        self.stats.snapshot(self.queue.len())
    }

    pub fn push_mono_48k(&self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }

        let mut converted = Vec::with_capacity(samples.len() + samples.len() / 4 + 8);
        if let Ok(mut resampler) = self.resampler.lock() {
            if let Err(err) = resampler.process(samples, &mut converted) {
                log::warn!("output resampler failed; dropping frame chunk: {err}");
                return;
            }
        } else {
            return;
        }

        if converted.is_empty() {
            return;
        }

        for sample in converted {
            let clipped = sample.clamp(-1.0, 1.0);
            if sample.abs() >= CLIP_THRESHOLD {
                self.stats.clipped_samples.fetch_add(1, Ordering::Relaxed);
            }

            if self.queue.push(clipped).is_err() {
                let _ = self.queue.pop();
                if self.queue.push(clipped).is_ok() {
                    self.stats
                        .overflow_dropped_samples
                        .fetch_add(1, Ordering::Relaxed);
                }
            }
        }

        let depth = self.queue.len();
        self.stats.observe_peak_depth(depth);

        if let Ok(mut window) = self.push_window.lock() {
            if window.pushes_since_log == 0 {
                window.window_min_depth = depth;
                window.window_max_depth = depth;
            } else {
                window.window_min_depth = window.window_min_depth.min(depth);
                window.window_max_depth = window.window_max_depth.max(depth);
            }
            window.pushes_since_log = window.pushes_since_log.saturating_add(1);
            if window.pushes_since_log >= QUEUE_LOG_WINDOW_PUSHES {
                let stats = self.stats.snapshot(depth);
                log::debug!(
                    "output queue depth window={}..{} samples underflows={} overflows={} callback_overruns={}",
                    window.window_min_depth,
                    window.window_max_depth,
                    stats.underflow_events,
                    stats.overflow_dropped_samples,
                    stats.callback_overruns
                );
                window.pushes_since_log = 0;
            }
        }
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
    let device_name = device
        .name()
        .unwrap_or_else(|_| "Unknown Output".to_string());
    let supported = device
        .default_output_config()
        .map_err(|err| format!("failed to query default output config: {err}"))?;

    let sample_rate = supported.sample_rate().0;
    let sample_format = supported.sample_format();
    let config: StreamConfig = supported.into();
    let channels = usize::from(config.channels.max(1));

    let queue_capacity = ((sample_rate as f32 * OUTPUT_QUEUE_SECONDS) as usize)
        .max(OUTPUT_QUEUE_MIN_CAPACITY)
        .max(channels * 256);
    let queue = Arc::new(ArrayQueue::<f32>::new(queue_capacity));
    let stats = Arc::new(PlaybackStatsAtomic::default());
    let queue_for_callback = Arc::clone(&queue);
    let stats_for_callback = Arc::clone(&stats);
    let err_fn = move |err| {
        log::warn!("output stream error: {err}");
    };

    let stream = match sample_format {
        SampleFormat::I8 => build_output_stream::<i8>(
            &device,
            &config,
            channels,
            sample_rate,
            queue_for_callback,
            stats_for_callback,
            err_fn,
        )?,
        SampleFormat::I16 => build_output_stream::<i16>(
            &device,
            &config,
            channels,
            sample_rate,
            queue_for_callback,
            stats_for_callback,
            err_fn,
        )?,
        SampleFormat::I32 => build_output_stream::<i32>(
            &device,
            &config,
            channels,
            sample_rate,
            queue_for_callback,
            stats_for_callback,
            err_fn,
        )?,
        SampleFormat::U8 => build_output_stream::<u8>(
            &device,
            &config,
            channels,
            sample_rate,
            queue_for_callback,
            stats_for_callback,
            err_fn,
        )?,
        SampleFormat::U16 => build_output_stream::<u16>(
            &device,
            &config,
            channels,
            sample_rate,
            queue_for_callback,
            stats_for_callback,
            err_fn,
        )?,
        SampleFormat::U32 => build_output_stream::<u32>(
            &device,
            &config,
            channels,
            sample_rate,
            queue_for_callback,
            stats_for_callback,
            err_fn,
        )?,
        SampleFormat::F32 => build_output_stream::<f32>(
            &device,
            &config,
            channels,
            sample_rate,
            queue_for_callback,
            stats_for_callback,
            err_fn,
        )?,
        SampleFormat::F64 => build_output_stream::<f64>(
            &device,
            &config,
            channels,
            sample_rate,
            queue_for_callback,
            stats_for_callback,
            err_fn,
        )?,
        other => return Err(format!("unsupported output sample format: {other:?}")),
    };

    stream
        .play()
        .map_err(|err| format!("failed to start output stream: {err}"))?;

    let resampler = MonoResampler::new(48_000, sample_rate)?;
    log::info!(
        "output stream started: device=\"{}\" sample_rate={} channels={} format={:?} queue_capacity={}",
        device_name,
        sample_rate,
        config.channels,
        sample_format,
        queue_capacity
    );

    Ok(OutputPlayback {
        _stream: stream,
        device_name,
        sample_rate,
        queue,
        resampler: Mutex::new(resampler),
        stats,
        push_window: Mutex::new(PushWindowState::default()),
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
    sample_rate: u32,
    queue: Arc<ArrayQueue<f32>>,
    stats: Arc<PlaybackStatsAtomic>,
    err_fn: impl Fn(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream, String>
where
    T: Sample + cpal::SizedSample + FromSample<f32> + Send + 'static,
{
    let channels = channels.max(1);
    let sample_rate = sample_rate.max(1);
    let frame_budget_us = 1_000_000_f64 / sample_rate as f64;
    let prefill_samples = ((sample_rate as usize) * PLAYOUT_PREFILL_MS / 1_000).max(channels * 8);
    let mut primed = false;
    let mut underflowing = false;

    device
        .build_output_stream(
            config,
            move |data: &mut [T], _| {
                let started = Instant::now();

                for frame in data.chunks_mut(channels) {
                    let mono = if !primed && queue.len() < prefill_samples {
                        0.0
                    } else if let Some(value) = queue.pop() {
                        if !primed {
                            primed = true;
                        }
                        if underflowing {
                            underflowing = false;
                        }
                        value
                    } else {
                        primed = false;
                        if !underflowing {
                            underflowing = true;
                            stats.underflow_events.fetch_add(1, Ordering::Relaxed);
                            log::debug!("output stream underflow: queue depth={}", queue.len());
                        }
                        0.0
                    };

                    let clipped = mono.clamp(-1.0, 1.0);
                    if mono.abs() >= CLIP_THRESHOLD {
                        stats.clipped_samples.fetch_add(1, Ordering::Relaxed);
                    }
                    let converted = T::from_sample(clipped);
                    for sample in frame {
                        *sample = converted;
                    }
                }

                let elapsed_us = started.elapsed().as_micros() as u64;
                stats.observe_callback_duration(elapsed_us);
                let frame_count = (data.len() / channels) as f64;
                let budget_us = (frame_count * frame_budget_us) as u64;
                if elapsed_us > budget_us {
                    stats.callback_overruns.fetch_add(1, Ordering::Relaxed);
                }
            },
            err_fn,
            None,
        )
        .map_err(|err| format!("failed to build output stream: {err}"))
}

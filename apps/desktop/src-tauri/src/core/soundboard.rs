use std::collections::HashMap;
use std::f32::consts::PI;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::default::{get_codecs, get_probe};

const APP_DIR: &str = "Harmony";
const SOUNDBOARD_DIR: &str = "soundboard";
const CUSTOM_CLIPS_DIR: &str = "clips";
const MANIFEST_FILE: &str = "manifest.json";
const OUTPUT_SAMPLE_RATE: u32 = 48_000;
const MAX_IMPORT_BYTES: usize = 6 * 1024 * 1024;
const MAX_CLIP_DURATION_MS: u32 = 8_000;
const MAX_CLIP_SAMPLES: usize = ((OUTPUT_SAMPLE_RATE as u64 * MAX_CLIP_DURATION_MS as u64) / 1000) as usize;
const MAX_LABEL_CHARS: usize = 36;

static CUSTOM_CLIP_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SoundboardClipSource {
    Default,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SoundboardClip {
    pub id: String,
    pub label: String,
    pub source: SoundboardClipSource,
    pub duration_ms: u32,
}

struct StoredClip {
    clip: SoundboardClip,
    samples_48k: Vec<f32>,
    file_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct SoundboardManifest {
    #[serde(default)]
    custom_clips: Vec<ManifestCustomClip>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ManifestCustomClip {
    id: String,
    label: String,
    file_name: String,
}

struct DefaultAsset {
    id: &'static str,
    label: &'static str,
    descriptor: &'static [u8],
}

#[derive(Clone, Copy)]
enum Waveform {
    Sine,
    Square,
    Triangle,
}

struct DefaultSpec {
    waveform: Waveform,
    freq_hz: f32,
    duration_ms: u32,
    gain: f32,
    attack_ms: u32,
    release_ms: u32,
}

impl Default for DefaultSpec {
    fn default() -> Self {
        Self {
            waveform: Waveform::Sine,
            freq_hz: 660.0,
            duration_ms: 220,
            gain: 0.72,
            attack_ms: 5,
            release_ms: 70,
        }
    }
}

pub struct SoundboardStore {
    custom_dir: PathBuf,
    manifest_path: PathBuf,
    clips: HashMap<String, StoredClip>,
}

impl SoundboardStore {
    pub fn load() -> Result<Self, String> {
        let root_dir = resolve_soundboard_root()?;
        let custom_dir = root_dir.join(CUSTOM_CLIPS_DIR);
        let manifest_path = root_dir.join(MANIFEST_FILE);
        fs::create_dir_all(&custom_dir)
            .map_err(|err| format!("failed to create soundboard directory: {err}"))?;

        let mut store = Self {
            custom_dir,
            manifest_path,
            clips: HashMap::new(),
        };

        store.load_default_clips()?;
        store.load_custom_clips()?;
        Ok(store)
    }

    pub fn list_clips(&self) -> Vec<SoundboardClip> {
        let mut clips = self
            .clips
            .values()
            .map(|entry| entry.clip.clone())
            .collect::<Vec<_>>();
        clips.sort_by(|left, right| {
            match (&left.source, &right.source) {
                (SoundboardClipSource::Default, SoundboardClipSource::Custom) => {
                    std::cmp::Ordering::Less
                }
                (SoundboardClipSource::Custom, SoundboardClipSource::Default) => {
                    std::cmp::Ordering::Greater
                }
                _ => left.label.to_lowercase().cmp(&right.label.to_lowercase()),
            }
        });
        clips
    }

    pub fn import_custom_clip(
        &mut self,
        label: &str,
        file_name: &str,
        bytes: &[u8],
    ) -> Result<SoundboardClip, String> {
        if bytes.is_empty() {
            return Err("sound file is empty".to_string());
        }
        if bytes.len() > MAX_IMPORT_BYTES {
            return Err(format!(
                "sound file is too large (max {} MB)",
                MAX_IMPORT_BYTES / (1024 * 1024)
            ));
        }

        let ext = normalize_extension(file_name)
            .ok_or_else(|| "unsupported file type. use .mp3, .wav, or .ogg".to_string())?;
        let decoded = decode_audio_to_48k_mono(bytes, Some(ext))?;
        if decoded.is_empty() {
            return Err("could not decode any audio frames".to_string());
        }
        ensure_clip_length(decoded.len())?;

        let clip_id = next_custom_clip_id();
        let normalized_label = normalize_label(label, file_name);
        let stored_file_name = format!("{clip_id}.{ext}");
        let stored_file_path = self.custom_dir.join(&stored_file_name);
        fs::write(&stored_file_path, bytes)
            .map_err(|err| format!("failed to store custom sound file: {err}"))?;

        let clip = SoundboardClip {
            id: clip_id.clone(),
            label: normalized_label,
            source: SoundboardClipSource::Custom,
            duration_ms: duration_ms_for_samples(decoded.len()),
        };

        self.clips.insert(
            clip_id,
            StoredClip {
                clip: clip.clone(),
                samples_48k: decoded,
                file_path: Some(stored_file_path),
            },
        );
        self.persist_manifest()?;
        Ok(clip)
    }

    pub fn delete_custom_clip(&mut self, clip_id: &str) -> Result<(), String> {
        let Some(existing) = self.clips.get(clip_id) else {
            return Err("clip not found".to_string());
        };
        if existing.clip.source != SoundboardClipSource::Custom {
            return Err("default clips cannot be deleted".to_string());
        }

        let removed = self.clips.remove(clip_id);
        if let Some(stored) = removed {
            if let Some(path) = stored.file_path {
                match fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(err) if err.kind() == ErrorKind::NotFound => {}
                    Err(err) => {
                        log::warn!("failed to remove custom clip file {}: {err}", path.display());
                    }
                }
            }
        }
        self.persist_manifest()?;
        Ok(())
    }

    pub fn samples_for_clip(&self, clip_id: &str) -> Option<Vec<f32>> {
        self.clips.get(clip_id).map(|entry| entry.samples_48k.clone())
    }

    fn load_default_clips(&mut self) -> Result<(), String> {
        for asset in default_assets() {
            let spec = parse_default_spec(asset.descriptor)?;
            let samples = synthesize_default_clip(spec);
            if samples.is_empty() {
                continue;
            }
            let clip = SoundboardClip {
                id: asset.id.to_string(),
                label: asset.label.to_string(),
                source: SoundboardClipSource::Default,
                duration_ms: duration_ms_for_samples(samples.len()),
            };
            self.clips.insert(
                clip.id.clone(),
                StoredClip {
                    clip,
                    samples_48k: samples,
                    file_path: None,
                },
            );
        }
        Ok(())
    }

    fn load_custom_clips(&mut self) -> Result<(), String> {
        let manifest = self.read_manifest()?;
        let mut loaded_entries = Vec::new();

        for item in manifest.custom_clips {
            let file_path = self.custom_dir.join(&item.file_name);
            let bytes = match fs::read(&file_path) {
                Ok(data) => data,
                Err(err) if err.kind() == ErrorKind::NotFound => {
                    log::warn!(
                        "soundboard clip file missing for {} ({}), skipping",
                        item.id,
                        file_path.display()
                    );
                    continue;
                }
                Err(err) => {
                    log::warn!(
                        "failed to read soundboard clip {} at {}: {err}",
                        item.id,
                        file_path.display()
                    );
                    continue;
                }
            };

            let ext = normalize_extension(&item.file_name);
            let decoded = match decode_audio_to_48k_mono(&bytes, ext) {
                Ok(samples) => samples,
                Err(err) => {
                    log::warn!("failed to decode custom clip {}: {err}", item.id);
                    continue;
                }
            };
            if decoded.is_empty() || ensure_clip_length(decoded.len()).is_err() {
                continue;
            }

            let clip = SoundboardClip {
                id: item.id.clone(),
                label: normalize_label(&item.label, &item.file_name),
                source: SoundboardClipSource::Custom,
                duration_ms: duration_ms_for_samples(decoded.len()),
            };
            self.clips.insert(
                clip.id.clone(),
                StoredClip {
                    clip,
                    samples_48k: decoded,
                    file_path: Some(file_path.clone()),
                },
            );
            loaded_entries.push(item);
        }

        self.write_manifest(&SoundboardManifest {
            custom_clips: loaded_entries,
        })?;
        Ok(())
    }

    fn read_manifest(&self) -> Result<SoundboardManifest, String> {
        if !self.manifest_path.exists() {
            return Ok(SoundboardManifest::default());
        }
        let raw = fs::read_to_string(&self.manifest_path)
            .map_err(|err| format!("failed to read soundboard manifest: {err}"))?;
        serde_json::from_str::<SoundboardManifest>(&raw)
            .map_err(|err| format!("failed to parse soundboard manifest: {err}"))
    }

    fn persist_manifest(&self) -> Result<(), String> {
        let mut custom_clips = self
            .clips
            .values()
            .filter(|entry| entry.clip.source == SoundboardClipSource::Custom)
            .filter_map(|entry| {
                let path = entry.file_path.as_ref()?;
                let file_name = path.file_name()?.to_str()?.to_string();
                Some(ManifestCustomClip {
                    id: entry.clip.id.clone(),
                    label: entry.clip.label.clone(),
                    file_name,
                })
            })
            .collect::<Vec<_>>();
        custom_clips.sort_by(|left, right| left.label.to_lowercase().cmp(&right.label.to_lowercase()));
        self.write_manifest(&SoundboardManifest { custom_clips })
    }

    fn write_manifest(&self, manifest: &SoundboardManifest) -> Result<(), String> {
        let content = serde_json::to_string_pretty(manifest)
            .map_err(|err| format!("failed to serialize soundboard manifest: {err}"))?;
        fs::write(&self.manifest_path, content)
            .map_err(|err| format!("failed to write soundboard manifest: {err}"))
    }
}

fn resolve_soundboard_root() -> Result<PathBuf, String> {
    let base = dirs::data_local_dir()
        .or_else(dirs::data_dir)
        .ok_or_else(|| "failed to resolve local data directory".to_string())?;
    Ok(base.join(APP_DIR).join(SOUNDBOARD_DIR))
}

fn next_custom_clip_id() -> String {
    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let counter = CUSTOM_CLIP_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("custom-{timestamp_ms}-{counter}")
}

fn normalize_label(label: &str, file_name: &str) -> String {
    let trimmed = label.trim();
    if !trimmed.is_empty() {
        return trimmed.chars().take(MAX_LABEL_CHARS).collect();
    }
    let stem = Path::new(file_name)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("Custom clip")
        .trim();
    if stem.is_empty() {
        "Custom clip".to_string()
    } else {
        stem.chars().take(MAX_LABEL_CHARS).collect()
    }
}

fn normalize_extension(file_name: &str) -> Option<&'static str> {
    let ext = Path::new(file_name).extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "mp3" => Some("mp3"),
        "wav" => Some("wav"),
        "ogg" => Some("ogg"),
        _ => None,
    }
}

fn duration_ms_for_samples(sample_count: usize) -> u32 {
    ((sample_count as u64 * 1000) / OUTPUT_SAMPLE_RATE as u64) as u32
}

fn ensure_clip_length(sample_count: usize) -> Result<(), String> {
    if sample_count > MAX_CLIP_SAMPLES {
        return Err(format!(
            "clip is too long (max {} seconds)",
            MAX_CLIP_DURATION_MS / 1000
        ));
    }
    Ok(())
}

fn decode_audio_to_48k_mono(bytes: &[u8], extension_hint: Option<&str>) -> Result<Vec<f32>, String> {
    let mut hint = Hint::new();
    if let Some(ext) = extension_hint {
        hint.with_extension(ext);
    }

    let source = std::io::Cursor::new(bytes.to_vec());
    let mss = MediaSourceStream::new(Box::new(source), Default::default());
    let probe = get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|err| format!("unsupported or invalid audio format: {err}"))?;

    let mut format = probe.format;
    let track = format
        .default_track()
        .ok_or_else(|| "audio file has no default track".to_string())?;
    if track.codec_params.codec == CODEC_TYPE_NULL {
        return Err("audio track codec is not supported".to_string());
    }

    let mut decoder = get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|err| format!("failed to initialize audio decoder: {err}"))?;
    let target_track = track.id;

    let mut mono_samples = Vec::new();
    let mut decoded_sample_rate = track.codec_params.sample_rate.unwrap_or(OUTPUT_SAMPLE_RATE);

    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(err)) if err.kind() == ErrorKind::UnexpectedEof => break,
            Err(err) => return Err(format!("audio demux failed: {err}")),
        };

        if packet.track_id() != target_track {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(audio) => audio,
            Err(SymphoniaError::IoError(err)) if err.kind() == ErrorKind::UnexpectedEof => break,
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(err) => return Err(format!("audio decode failed: {err}")),
        };

        let spec = *decoded.spec();
        let channels = spec.channels.count().max(1);
        decoded_sample_rate = spec.rate;

        let mut sample_buffer = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        sample_buffer.copy_interleaved_ref(decoded);
        for frame in sample_buffer.samples().chunks(channels) {
            let sum = frame.iter().copied().sum::<f32>();
            mono_samples.push(sum / channels as f32);
        }

        if mono_samples.len() > MAX_CLIP_SAMPLES * 3 {
            return Err("decoded clip is too long".to_string());
        }
    }

    if mono_samples.is_empty() {
        return Err("no decodable audio found".to_string());
    }

    let resampled = resample_linear(&mono_samples, decoded_sample_rate, OUTPUT_SAMPLE_RATE);
    let normalized = normalize_audio(&resampled);
    Ok(normalized)
}

fn resample_linear(input: &[f32], input_rate: u32, output_rate: u32) -> Vec<f32> {
    if input.is_empty() {
        return Vec::new();
    }
    let safe_input_rate = input_rate.max(1);
    let safe_output_rate = output_rate.max(1);
    if safe_input_rate == safe_output_rate {
        return input.to_vec();
    }

    let ratio = safe_input_rate as f64 / safe_output_rate as f64;
    let mut output = Vec::with_capacity(
        ((input.len() as u64 * safe_output_rate as u64) / safe_input_rate as u64)
            .max(1)
            .min((MAX_CLIP_SAMPLES * 2) as u64) as usize,
    );

    let mut source_pos = 0.0_f64;
    while source_pos + 1.0 < input.len() as f64 {
        let left_idx = source_pos.floor() as usize;
        let frac = (source_pos - left_idx as f64) as f32;
        let left = input[left_idx];
        let right = input[left_idx + 1];
        output.push(left + (right - left) * frac);
        source_pos += ratio;
    }

    if output.is_empty() {
        output.push(input[0]);
    }
    output
}

fn normalize_audio(input: &[f32]) -> Vec<f32> {
    if input.is_empty() {
        return Vec::new();
    }
    let peak = input.iter().fold(0.0_f32, |max, sample| max.max(sample.abs()));
    let gain = if peak > 0.92 { 0.92 / peak } else { 1.0 };
    input
        .iter()
        .map(|sample| (sample * gain).clamp(-1.0, 1.0))
        .collect()
}

fn default_assets() -> [DefaultAsset; 3] {
    [
        DefaultAsset {
            id: "default-chime",
            label: "Chime",
            descriptor: include_bytes!("soundboard_defaults/chime.sb"),
        },
        DefaultAsset {
            id: "default-pop",
            label: "Pop",
            descriptor: include_bytes!("soundboard_defaults/pop.sb"),
        },
        DefaultAsset {
            id: "default-rim",
            label: "Rim",
            descriptor: include_bytes!("soundboard_defaults/rim.sb"),
        },
    ]
}

fn parse_default_spec(raw_descriptor: &[u8]) -> Result<DefaultSpec, String> {
    let text = std::str::from_utf8(raw_descriptor)
        .map_err(|err| format!("default sound descriptor must be utf8: {err}"))?;
    let mut spec = DefaultSpec::default();

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((raw_key, raw_value)) = trimmed.split_once('=') else {
            continue;
        };
        let key = raw_key.trim();
        let value = raw_value.trim();
        match key {
            "wave" => {
                spec.waveform = match value {
                    "sine" => Waveform::Sine,
                    "square" => Waveform::Square,
                    "triangle" => Waveform::Triangle,
                    _ => return Err(format!("unsupported waveform '{value}' in default clip")),
                }
            }
            "freq_hz" => {
                if let Ok(parsed) = value.parse::<f32>() {
                    spec.freq_hz = parsed.clamp(40.0, 4_000.0);
                }
            }
            "duration_ms" => {
                if let Ok(parsed) = value.parse::<u32>() {
                    spec.duration_ms = parsed.clamp(60, 1_200);
                }
            }
            "gain" => {
                if let Ok(parsed) = value.parse::<f32>() {
                    spec.gain = parsed.clamp(0.05, 1.0);
                }
            }
            "attack_ms" => {
                if let Ok(parsed) = value.parse::<u32>() {
                    spec.attack_ms = parsed.clamp(0, 250);
                }
            }
            "release_ms" => {
                if let Ok(parsed) = value.parse::<u32>() {
                    spec.release_ms = parsed.clamp(0, 400);
                }
            }
            _ => {}
        }
    }
    Ok(spec)
}

fn synthesize_default_clip(spec: DefaultSpec) -> Vec<f32> {
    let sample_count = ((spec.duration_ms as u64 * OUTPUT_SAMPLE_RATE as u64) / 1000) as usize;
    if sample_count == 0 {
        return Vec::new();
    }
    let attack_samples = ((spec.attack_ms as u64 * OUTPUT_SAMPLE_RATE as u64) / 1000) as usize;
    let release_samples = ((spec.release_ms as u64 * OUTPUT_SAMPLE_RATE as u64) / 1000) as usize;

    let mut out = Vec::with_capacity(sample_count);
    for idx in 0..sample_count {
        let t = idx as f32 / OUTPUT_SAMPLE_RATE as f32;
        let phase = 2.0 * PI * spec.freq_hz * t;
        let wave = match spec.waveform {
            Waveform::Sine => phase.sin(),
            Waveform::Square => {
                if phase.sin() >= 0.0 {
                    1.0
                } else {
                    -1.0
                }
            }
            Waveform::Triangle => {
                let cycle = (spec.freq_hz * t).fract();
                4.0 * (cycle - 0.5).abs() - 1.0
            }
        };

        let attack_gain = if attack_samples == 0 {
            1.0
        } else {
            (idx as f32 / attack_samples as f32).clamp(0.0, 1.0)
        };
        let samples_from_end = sample_count.saturating_sub(idx + 1);
        let release_gain = if release_samples == 0 {
            1.0
        } else {
            (samples_from_end as f32 / release_samples as f32).clamp(0.0, 1.0)
        };
        let envelope = attack_gain.min(release_gain);
        out.push((wave * spec.gain * envelope).clamp(-1.0, 1.0));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_assets_generate_audio() {
        for asset in default_assets() {
            let spec = parse_default_spec(asset.descriptor).expect("descriptor parses");
            let samples = synthesize_default_clip(spec);
            assert!(!samples.is_empty(), "{}", asset.id);
            assert!(samples.iter().any(|sample| sample.abs() > 0.0001));
        }
    }

    #[test]
    fn resample_linear_downsamples() {
        let input = vec![0.0_f32; 48_000];
        let output = resample_linear(&input, 48_000, 24_000);
        assert!((23_995..=24_005).contains(&output.len()));
    }

    #[test]
    fn extension_normalization_restricts_supported_types() {
        assert_eq!(normalize_extension("clip.WAV"), Some("wav"));
        assert_eq!(normalize_extension("clip.mp3"), Some("mp3"));
        assert_eq!(normalize_extension("clip.ogg"), Some("ogg"));
        assert_eq!(normalize_extension("clip.flac"), None);
    }

    #[test]
    fn ensure_clip_length_enforces_duration_limit() {
        assert!(ensure_clip_length(MAX_CLIP_SAMPLES).is_ok());
        assert!(ensure_clip_length(MAX_CLIP_SAMPLES + 1).is_err());
    }
}

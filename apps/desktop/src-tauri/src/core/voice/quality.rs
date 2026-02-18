#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MixMonoResult {
    pub active_frames: usize,
    pub clip_samples: u64,
    pub nan_samples: u64,
}

pub fn soft_limiter(sample: f32) -> f32 {
    let abs = sample.abs();
    if abs <= 1.0 {
        sample
    } else {
        sample / (1.0 + abs * 0.5)
    }
}

pub fn should_conceal_gap(
    buffered_len: usize,
    gap_frames: u64,
    force_gap_conceal: bool,
    target_frames: usize,
    max_frames: usize,
    gap_plc_trigger_frames: u64,
) -> bool {
    buffered_len >= max_frames
        || (buffered_len >= target_frames && gap_frames >= gap_plc_trigger_frames)
        || (force_gap_conceal && gap_frames >= 1)
}

pub fn mix_mono_frames(
    frames: &[&[f32]],
    output: &mut [f32],
    headroom_gain: f32,
    limiter_drive: f32,
) -> MixMonoResult {
    output.fill(0.0);

    let mut active_frames = 0_usize;
    for frame in frames {
        if frame.is_empty() {
            continue;
        }
        active_frames = active_frames.saturating_add(1);
        for (idx, sample) in frame.iter().take(output.len()).enumerate() {
            output[idx] += *sample;
        }
    }

    if active_frames == 0 {
        return MixMonoResult::default();
    }

    let norm = (active_frames as f32).sqrt().max(1.0);
    let mut clip_samples = 0_u64;
    let mut nan_samples = 0_u64;
    for sample in output.iter_mut() {
        let pre = *sample * (headroom_gain / norm);
        if pre.abs() >= 1.0 {
            clip_samples = clip_samples.saturating_add(1);
        }
        let mut limited = soft_limiter(pre * limiter_drive);
        if !limited.is_finite() {
            nan_samples = nan_samples.saturating_add(1);
            limited = 0.0;
        }
        *sample = limited;
    }

    MixMonoResult {
        active_frames,
        clip_samples,
        nan_samples,
    }
}

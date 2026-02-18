#[path = "../src/core/voice/quality.rs"]
mod quality;
#[path = "../src/core/voice/resampler.rs"]
mod resampler;

fn approx_eq(left: f32, right: f32, epsilon: f32) -> bool {
    (left - right).abs() <= epsilon
}

#[test]
fn mixer_blends_two_speakers_with_headroom() {
    let frame_len = 960;
    let a = vec![0.4_f32; frame_len];
    let b = vec![-0.2_f32; frame_len];
    let mut out = vec![0.0_f32; frame_len];

    let mixed = quality::mix_mono_frames(&[a.as_slice(), b.as_slice()], &mut out, 0.90, 1.35);
    assert_eq!(mixed.active_frames, 2);
    assert_eq!(mixed.nan_samples, 0);
    assert_eq!(mixed.clip_samples, 0);
    assert!(approx_eq(out[0], 0.171, 0.01));
}

#[test]
fn limiter_prevents_runaway_mix_levels() {
    let frame_len = 960;
    let hot = vec![2.5_f32; frame_len];
    let mut out = vec![0.0_f32; frame_len];

    let mixed = quality::mix_mono_frames(&[hot.as_slice()], &mut out, 0.90, 1.35);
    assert_eq!(mixed.active_frames, 1);
    assert!(mixed.clip_samples > 0);
    assert!(out.iter().all(|sample| sample.is_finite()));
    assert!(out.iter().all(|sample| sample.abs() < 1.8));
}

#[test]
fn jitter_concealment_decision_matches_policy() {
    assert!(quality::should_conceal_gap(10, 1, false, 4, 10, 2));
    assert!(quality::should_conceal_gap(4, 2, false, 4, 10, 2));
    assert!(quality::should_conceal_gap(1, 1, true, 4, 10, 2));
    assert!(!quality::should_conceal_gap(3, 1, false, 4, 10, 2));
}

#[test]
fn resampler_generates_finite_audio_for_common_rates() {
    let mut upsampler = resampler::MonoResampler::new(44_100, 48_000).expect("upsampler");
    let input = (0..44_100)
        .map(|idx| ((idx as f32 / 35.0).sin()) * 0.6)
        .collect::<Vec<_>>();
    let mut upsampled = Vec::new();
    upsampler
        .process(&input, &mut upsampled)
        .expect("upsample succeeds");
    assert!(upsampled.len() > 1_000);
    assert!(upsampled.iter().all(|sample| sample.is_finite()));

    let mut downsampler = resampler::MonoResampler::new(48_000, 44_100).expect("downsampler");
    let mut downsampled = Vec::new();
    downsampler
        .process(&upsampled, &mut downsampled)
        .expect("downsample succeeds");
    assert!(downsampled.len() > 1_000);
    assert!(downsampled.iter().all(|sample| sample.is_finite()));
}

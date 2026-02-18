use audioadapter_buffers::direct::SequentialSliceOfVecs;
use rubato::{Fft, FixedSync, Resampler};

const RESAMPLER_CHUNK_FRAMES: usize = 960;

pub struct MonoResampler {
    passthrough: bool,
    engine: Option<Fft<f32>>,
    input_pending: Vec<f32>,
    output_pending: Vec<f32>,
}

impl MonoResampler {
    pub fn new(input_rate: u32, output_rate: u32) -> Result<Self, String> {
        let safe_input = input_rate.max(1);
        let safe_output = output_rate.max(1);
        let passthrough = safe_input == safe_output;
        let engine = if passthrough {
            None
        } else {
            Some(
                Fft::<f32>::new(
                    safe_input as usize,
                    safe_output as usize,
                    RESAMPLER_CHUNK_FRAMES,
                    2,
                    1,
                    FixedSync::Input,
                )
                .map_err(|err| format!("failed to create resampler: {err}"))?,
            )
        };

        Ok(Self {
            passthrough,
            engine,
            input_pending: Vec::with_capacity(RESAMPLER_CHUNK_FRAMES * 3),
            output_pending: Vec::with_capacity(RESAMPLER_CHUNK_FRAMES * 3),
        })
    }

    pub fn process(&mut self, input: &[f32], output: &mut Vec<f32>) -> Result<(), String> {
        if input.is_empty() {
            self.drain_output(output);
            return Ok(());
        }

        if self.passthrough {
            output.extend_from_slice(input);
            return Ok(());
        }

        let Some(engine) = self.engine.as_mut() else {
            output.extend_from_slice(input);
            return Ok(());
        };

        self.input_pending.extend_from_slice(input);
        let input_frames = engine.input_frames_next();
        while self.input_pending.len() >= input_frames {
            let chunk = self
                .input_pending
                .drain(..input_frames)
                .collect::<Vec<f32>>();
            let channel_data = vec![chunk];
            let input = SequentialSliceOfVecs::new(&channel_data, 1, input_frames)
                .map_err(|err| format!("failed to wrap resampler input: {err}"))?;
            let processed = engine
                .process(&input, 0, None)
                .map_err(|err| format!("resampler process failed: {err}"))?;
            self.output_pending.extend(processed.take_data());
        }

        self.drain_output(output);
        Ok(())
    }

    pub fn drain_output(&mut self, output: &mut Vec<f32>) {
        if self.output_pending.is_empty() {
            return;
        }
        output.extend(self.output_pending.drain(..));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_when_rates_match() {
        let mut resampler = MonoResampler::new(48_000, 48_000).expect("creates passthrough");
        let input = vec![0.1_f32, -0.2, 0.3, -0.4];
        let mut output = Vec::new();
        resampler
            .process(&input, &mut output)
            .expect("passthrough process succeeds");
        assert_eq!(input, output);
    }

    #[test]
    fn resamples_44100_to_48000_without_nans() {
        let mut resampler = MonoResampler::new(44_100, 48_000).expect("creates resampler");
        let input = (0..4_410)
            .map(|idx| ((idx as f32 / 40.0).sin()) * 0.7)
            .collect::<Vec<_>>();
        let mut output = Vec::new();
        resampler
            .process(&input, &mut output)
            .expect("resampler process succeeds");

        assert!(output.iter().all(|value| value.is_finite()));
        assert!(!output.is_empty());
    }
}

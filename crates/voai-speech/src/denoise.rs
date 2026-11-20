//! Session-local FastEnhancer-B state. All inference runs on the capture worker,
//! after resampling, outside the Core Audio callback.
use std::sync::LazyLock;

use ort::{
    environment::Environment,
    session::{Session, SessionInputValue, builder::AutoDevicePolicy},
    value::{DynValue, Tensor, TensorRef},
};

const MODEL: &[u8] = include_bytes!("../assets/fastenhancer/fastenhancer_b.onnx");
const HOP: usize = 256;

// ONNX Runtime permits one environment per process. Each recording still owns
// a separate session and fresh overlap/recurrent state.
static ENVIRONMENT: LazyLock<ort::Result<Environment>> =
    LazyLock::new(|| ort::init().with_telemetry(false).build());

pub(super) struct NoiseSuppression(Option<FastEnhancer>);

impl NoiseSuppression {
    pub fn new(enabled: bool) -> Self {
        Self(enabled.then(FastEnhancer::new).and_then(|result| {
            result
                .inspect_err(|_| eprintln!("FastEnhancer unavailable; using microphone audio"))
                .ok()
        }))
    }

    /// Both decoders consume the same stream. Keep raw PCM available for history
    /// and fall back to it for the whole utterance if inference ever fails.
    pub fn audio<'a>(&'a mut self, raw: &'a [f32], finish: bool) -> &'a [f32] {
        if let Some(filter) = &mut self.0
            && filter.advance(raw, finish).is_err()
        {
            eprintln!("FastEnhancer stopped; using microphone audio");
            self.0 = None;
        }
        self.0.as_ref().map_or(raw, |filter| &filter.output)
    }
}

struct FastEnhancer {
    session: Session,
    state: Vec<DynValue>,
    consumed: usize,
    primed: bool,
    output: Vec<f32>,
}

impl FastEnhancer {
    fn new() -> ort::Result<Self> {
        let env = ENVIRONMENT
            .as_ref()
            .map_err(|err| ort::Error::new(err.to_string()))?;
        let session = Session::builder(env)?
            .with_auto_device(AutoDevicePolicy::PreferCPU)?
            .with_intra_threads(1)?
            .with_inter_threads(1)?
            .commit_from_memory(MODEL)?;
        // Model input order: waveform, input/output overlap, three recurrent states.
        let overlap = || Tensor::from_array(([1, HOP], vec![0f32; HOP])).map(Tensor::into_dyn);
        let recurrent =
            || Tensor::from_array(([1, 24, 36], vec![0f32; 24 * 36])).map(Tensor::into_dyn);
        let state = vec![
            overlap()?,
            overlap()?,
            recurrent()?,
            recurrent()?,
            recurrent()?,
        ];
        Ok(Self {
            session,
            state,
            consumed: 0,
            primed: false,
            output: Vec::new(),
        })
    }

    fn advance(&mut self, raw: &[f32], finish: bool) -> ort::Result<()> {
        let (frames, tail) = raw[self.consumed..].as_chunks::<HOP>();
        for frame in frames {
            self.frame(frame)?;
            self.consumed += HOP;
        }
        if finish && !raw.is_empty() {
            if !tail.is_empty() {
                let mut padded = [0.; HOP];
                padded[..tail.len()].copy_from_slice(tail);
                self.frame(&padded)?;
            }
            // The first output is startup padding. One zero frame releases the last real hop.
            self.frame(&[0.; HOP])?;
            self.output.truncate(raw.len());
        }
        Ok(())
    }

    fn frame(&mut self, samples: &[f32]) -> ort::Result<()> {
        let audio = TensorRef::from_array_view(([1, HOP], samples))?;
        let mut inputs = Vec::with_capacity(1 + self.state.len());
        inputs.push(SessionInputValue::from(audio));
        inputs.extend(self.state.iter().map(SessionInputValue::from));
        let outputs = self.session.run(inputs.as_slice())?;
        let (_, wave) = outputs["wav_out"].try_extract_tensor::<f32>()?;
        if self.primed {
            self.output.extend_from_slice(wave);
        }
        self.primed = true;
        self.state.clear();
        self.state
            .extend(outputs.into_iter().skip(1).map(|(_, value)| value));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn irregular_capture_blocks_match_reference_through_release() {
        let signal: Vec<_> = (0..4099).map(|i| ((i % 137) as f32 - 68.) / 128.).collect();
        // Golden samples from the upstream streaming ONNX frames with reset state.
        for (length, last) in [(4096, 0.00410483), (4099, 0.00524390)] {
            let mut filter = FastEnhancer::new().unwrap();
            for end in (0..length).step_by(173) {
                filter.advance(&signal[..end], false).unwrap();
            }
            filter.advance(&signal[..length], true).unwrap();
            assert_eq!(filter.output.len(), length);
            for (index, expected) in [
                (100, 0.049_025_9),
                (1100, -0.02819447),
                (3800, 0.00405125),
                (length - 1, last),
            ] {
                assert!(
                    (filter.output[index] - expected).abs() < 0.0001,
                    "sample {index} was {}",
                    filter.output[index]
                );
            }
        }
        let mut disabled = NoiseSuppression::new(false);
        assert!(std::ptr::eq(
            disabled.audio(&signal, true),
            signal.as_slice()
        ));
    }
}

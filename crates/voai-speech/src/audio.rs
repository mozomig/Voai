use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};
use voai_l10n::{tf, tr};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, FromSample, Sample, SizedSample, Stream, StreamConfig};
use rtrb::{Consumer, Producer, RingBuffer};
use rubato::{FftFixedInOut, Resampler};

use crate::{RATE, RECORDING};

pub(super) struct Microphone {
    stream: Option<Stream>,
    samples: Consumer<f32>,
    issue: Arc<AtomicU8>,
    converter: Converter,
}

impl Microphone {
    pub(super) fn open(name: Option<&str>, state: Arc<AtomicU8>) -> Result<Self, String> {
        let host = cpal::default_host();
        let device = match name {
            Some(name) => crate::devices::selected_device(&host, name)?,
            None => host.default_input_device().ok_or(tr("mic.unavailable"))?,
        };
        let supported = device
            .default_input_config()
            .map_err(|e| tf!("mic.open", e = e))?;
        let config = supported.config();
        let rate = config.sample_rate as usize;
        if !(8_000..=384_000).contains(&rate) || config.channels == 0 {
            return Err(tr("mic.config").into());
        }
        let converter = Converter::new(rate)?;
        let (producer, samples) = RingBuffer::new(rate * 2);
        let issue = Arc::new(AtomicU8::new(0));
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => {
                build_stream::<f32>(&device, &config, producer, issue.clone(), state)
            }
            cpal::SampleFormat::F64 => {
                build_stream::<f64>(&device, &config, producer, issue.clone(), state)
            }
            cpal::SampleFormat::I16 => {
                build_stream::<i16>(&device, &config, producer, issue.clone(), state)
            }
            cpal::SampleFormat::I32 => {
                build_stream::<i32>(&device, &config, producer, issue.clone(), state)
            }
            format => return Err(tf!("mic.format", format = format)),
        }?;
        stream.play().map_err(|e| tf!("mic.start", e = e))?;
        Ok(Self {
            stream: Some(stream),
            samples,
            issue,
            converter,
        })
    }

    #[cfg(any(test, feature = "test-fixtures"))]
    pub(super) fn fixture(
        pcm: Vec<f32>,
        rate: usize,
        state: Arc<AtomicU8>,
        eof: std::sync::mpsc::Sender<()>,
    ) -> Result<Self, String> {
        if !(8_000..=384_000).contains(&rate) {
            return Err("Unsupported PCM fixture rate".into());
        }
        let converter = Converter::new(rate)?;
        let (mut producer, samples) = RingBuffer::new(rate * 2);
        let issue = Arc::new(AtomicU8::new(0));
        let stream_issue = issue.clone();
        std::thread::Builder::new()
            .name("voai-fixture-input".into())
            .spawn(move || {
                for block in pcm.chunks(rate / 50) {
                    if state.load(Ordering::Acquire) != RECORDING {
                        return;
                    }
                    for &sample in block {
                        if producer.push(sample).is_err() {
                            stream_issue.store(1, Ordering::Release);
                            return;
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                let _ = eof.send(());
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            stream: None,
            samples,
            issue,
            converter,
        })
    }

    pub(super) fn stop(&mut self) {
        self.stream.take();
    }

    pub(super) fn interruption(&self) -> Option<&'static str> {
        match self.issue.load(Ordering::Acquire) {
            1 => Some(tr("mic.overflow")),
            2 => Some(tr("mic.lost")),
            _ => None,
        }
    }

    pub(super) fn drain(&mut self, output: &mut Vec<f32>) -> Result<bool, String> {
        let available = self.samples.slots();
        if available != 0 {
            let chunk = self
                .samples
                .read_chunk(available)
                .map_err(|e| e.to_string())?;
            let (left, right) = chunk.as_slices();
            self.converter.push(left, output)?;
            self.converter.push(right, output)?;
            chunk.commit_all();
        }
        Ok(available != 0)
    }

    pub(super) fn finish(mut self, output: &mut Vec<f32>) -> Result<(), String> {
        self.stop();
        self.drain(output)?;
        self.converter.finish(output)
    }
}

fn build_stream<T: SizedSample + Sample>(
    device: &Device,
    config: &StreamConfig,
    mut samples: Producer<f32>,
    issue: Arc<AtomicU8>,
    state: Arc<AtomicU8>,
) -> Result<Stream, String>
where
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels);
    let stream_issue = issue.clone();
    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                // The callback neither allocates nor waits on a lock. finish() closes this gate
                // synchronously, even when the capture thread is waiting for model startup.
                if state.load(Ordering::Acquire) != RECORDING || issue.load(Ordering::Acquire) != 0
                {
                    return;
                }
                for frame in data.chunks_exact(channels) {
                    let mono =
                        frame.iter().map(|v| v.to_sample::<f32>()).sum::<f32>() / channels as f32;
                    if samples.push(mono).is_err() {
                        issue.store(1, Ordering::Release);
                        break;
                    }
                }
            },
            move |_| stream_issue.store(2, Ordering::Release),
            None,
        )
        .map_err(|e| tf!("mic.stream", e = e))
}

/// Band-limited resampling with the filter delay removed and its tail flushed at release.
struct Converter {
    filter: FftFixedInOut<f32>,
    pending: Vec<f32>,
    input_rate: usize,
    input_len: usize,
    skip: usize,
}

impl Converter {
    fn new(input_rate: usize) -> Result<Self, String> {
        let filter = FftFixedInOut::new(input_rate, RATE, 512, 1).map_err(|e| e.to_string())?;
        let skip = filter.output_delay();
        let pending = Vec::with_capacity(filter.input_frames_next());
        Ok(Self {
            filter,
            pending,
            input_rate,
            input_len: 0,
            skip,
        })
    }

    fn push(&mut self, mut input: &[f32], output: &mut Vec<f32>) -> Result<(), String> {
        self.input_len += input.len();
        while !input.is_empty() {
            let needed = self.filter.input_frames_next() - self.pending.len();
            let (part, rest) = input.split_at(needed.min(input.len()));
            self.pending.extend_from_slice(part);
            input = rest;
            if self.pending.len() == self.filter.input_frames_next() {
                self.flush_block(output)?;
            }
        }
        Ok(())
    }

    fn flush_block(&mut self, output: &mut Vec<f32>) -> Result<(), String> {
        let converted = self
            .filter
            .process(&[self.pending.as_slice()], None)
            .map_err(|e| e.to_string())?;
        let skip = self.skip.min(converted[0].len());
        self.skip -= skip;
        output.extend_from_slice(&converted[0][skip..]);
        self.pending.clear();
        Ok(())
    }

    fn finish(&mut self, output: &mut Vec<f32>) -> Result<(), String> {
        let expected = self.input_len * RATE / self.input_rate;
        while output.len() < expected {
            self.pending.resize(self.filter.input_frames_next(), 0.);
            self.flush_block(output)?;
        }
        output.truncate(expected);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn release_preserves_last_block_at_fractional_rate() {
        let mut converter = Converter::new(44_100).unwrap();
        let mut output = vec![];
        let signal: Vec<_> = (0..44_101).map(|i| (i as f32 * 0.05).sin()).collect();
        for part in signal.chunks(473) {
            converter.push(part, &mut output).unwrap();
        }
        converter.finish(&mut output).unwrap();
        assert_eq!(output.len(), 16_000);
        assert!(output[15_900..].iter().any(|v| v.abs() > 0.6));
    }
}

use super::{SAMPLE_RATE, Turn};
use sherpa_onnx_sys as sys;
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

pub(super) struct Diarizer(*const sys::OfflineSpeakerDiarization);

impl Drop for Diarizer {
    fn drop(&mut self) {
        // The owned engine is destroyed exactly once, after all inference calls.
        unsafe { sys::SherpaOnnxDestroyOfflineSpeakerDiarization(self.0) }
    }
}

impl Diarizer {
    pub(super) fn load(dir: &Path) -> Result<Self, ()> {
        let path = |name| {
            let path = dir.join(name);
            if !path.is_file() {
                return Err(());
            }
            CString::new(path.as_os_str().as_bytes()).map_err(|_| ())
        };
        let segmentation = path("segmentation.onnx")?;
        let embedding = path("embedding.onnx")?;
        let config = sys::OfflineSpeakerDiarizationConfig {
            segmentation: sys::OfflineSpeakerSegmentationModelConfig {
                pyannote: sys::OfflineSpeakerSegmentationPyannoteModelConfig {
                    model: segmentation.as_ptr(),
                    window_shift_ratio: 0.2,
                },
                num_threads: 1,
                debug: 0,
                provider: c"cpu".as_ptr(),
            },
            embedding: sys::SpeakerEmbeddingExtractorConfig {
                model: embedding.as_ptr(),
                num_threads: 1,
                debug: 0,
                provider: c"cpu".as_ptr(),
            },
            clustering: sys::FastClusteringConfig {
                num_clusters: -1,
                threshold: 0.75,
                compute_confidence: 0,
            },
            min_duration_on: 0.3,
            min_duration_off: 0.5,
        };
        // The C API copies configuration strings during synchronous construction.
        let engine = unsafe { sys::SherpaOnnxCreateOfflineSpeakerDiarization(&config) };
        if engine.is_null() {
            return Err(());
        }
        let engine = Self(engine);
        if unsafe { sys::SherpaOnnxOfflineSpeakerDiarizationGetSampleRate(engine.0) } != SAMPLE_RATE
        {
            return Err(());
        }
        Ok(engine)
    }

    pub(super) fn turns(&self, samples: &[f32]) -> Result<Vec<Turn>, ()> {
        let n = i32::try_from(samples.len()).map_err(|_| ())?;
        let duration_ms = samples.len() as u64 / 16;
        // The input is one bounded window, alive for the whole synchronous call.
        let result =
            unsafe { sys::SherpaOnnxOfflineSpeakerDiarizationProcess(self.0, samples.as_ptr(), n) };
        if result.is_null() {
            return Err(());
        }
        let turns = unsafe {
            let count = sys::SherpaOnnxOfflineSpeakerDiarizationResultGetNumSegments(result);
            match count.cmp(&0) {
                std::cmp::Ordering::Less => Err(()),
                std::cmp::Ordering::Equal => Ok(Vec::new()),
                std::cmp::Ordering::Greater => {
                    let ptr = sys::SherpaOnnxOfflineSpeakerDiarizationResultSortByStartTime(result);
                    if ptr.is_null() {
                        Err(())
                    } else {
                        let items = std::slice::from_raw_parts(ptr, count as usize);
                        let turns = items
                            .iter()
                            .filter_map(|item| {
                                let start_ms = ms(item.start)?;
                                let end_ms = ms(item.end)?.min(duration_ms);
                                (end_ms > start_ms).then_some(Turn {
                                    start_ms,
                                    end_ms,
                                    speaker: item.speaker.try_into().ok()?,
                                })
                            })
                            .collect();
                        sys::SherpaOnnxOfflineSpeakerDiarizationDestroySegment(ptr);
                        Ok(turns)
                    }
                }
            }
        };
        unsafe {
            sys::SherpaOnnxOfflineSpeakerDiarizationDestroyResult(result);
        }
        turns
    }
}

fn ms(seconds: f32) -> Option<u64> {
    seconds
        .is_finite()
        .then(|| (seconds.max(0.0) as f64 * 1000.0).round() as u64)
}

pub(super) struct Embedder(*const sys::SpeakerEmbeddingExtractor, usize);

impl Drop for Embedder {
    fn drop(&mut self) {
        unsafe { sys::SherpaOnnxDestroySpeakerEmbeddingExtractor(self.0) };
    }
}

impl Embedder {
    pub(super) fn load(dir: &Path) -> Result<Self, ()> {
        let path =
            CString::new(dir.join("embedding.onnx").as_os_str().as_bytes()).map_err(|_| ())?;
        let cfg = sys::SpeakerEmbeddingExtractorConfig {
            model: path.as_ptr(),
            num_threads: 1,
            debug: 0,
            provider: c"cpu".as_ptr(),
        };
        let ptr = unsafe { sys::SherpaOnnxCreateSpeakerEmbeddingExtractor(&cfg) };
        if ptr.is_null() {
            return Err(());
        }
        let dim = unsafe { sys::SherpaOnnxSpeakerEmbeddingExtractorDim(ptr) };
        let engine = Self(ptr, dim as usize);
        if !(1..=4096).contains(&dim) {
            return Err(());
        }
        Ok(engine)
    }

    pub(super) fn extract(&self, samples: &[f32]) -> Option<Vec<f32>> {
        if samples.len() < SAMPLE_RATE as usize / 2 {
            return None;
        }
        // Stream/embedding buffers belong to sherpa. Copy only the fixed-dimensional
        // vector into the recording's voice registry before releasing both.
        unsafe {
            let stream = sys::SherpaOnnxSpeakerEmbeddingExtractorCreateStream(self.0);
            if stream.is_null() {
                return None;
            }
            sys::SherpaOnnxOnlineStreamAcceptWaveform(
                stream,
                SAMPLE_RATE,
                samples.as_ptr(),
                samples.len() as i32,
            );
            sys::SherpaOnnxOnlineStreamInputFinished(stream);
            let result = if sys::SherpaOnnxSpeakerEmbeddingExtractorIsReady(self.0, stream) != 0 {
                let ptr = sys::SherpaOnnxSpeakerEmbeddingExtractorComputeEmbedding(self.0, stream);
                if ptr.is_null() {
                    None
                } else {
                    let vector = std::slice::from_raw_parts(ptr, self.1).to_vec();
                    sys::SherpaOnnxSpeakerEmbeddingExtractorDestroyEmbedding(ptr);
                    Some(vector)
                }
            } else {
                None
            };
            sys::SherpaOnnxDestroyOnlineStream(stream);
            result
        }
    }
}

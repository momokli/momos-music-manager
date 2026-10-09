//! Discogs-EffNet audio embeddings + genre head.
//!
//! Pipeline validated in `examples/effnet_spike/` (issue #189):
//!   ffmpeg → 16 kHz mono f32 → frames(512/256, first frame zero-centered)
//!   → Hann → power FFT → **96-band** Slaney mel → `log10(1 + 10000·E)`
//!   → patches(128 frames × 96 bands, hop 62) → ONNX Runtime → mean over patches
//!   → 1280-d embedding (+ 400 Discogs genres, sigmoid).
//!
//! Gated behind the `effnet` cargo feature so the default/CI build stays light.
//! Requires `ffmpeg` on PATH. EffNet gives embeddings + genre only — **not**
//! BPM/key (those come from the analyzer service, see `analyzer.rs`).

/// Default model file name (expected next to the configured model path dir).
pub const MODEL_FILE: &str = "discogs-effnet-bsdynamic-1.onnx";
pub const LABELS_FILE: &str = "genre_discogs400-discogs-effnet-1.json";

#[derive(Debug, Clone, Default)]
pub struct Embedding {
    /// 1280-d track embedding (mean over patches).
    pub vec: Vec<f32>,
    /// Top genres `(label, probability)`, descending. Empty if no labels file.
    pub genres: Vec<(String, f32)>,
}

/// Was the crate compiled with the `effnet` feature?
pub const fn available() -> bool {
    cfg!(feature = "effnet")
}

#[cfg(feature = "effnet")]
pub use imp::embed_file;

#[cfg(not(feature = "effnet"))]
pub use imp::embed_file;

#[cfg(feature = "effnet")]
mod imp {
    use std::f32::consts::PI;
    use std::path::Path;

    use anyhow::{Context, Result, bail};
    use ort::session::Session;
    use ort::value::Tensor;
    use rustfft::FftPlanner;
    use rustfft::num_complex::Complex32;

    use super::Embedding;

    /// `ort::Error` is not `Send + Sync`, so it can't convert into `anyhow`.
    fn oe<T, E: std::fmt::Display>(r: Result<T, E>) -> Result<T> {
        r.map_err(|e| anyhow::anyhow!("onnxruntime: {e}"))
    }

    const FRAME_SIZE: usize = 512;
    const HOP_SIZE: usize = 256;
    const N_BANDS: usize = 96;
    const SAMPLE_RATE: f32 = 16000.0;
    const FMAX: f32 = 8000.0;
    const PATCH_SIZE: usize = 128;
    const PATCH_HOP: usize = 62;
    const NUM_BINS: usize = FRAME_SIZE / 2 + 1; // 257
    const SR_PER_BIN: f32 = (SAMPLE_RATE / 2.0) / ((FRAME_SIZE / 2) as f32);
    const MIN_LOG_HZ: f32 = 1000.0;
    const LIN_SLOPE: f32 = 3.0 / 200.0;

    fn hz2mel_slaney(hz: f32) -> f32 {
        let min_log_mel = MIN_LOG_HZ * LIN_SLOPE;
        if hz < MIN_LOG_HZ {
            hz * LIN_SLOPE
        } else {
            let log_step = (6.4f32).ln() / 27.0;
            min_log_mel + (hz / MIN_LOG_HZ).ln() / log_step
        }
    }

    fn mel2hz_slaney(mel: f32) -> f32 {
        let min_log_mel = MIN_LOG_HZ * LIN_SLOPE;
        if mel < min_log_mel {
            mel / LIN_SLOPE
        } else {
            let log_step = (6.4f32).ln() / 27.0;
            MIN_LOG_HZ * ((mel - min_log_mel) * log_step).exp()
        }
    }

    /// TriangularBands with weighting=linear, normalize=unit_tri.
    fn mel_filterbank() -> Vec<[f32; NUM_BINS]> {
        let low_mel = hz2mel_slaney(0.0);
        let high_mel = hz2mel_slaney(FMAX);
        let mel_inc = (high_mel - low_mel) / (N_BANDS as f32 + 1.0);
        let freqs: Vec<f32> = (0..N_BANDS + 2)
            .map(|i| mel2hz_slaney(low_mel + i as f32 * mel_inc))
            .collect();
        let mut fb = vec![[0.0f32; NUM_BINS]; N_BANDS];
        for i in 0..N_BANDS {
            let fstep1 = freqs[i + 1] - freqs[i];
            let fstep2 = freqs[i + 2] - freqs[i + 1];
            let jbegin = (freqs[i] / SR_PER_BIN).ceil() as usize;
            let jend = (freqs[i + 2] / SR_PER_BIN).floor() as usize;
            for j in jbegin..=jend.min(NUM_BINS - 1) {
                let binfreq = j as f32 * SR_PER_BIN;
                let c = if binfreq < freqs[i + 1] {
                    (binfreq - freqs[i]) / fstep1
                } else {
                    (freqs[i + 2] - binfreq) / fstep2
                };
                fb[i][j] = c;
            }
            let weight = (fstep1 + fstep2) / 2.0;
            for j in jbegin..=jend.min(NUM_BINS - 1) {
                fb[i][j] /= weight;
            }
        }
        fb
    }

    fn hann_window() -> Vec<f32> {
        (0..FRAME_SIZE)
            .map(|i| 0.5 - 0.5 * ((2.0 * PI * i as f32) / (FRAME_SIZE as f32 - 1.0)).cos())
            .collect()
    }

    /// FrameCutter default (startFromZero=false): first frame zero-centered at 0.
    fn frame_signal(signal: &[f32]) -> Vec<Vec<f32>> {
        let n = signal.len();
        let off = (FRAME_SIZE + 1) / 2; // 256
        let mut frames = Vec::new();
        let mut k: i64 = 0;
        loop {
            let start = -(off as i64) + k * HOP_SIZE as i64;
            if start >= n as i64 {
                break;
            }
            let mut frame = vec![0.0f32; FRAME_SIZE];
            let s = start.max(0) as usize;
            let e = (start + FRAME_SIZE as i64).min(n as i64).max(0) as usize;
            if e > s {
                let dst = (s as i64 - start) as usize;
                frame[dst..dst + (e - s)].copy_from_slice(&signal[s..e]);
            }
            frames.push(frame);
            if start + (FRAME_SIZE / 2) as i64 >= n as i64 {
                break;
            }
            k += 1;
        }
        frames
    }

    /// Decode any ffmpeg-readable file to 16 kHz mono f32 samples.
    fn decode_16k_mono(path: &Path) -> Result<Vec<f32>> {
        let out = std::process::Command::new("ffmpeg")
            .arg("-v")
            .arg("error")
            .arg("-i")
            .arg(path)
            .args(["-f", "f32le", "-ac", "1", "-ar", "16000", "-"])
            .output()
            .context("run ffmpeg (is it on PATH?)")?;
        if !out.status.success() {
            bail!(
                "ffmpeg failed for {}: {}",
                path.display(),
                String::from_utf8_lossy(&out.stderr)
            );
        }
        let b = out.stdout;
        Ok(b.chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect())
    }

    /// Compute the EffNet embedding (+ top genres) for one audio file.
    /// Synchronous and CPU-bound — call inside `spawn_blocking`.
    pub fn embed_file(model: &Path, labels: Option<&Path>, audio: &Path) -> Result<Embedding> {
        let signal = decode_16k_mono(audio)?;
        if signal.is_empty() {
            bail!("no audio decoded from {}", audio.display());
        }

        let window = hann_window();
        let fb = mel_filterbank();
        let frames = frame_signal(&signal);
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FRAME_SIZE);
        let mut buf = vec![Complex32::new(0.0, 0.0); FRAME_SIZE];

        let n_frames = frames.len();
        let mut mel = vec![vec![0.0f32; N_BANDS]; n_frames];
        for (fi, frame) in frames.iter().enumerate() {
            for i in 0..FRAME_SIZE {
                buf[i] = Complex32::new(frame[i] * window[i], 0.0);
            }
            fft.process(&mut buf);
            let mut power = [0.0f32; NUM_BINS];
            for (j, c) in buf[..NUM_BINS].iter().enumerate() {
                power[j] = c.re * c.re + c.im * c.im;
            }
            for b in 0..N_BANDS {
                let mut e = 0.0f32;
                let f = &fb[b];
                for j in 0..NUM_BINS {
                    e += power[j] * f[j];
                }
                mel[fi][b] = (1.0 + 10000.0 * e).log10();
            }
        }

        let n_patches = if n_frames < PATCH_SIZE {
            0
        } else {
            1 + (n_frames - PATCH_SIZE) / PATCH_HOP
        };
        if n_patches == 0 {
            bail!("audio too short for one patch ({} frames)", n_frames);
        }

        const BATCH: usize = 64;
        let builder = oe(Session::builder())?;
        let mut builder = oe(builder.with_memory_pattern(false))?;
        let mut session = oe(builder.commit_from_file(model))?;

        let mut emb_sum: Vec<f32> = Vec::new();
        let mut prob_sum: Vec<f32> = Vec::new();
        let (mut emb_dim, mut n_classes) = (0usize, 0usize);
        let n_batches = n_patches.div_ceil(BATCH);
        for bi in 0..n_batches {
            let cstart = bi * BATCH;
            let bs = (n_patches - cstart).min(BATCH);
            let mut data = vec![0.0f32; bs * PATCH_SIZE * N_BANDS];
            for p in 0..bs {
                let s = (cstart + p) * PATCH_HOP;
                for f in 0..PATCH_SIZE {
                    for b in 0..N_BANDS {
                        data[(p * PATCH_SIZE + f) * N_BANDS + b] = mel[s + f][b];
                    }
                }
            }
            let tensor = oe(Tensor::from_array(([bs, PATCH_SIZE, N_BANDS], data)))?;
            let outputs = oe(session.run(ort::inputs!["melspectrogram" => tensor]))?;
            let (emb_shape, emb) = oe(outputs["embeddings"].try_extract_tensor::<f32>())?;
            let (act_shape, act) = oe(outputs["activations"].try_extract_tensor::<f32>())?;
            if bi == 0 {
                emb_dim = emb_shape[1] as usize;
                n_classes = act_shape[1] as usize;
                emb_sum = vec![0.0f32; emb_dim];
                prob_sum = vec![0.0f32; n_classes];
            }
            for p in 0..bs {
                for d in 0..emb_dim {
                    emb_sum[d] += emb[p * emb_dim + d];
                }
                for d in 0..n_classes {
                    prob_sum[d] += act[p * n_classes + d];
                }
            }
        }
        let np = n_patches as f32;
        for v in emb_sum.iter_mut() {
            *v /= np;
        }
        for v in prob_sum.iter_mut() {
            *v /= np;
        }

        let genres = labels
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|j| j["classes"].as_array().cloned())
            .map(|classes| {
                let mut idx: Vec<usize> = (0..prob_sum.len()).collect();
                idx.sort_by(|a, b| prob_sum[*b].partial_cmp(&prob_sum[*a]).unwrap());
                idx.into_iter()
                    .take(5)
                    .filter_map(|i| {
                        classes
                            .get(i)
                            .and_then(|c| c.as_str())
                            .map(|s| (s.to_string(), prob_sum[i]))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        Ok(Embedding {
            vec: emb_sum,
            genres,
        })
    }
}

#[cfg(not(feature = "effnet"))]
mod imp {
    use std::path::Path;

    use anyhow::{Result, bail};

    use super::Embedding;

    pub fn embed_file(_model: &Path, _labels: Option<&Path>, _audio: &Path) -> Result<Embedding> {
        bail!("mmm-hub was built without the `effnet` feature (rebuild with --features effnet)")
    }
}

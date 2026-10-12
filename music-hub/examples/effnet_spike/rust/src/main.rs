//! Discogs-EffNet feasibility spike (Rust).
//!
//! Pipeline (validated against Essentia in Python, see ../python/):
//!   WAV(16k mono) -> frames(512/256, startFromZero=false)
//!     -> Hann(window) -> |FFT|^2 (power) -> 96-band Slaney mel (unit_tri, linear)
//!     -> log10(1 + 10000*mel) -> patches(128 frames x 96 bands, hop 62)
//!     -> ort inference -> embeddings[1280] / activations[400]

use std::f32::consts::PI;
use std::time::Instant;

use ort::session::Session;
use ort::value::Tensor;
use rustfft::num_complex::Complex32;
use rustfft::FftPlanner;

const FRAME_SIZE: usize = 512;
const HOP_SIZE: usize = 256;
const N_BANDS: usize = 96;
const SAMPLE_RATE: f32 = 16000.0;
const FMAX: f32 = 8000.0;
const PATCH_SIZE: usize = 128;
const PATCH_HOP: usize = 62;

const SR_PER_BIN: f32 = (SAMPLE_RATE / 2.0) / ((FRAME_SIZE / 2) as f32);

// --- Slaney mel scale (essentiamath.h) ---
const MIN_LOG_HZ: f32 = 1000.0;
const LIN_SLOPE: f32 = 3.0 / 200.0;
const NUM_BINS: usize = FRAME_SIZE / 2 + 1; // 257

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

/// TriangularBands::createFilters with weighting=linear, normalize=unit_tri.
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
            // left zero-padding for the first (zero-centered) frame(s)
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let model_path = std::env::args().nth(1).unwrap_or_else(|| "../models/discogs-effnet-bsdynamic-1.onnx".to_string());
    let wav_path = std::env::args().nth(2).unwrap_or_else(|| "../audio/test_clip_16k_mono.wav".to_string());

    // --- decode WAV (16-bit PCM mono 16k) ---
    let t0 = Instant::now();
    let mut reader = hound::WavReader::open(&wav_path)?;
    let spec = reader.spec();
    let signal: Vec<f32> = reader
        .samples::<i16>()
        .map(|s| s.unwrap() as f32 / 32768.0)
        .collect();
    let decode_ms = t0.elapsed().as_secs_f64() * 1e3;
    println!(
        "wav: {}  {} Hz, {} ch, {:?}, {} samples  (decode {:.1} ms)",
        wav_path, spec.sample_rate, spec.channels, spec.sample_format, signal.len(), decode_ms
    );

    // --- frames + mel ---
    let t0 = Instant::now();
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
        // power spectrum |X|^2 (Spectrum magnitude squared by MelBands type=power)
        let power: Vec<f32> = buf[..NUM_BINS].iter().map(|c| c.re * c.re + c.im * c.im).collect();
        for b in 0..N_BANDS {
            let mut e = 0.0f32;
            for j in 0..NUM_BINS {
                e += power[j] * fb[b][j];
            }
            mel[fi][b] = (1.0 + 10000.0 * e).log10();
        }
    }
    let mel_ms = t0.elapsed().as_secs_f64() * 1e3;
    println!("mel: {} frames x {} bands  ({:.1} ms, {:.0} frames/s)",
             n_frames, N_BANDS, mel_ms, n_frames as f64 / (mel_ms / 1e3));

    // dump mel (raw f32 LE, [n_frames, 96]) for cross-validation in Python
    {
        let mut bytes = Vec::with_capacity(n_frames * N_BANDS * 4);
        for row in &mel {
            for &v in row {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
        }
        std::fs::write("../audio/mel_rust.f32", &bytes)?;
    }

    // --- patches [N,128,96], lastPatchMode=discard ---
    let n_patches = if n_frames < PATCH_SIZE { 0 } else { 1 + (n_frames - PATCH_SIZE) / PATCH_HOP };
    println!("patches: [{}, {}, {}]", n_patches, PATCH_SIZE, N_BANDS);

    // --- ort inference (batched, like Essentia batchSize=64, to bound memory) ---
    const BATCH: usize = 64;
    let t0 = Instant::now();
    // memory_pattern(false) avoids pre-allocating a pattern-sized arena; threads
    // are bounded so per-thread scratch memory stays predictable.
    let mut session = Session::builder()?
        .with_memory_pattern(false)?
        .commit_from_file(&model_path)?;
    let init_ms = t0.elapsed().as_secs_f64() * 1e3;

    // warmup (single patch)
    {
        let mut warm = vec![0.0f32; PATCH_SIZE * N_BANDS];
        for f in 0..PATCH_SIZE {
            for b in 0..N_BANDS {
                warm[f * N_BANDS + b] = mel[f][b];
            }
        }
        let warm = Tensor::from_array(([1usize, PATCH_SIZE, N_BANDS], warm))?;
        let _ = session.run(ort::inputs!["melspectrogram" => warm]);
    }

    let mut emb_sum: Vec<f32> = Vec::new();
    let mut prob_sum: Vec<f32> = Vec::new();
    let (mut emb_dim, mut n_classes) = (0usize, 0usize);
    let mut inf_ms = 0.0f64;
    let n_batches = (n_patches + BATCH - 1) / BATCH;
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
        let t = Instant::now();
        let tensor = Tensor::from_array(([bs, PATCH_SIZE, N_BANDS], data))?;
        let outputs = session.run(ort::inputs!["melspectrogram" => tensor])?;
        inf_ms += t.elapsed().as_secs_f64() * 1e3;

        let (emb_shape, emb) = outputs["embeddings"].try_extract_tensor::<f32>()?;
        let (act_shape, act) = outputs["activations"].try_extract_tensor::<f32>()?;
        if bi == 0 {
            emb_dim = emb_shape[1] as usize;
            n_classes = act_shape[1] as usize;
            emb_sum = vec![0.0f32; emb_dim];
            prob_sum = vec![0.0f32; n_classes];
            println!(
                "onnx: model init {:.0} ms | embeddings {:?} activations {:?}",
                init_ms, emb_shape, act_shape
            );
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
    println!(
        "onnx: inference {:.1} ms for {} patches in {} batches (batch<= {}) => {:.0} patches/s",
        inf_ms, n_patches, n_batches, BATCH, n_patches as f64 / (inf_ms / 1e3)
    );

    // mean over patches -> track embedding
    for v in emb_sum.iter_mut() {
        *v /= n_patches as f32;
    }
    let norm: f32 = emb_sum.iter().map(|x| x * x).sum::<f32>().sqrt();
    println!("embedding: dim={}  mean-norm={:.4}", emb_dim, norm);
    {
        let mut bytes = Vec::with_capacity(emb_sum.len() * 4);
        for &v in &emb_sum {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        std::fs::write("../audio/embedding_rust.f32", &bytes)?;
    }

    // top genres
    if let Ok(text) = std::fs::read_to_string("../models/genre_discogs400-discogs-effnet-1.json") {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(classes) = json["classes"].as_array() {
                let mut probs = prob_sum.clone();
                for v in probs.iter_mut() {
                    *v /= n_patches as f32;
                }
                let mut idx: Vec<usize> = (0..probs.len()).collect();
                idx.sort_by(|a, b| probs[*b].partial_cmp(&probs[*a]).unwrap());
                println!("top-5 styles:");
                for &i in idx.iter().take(5) {
                    println!("  {:.3}  {}", probs[i], classes[i].as_str().unwrap_or("?"));
                }
            }
        }
    }

    // peak RSS
    unsafe {
        let mut ru: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut ru);
        let rss_mb = ru.ru_maxrss as f64 / (1024.0 * 1024.0); // bytes on macOS
        println!("peak RSS: {:.1} MB", rss_mb);
    }

    let total = decode_ms + mel_ms + inf_ms;
    let dur = signal.len() as f64 / SAMPLE_RATE as f64;
    println!("TOTAL (decode+mel+infer, excl. model init): {:.0} ms for {:.1}s audio => {:.1}x realtime",
             total, dur, dur / (total / 1e3));
    Ok(())
}

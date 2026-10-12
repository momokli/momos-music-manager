# EffNet-Discogs ONNX spike — audio embeddings in Rust (feasibility)

**Status: ✅ fully working (Python reference + Rust prototype), numerically validated.**
Issue #189. Everything here is self-contained under `mmm-hub/examples/effnet_spike/`.

We proved we can compute a **Discogs-EffNet** 1280-dim audio embedding (the model
cosine.club uses) **and** the 400-class Discogs genre head, in **pure Rust** with
ONNX Runtime (`ort`), on CPU — no Python container.

---

## TL;DR

|                         | Python (Essentia mel + onnxruntime) | **Rust (`ort` rc.13, self-made mel)** |
| ----------------------- | ----------------------------------- | ------------------------------------- |
| 30 s clip, total        | 65 ms (**≈460× realtime**)          | 61 ms (**≈490× realtime**)            |
| 180 s clip, total       | 335 ms (≈537× realtime)             | 338 ms (≈533× realtime)               |
| Peak RSS (30 s / 180 s) | 321 MB / 871 MB                     | **174 MB / 371 MB**                   |
| Mel vs Essentia         | matches (max abs 5.5e-5)            | matches (max abs 2.4e-5)              |
| Embedding vs reference  | cosine 1.000000                     | cosine 1.000000                       |

**Recommendation: A (ONNX/Rust).** The Rust mel + `ort` path reproduces Essentia to
float precision, uses less memory, ships as a single static binary, and needs no
Python. Remaining work is decode/resample + wiring, not research.

> ⚠️ **Correction to the issue/plan:** Discogs-EffNet produces \*\*embeddings (1280-d)
>
> - genre logits (400)** only. It does **NOT** produce **BPM or key\*\*. Those must
>   come from a separate algorithm/model (Essentia `RhythmExtractor`/`KeyExtractor`,
>   madmom, Traktor, …). See [§6](#6-what-effnet-does-not-give-you-bpmkey).

---

## 1. What's here

```
examples/effnet_spike/
├── README.md                     # this file
├── results.txt                   # raw captured run outputs
├── models/                       # downloaded from HuggingFace (see §3)
│   ├── discogs-effnet-bsdynamic-1.onnx        (17.2 MB)
│   └── genre_discogs400-discogs-effnet-1.json (labels, 400 classes)
├── python/
│   ├── inspect_model.py          # prints ONNX input/output names + shapes
│   ├── synth_audio.py            # makes a 30 s pseudo-electronic test clip
│   ├── make_16k_wav.py           # Essentia MonoLoader → 16 kHz mono PCM16 (for Rust)
│   ├── effnet_embed.py           # reference: Essentia mel + ort → embedding + genres
│   ├── mel_numpy.py              # PURE-NUMPY mel front-end (the portable recipe)
│   ├── effnet_embed_numpy.py     # numpy mel + ort end-to-end
│   ├── validate_mel.py           # numpy mel vs Essentia (PASS)
│   └── compare_rust.py           # Rust dumps vs Python (PASS)
└── rust/                         # standalone crate (NOT in any workspace)
    ├── Cargo.toml
    └── src/main.rs               # frames + Hann + FFT + mel + ort inference
```

Generated at runtime (not needed for review): `audio/`, `.venv/`, `rust/target/`.

---

## 2. The exact preprocessing recipe (validated)

Derived from Essentia's `TensorflowPredictEffnetDiscogs` →
`TensorflowInputMusiCNN` → `Windowing`/`Spectrum`/`MelBands`/`TriangularBands`.

| Stage           | Value                                                                                                                                          |
| --------------- | ---------------------------------------------------------------------------------------------------------------------------------------------- | --- | --------------- |
| Input           | mono, **16 000 Hz**, float                                                                                                                     |
| Framing         | `frameSize=512`, `hopSize=256`, **`startFromZero=false`** (first frame _zero-centered_ at sample 0, `startIndex=-256`), last frame zero-padded |
| Window          | **Hann**, `w[n] = 0.5 − 0.5·cos(2πn/(N−1))`, **not normalized**                                                                                |
| Spectrum        | FFT size 512 → **257 linear-magnitude** bins                                                                                                   |
| Mel             | **96** bands, Slaney mel scale, `lowFrequencyBound=0`, `highFrequencyBound=8000`, weighting=`linear`, normalize=`unit_tri`                     |
| Mel type        | `type=power` → band energy = Σⱼ `                                                                                                              | Xⱼ  | ²·filter[i][j]` |
| Compression     | `log10(1 + 10000 · energy)` (i.e. Essentia `UnaryOperator(scale=10000, shift=1)` then `log10`)                                                 |
| Patches         | **128 frames × 96 bands**; `patchHopSize=62`, `lastPatchMode=discard` (Essentia defaults)                                                      |
| Model input     | `melspectrogram` : `float32[batch, 128, 96]` = `[batch, frames, melBands]`                                                                     |
| Model output    | `embeddings` `float32[batch, 1280]`, `activations` `float32[batch, 400]` (sigmoid)                                                             |
| Track embedding | **mean over patches** → 1280-d                                                                                                                 |

**Important:** the issue text said "128 mel bands". It is actually **96 mel bands**
and **128 frames per patch**. The `[batch,128,96]` shape is `[batch, patchFrames,
melBands]` (Essentia builds `{batchSize, 1, patchSize, numberBands}` and squeezes
the channel for this ONNX export).

### Two gotchas we hit and fixed

1. **`startFromZero=false` is the default.** The first frame is _zero-centered_
   (`[-256, 256)`), not `[0, 512)`. Getting this wrong shifts the whole time axis
   (our first Rust version did; fixed — see `git`/`results.txt`).
2. **Power vs magnitude.** `Spectrum` returns linear _magnitude_; `MelBands`
   `type=power` squares it, so band energy is Σ`|X|²·coef`. You can therefore
   skip `sqrt` entirely (just use `re²+im²`).

---

## 3. Models (download)

```bash
cd examples/effnet_spike/models
curl -L -O https://huggingface.co/Heyian/discogs-effnet-onnx/resolve/main/discogs-effnet-bsdynamic-1.onnx
curl -L -O https://huggingface.co/Heyian/discogs-effnet-onnx/resolve/main/genre_discogs400-discogs-effnet-1.json
# sha256 (verify): a280825b334797cf677939db8cd5762c0392aedd0ca6415dbc1cd083f045e43c
```

Host needed: `huggingface.co` (reachable). ONNX Runtime itself is fetched by `ort`
at build time from `ort.pyke.io` (reachable).

---

## 4. Reproduce

### Python (reference)

```bash
cd examples/effnet_spike
/opt/homebrew/bin/python3.12 -m venv .venv
.venv/bin/pip install onnxruntime numpy soundfile essentia
.venv/bin/python python/inspect_model.py          # → input [batch,128,96], outputs [*,1280]/[*,400]
.venv/bin/python python/synth_audio.py            # 30 s test clip
.venv/bin/python python/effnet_embed.py           # Essentia-mel embedding + top-10 genres
.venv/bin/python python/validate_mel.py           # numpy mel vs Essentia → PASS
.venv/bin/python python/effnet_embed_numpy.py     # numpy-mel embedding (cosine 1.0 vs Essentia)
```

### Rust (prototype)

```bash
cd examples/effnet_spike
.venv/bin/python python/make_16k_wav.py           # 16 kHz mono PCM16 input for Rust
cd rust
cargo run --release -- ../models/discogs-effnet-bsdynamic-1.onnx ../audio/test_clip_16k_mono.wav
# validates dumps against Python:
cd .. && .venv/bin/python python/compare_rust.py  # → PASS (cosine 1.000000)
```

The Rust crate is **standalone** (`[workspace]` in its `Cargo.toml`); it is _not_ a
member of `mmm-hub` or the root crate. Deps: `ort = "2.0.0-rc.13"`, `rustfft`,
`hound`, `serde_json`, `libc`.

---

## 5. Measured numbers

Machine: **Apple M4 Pro, 12 cores, macOS** (this dev box — **not** the `.200`
server; see caveat §7). Audio: synthesized 30 s / 180 s clip at 124 BPM.

| Path                                                | Clip  | decode | mel    | ONNX   | total  | realtime | peak RSS   |
| --------------------------------------------------- | ----- | ------ | ------ | ------ | ------ | -------- | ---------- |
| Python (Essentia mel + ort)                         | 30 s  | 17 ms¹ | 10 ms  | 39 ms  | 65 ms  | 464×     | 321 MB     |
| Python (Essentia mel + ort)                         | 180 s | 57 ms¹ | 57 ms  | 221 ms | 335 ms | 537×     | 871 MB     |
| Python (pure-numpy mel + ort)                       | 30 s  | 17 ms¹ | 20 ms  | 39 ms  | 76 ms  | 396×     | 327 MB     |
| **Rust** (`ort`, batch 64, `memory_pattern(false)`) | 30 s  | 2 ms   | 18 ms  | 40 ms  | 61 ms  | 493×     | **174 MB** |
| **Rust** (same)                                     | 180 s | 13 ms  | 107 ms | 219 ms | 338 ms | 533×     | **371 MB** |

¹ includes Essentia `MonoLoader` decode + 44.1 kHz→16 kHz resample.

Model init (Rust, `commit_from_file`): **24 ms**. ONNX model 17.2 MB.
Rust release binary **27 MB** (ONNX Runtime is **statically** linked; the build
downloads an 80 MB `libonnxruntime.a` and the final binary has no runtime `.dylib`).

**Memory tuning findings**

- Batching all patches at once (180 patches) → 734 MB peak. Chunking to
  `batch=64` (Essentia's default `batchSize`) → 559 MB.
- `SessionBuilder::with_memory_pattern(false)` → **371 MB**, with **no** throughput
  loss (in fact slightly faster). This is the recommended setting.
- Limiting `with_intra_threads(4)` lowered memory further (≈371 MB) but cost ~28 %
  throughput; not worth it by default.

Throughput ≈ **1.9–2.0 ms per second of audio** (≈1.2 ms per 128-frame/62-hop patch).
Extrapolated: a 4-min track ≈ **0.45 s** single-stream on this box.

### Numerical validation (the important part)

```
numpy frames           == Essentia frames        (max abs diff 0.0)
numpy mel   vs Essentia mel   max abs 5.5e-05   (relative 8.9e-06)
numpy-pipe embedding vs Essentia-pipe embedding  cosine 1.000000
Rust  mel   vs numpy mel      max abs 2.4e-05   (relative 3.9e-06)
Rust  embedding vs Python    cosine 1.000000, max abs diff 7.7e-07
```

i.e. our independent NumPy/Rust implementations agree with Essentia (and with each
other) at float32 precision.

---

## 6. What EffNet does _not_ give you (BPM/key)

`discogs-effnet` yields **embeddings + genre** only. There is **no BPM, no key, no
Camelot** output. The `music-data-sources.md` §9 note that EffNet "gives BPM, Key +
Camelot" is **incorrect** — the pipeline needs a _separate_ feature stage for those:

- BPM/beat: Essentia `RhythmExtractor2013` / `PercivalBpmEstimator` (AGPL), or madmom.
- Key/Camelot: Essentia `KeyExtractor` (edma/profile), or a dedicated key model
  (cf. `rekordcloud/OpenKeyScan`), or **Traktor headless** on `.200` (already the
  plan's primary local source, epic #53).
- Genre/mood: EffNet (+ its classification-head siblings) **does** cover this, free.

So EffNet solves **audio similarity + genre + mood**, not the BPM/key gap.

---

## 7. Caveats / honesty

- **Latency/RAM are from this Mac (M4 Pro), not the `.200` server.** The issue asks
  for `.200` numbers; from this machine `http://192.168.178.200:8710` is **not
  reachable** (connect timeout), and there is **no `MUSIC_API_TOKEN` in `mmm-hub/.env`**
  (only `SPOTIFY_*`, `COSINECLUB_API`, `FREQBLOG_API`), so we could not pull a real
  FLAC from `music-api`. Test audio is therefore **synthesized** (`synth_audio.py`);
  numbers are directionally representative but must be re-measured on `.200`.
- **Resampling is not yet in Rust.** The Rust prototype reads a pre-made **16 kHz
  mono** WAV (produced via Essentia/ffmpeg). Production needs `symphonia` (decode) +
  `rubato` (resample). This is the main remaining port task and is low-risk.
- The genre sanity check is on a synthetic clip (it lands in Electronic\* styles —
  plausible, not a quality benchmark).
- `patchHopSize=62` (Essentia default) means overlapping patches; for a single
  mean-pooled track embedding the hop has little effect, but we kept the default for
  bit-for-bit comparability.

---

## 8. License note (read before shipping)

- **Essentia** (the wrapper used as ground truth in Python): **AGPL-3.0**, or a
  commercial license from MTG-UPF. Fine for a private self-hosted hub; a public
  offering triggers AGPL obligations — or buy the MTG commercial license.
- **Discogs-EffNet model weights** (the `Heyian/discogs-effnet-onnx` mirror is
  tagged **CC-BY-NC-SA-4.0**): **non-commercial**. Acceptable for the private hub;
  for commercial use, verify terms with MTG-UPF (the original Essentia models carry
  their own license).
- **Our code here** (NumPy + Rust mel, `ort` wiring) is an _independent
  re-implementation_ of the documented algorithm — no Essentia source is copied or
  linked. If you want to be maximally conservative about the AGPL/reimplementation
  question, either run Essentia itself (AGPL) or obtain the MTG commercial license.
- ONNX Runtime (used via `ort`) is MIT-licensed.

---

## 9. Recommendation & remaining production work

**Choose A (ONNX in Rust).** Validated here end-to-end. Remaining tasks for the hub:

1. **Decode + resample**: `symphonia` (FLAC/WAV/MP3/AAC) + `rubato` → 16 kHz mono,
   replacing the current WAV-only `hound` reader.
2. **Model delivery**: download `discogs-effnet-bsdynamic-1.onnx` on first use
   (pin the sha256 above), cache under the config dir.
3. **Batch policy**: fixed `batch=64`, `with_memory_pattern(false)`; one session
   reused across tracks (init is only 24 ms, but avoid per-track init).
4. **Storage**: 1280 × f32 = 5 KB/track BLOB in SQLite; brute-force cosine is fine
   for ~100k, else `sqlite-vec`.
5. **Genre**: already free from the `activations` output — map to Discogs400 labels.
6. **BPM/key**: add a _separate_ stage (Traktor on `.200`, or Essentia/madmom) —
   EffNet does not cover it.
7. **License review** (§8) before any non-private deployment.

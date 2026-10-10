# mmm-analyzer

A small, headless **BPM/key analysis service** for the music hub. The hub
(`mmm-hub`) calls it to fill in tempo and musical key for tracks whose FLAC has
been fetched to the server's local disk. It accepts a **server-local file
path** (no upload) and returns BPM, key and Camelot wheel notation. On request
it can additionally return a **Discogs-EffNet audio embedding** (1280-dim) and
the top **Discogs-400 music styles**.

- Binds to **127.0.0.1:8711** (localhost only).
- Runs under **systemd** as unit **`mmm-analyzer`**.
- No GUI/GPU required.

## Engine

**Essentia** (Python bindings, TensorFlow-enabled wheel), the preferred engine
from the brief:

| Output    | Algorithm                                                                                             |
| --------- | ----------------------------------------------------------------------------------------------------- |
| BPM       | `essentia.standard.RhythmExtractor2013` (method `multifeature`)                                       |
| Key       | `essentia.standard.KeyExtractor` (key + major/minor scale)                                            |
| Camelot   | derived in `server.py` from key + scale (sharp-spelled pitch classes)                                 |
| Decoding  | `essentia.standard.MonoLoader` (FLAC/WAV/MP3/OGG/… via libav/ffmpeg)                                  |
| Embedding | `essentia.standard.TensorflowPredictEffnetDiscogs` (Discogs-EffNet, mean-pooled, 1280-dim)            |
| Genres    | `essentia.standard.TensorflowPredict2D` on top of the embedding (`genre_discogs400-discogs-effnet-1`) |

`GET /health` reports the exact engine string, e.g.
`essentia 2.1-beta6-dev (RhythmExtractor2013/multifeature + KeyExtractor)`, plus
an `embeddings` block describing whether the embedding/genre models are present.

### Licensing note (important)

Essentia is licensed under **AGPL-3.0**. Running it as a network service can
trigger the AGPL's network-copyleft obligations if the service is exposed to
third parties. This service is bound to **127.0.0.1** and is used only as a
local batch tool inside the hub, but keep the licence in mind before exposing
it more widely.

The **Discogs-EffNet model weights** and the **Genre Discogs400 classifier**
are published by the MTG (Universitat Pompeu Fabra) under **CC BY-NC-SA 4.0**
(non-commercial). Verify the current terms on the model pages before any use
that is not strictly non-commercial:

- <https://essentia.upf.edu/models.html>
- <https://essentia.upf.edu/models/feature-extractors/discogs-effnet/discogs-effnet-bs64-1.json>

## API contract

### `GET /health`

```json
{
  "ok": true,
  "engine": "essentia 2.1-beta6-dev (RhythmExtractor2013/multifeature + KeyExtractor)",
  "embeddings": {
    "enabled": true,
    "available": true,
    "model": "discogs-effnet-bs64-1.pb",
    "dim": 1280,
    "genres_available": true
  }
}
```

`embeddings.available` is a cheap file-presence check (the model is loaded
lazily on the first embedding request). `available: false` still means BPM/key
analysis works normally.

### `POST /analyze`

Request (BPM/key only — unchanged, backward compatible):

```json
{ "path": "/absolute/path/to/file.flac" }
```

Request (with embeddings):

```json
{ "path": "/absolute/path/to/file.flac", "embed": true }
```

Success — `200` (with `embed: true`):

```json
{
  "bpm": 128.0,
  "key": "A-Minor",
  "camelot": "8A",
  "confidence": { "bpm": 2.28, "key": 0.83 },
  "embedding": [0.0123, -0.0456, 0.0789, "… (exactly 1280 floats)"],
  "genres": [
    { "label": "Rock---Coldwave", "score": 0.4442 },
    { "label": "Rock---Post-Punk", "score": 0.2796 },
    { "label": "Electronic---Darkwave", "score": 0.2024 },
    { "label": "Electronic---Synth-pop", "score": 0.1324 },
    { "label": "Electronic---New Wave", "score": 0.1123 }
  ],
  "path": "/absolute/path/to/file.flac"
}
```

- `embedding` is the **mean-pooled Discogs-EffNet track embedding**: the model
  emits one 1280-dim vector per 16 kHz patch (`[patches, 1280]`), and
  `server.py` averages across the patch axis → exactly **1280 floats**.
- `genres` (optional) is the **top 5** Discogs-400 styles (labels use the
  `Genre---Style` form from the classifier metadata), scored by averaging the
  per-patch sigmoid outputs. Only present when the genre model is installed.
- `key` is formatted `<Pitch>-<Major|Minor>` (sharp spelling, e.g. `A-Minor`,
  `C#-Major`). `camelot` is the Camelot wheel code (`8A`, `11B`, …), or `null`
  if the key could not be mapped. `confidence.{bpm,key}` are the raw Essentia
  confidences (optional, informational).

When `embed` is absent/false the response is **exactly as before** (no
`embedding`/`genres` keys). When `embed` is true but the embedding model is
missing or inference fails, the response still carries the BPM/key fields and
simply **omits** `embedding`/`genres` — the request never becomes a 500.

Errors — `4xx` with `{"error": "..."}`:

| Status | Cause                                                                   |
| ------ | ----------------------------------------------------------------------- |
| `400`  | invalid JSON body / missing or non-absolute `path` / not a regular file |
| `404`  | file does not exist                                                     |
| `411`  | missing `Content-Length`                                                |
| `415`  | unsupported audio extension                                             |
| `422`  | decoding/analysis failed                                                |

## Model files

Stored under `/home/momo/mmm-analyzer/models/`. Fetched from
`https://essentia.upf.edu/models/` (see `install.sh`; sizes are verified after
download to guard against truncated files):

| File                                     | Size (bytes) | Source                                   | Purpose                                                                                           |
| ---------------------------------------- | ------------ | ---------------------------------------- | ------------------------------------------------------------------------------------------------- |
| `discogs-effnet-bs64-1.pb`               | 18366619     | `feature-extractors/discogs-effnet/`     | Discogs-EffNet embedding graph (output `PartitionedCall:1`, `[patches, 1280]`)                    |
| `discogs-effnet-bs64-1.json`             | 14983        | `feature-extractors/discogs-effnet/`     | Model metadata (input `serving_default_melspectrogram` `[64,128,96]`, 16 kHz)                     |
| `genre_discogs400-discogs-effnet-1.pb`   | 2057977      | `classification-heads/genre_discogs400/` | 400-style classifier head (input `serving_default_model_Placeholder`, output `PartitionedCall:0`) |
| `genre_discogs400-discogs-effnet-1.json` | 14951        | `classification-heads/genre_discogs400/` | 400 class labels + layer names                                                                    |

The model files are optional at runtime: if they are absent the service still
serves BPM/key and simply omits embedding fields.

## Files

| File                   | Purpose                                                              |
| ---------------------- | -------------------------------------------------------------------- |
| `server.py`            | the service (stdlib `http.server`, no web framework)                 |
| `requirements.txt`     | pinned Python deps (see the numpy / essentia-tensorflow notes below) |
| `mmm-analyzer.service` | systemd unit                                                         |
| `install.sh`           | idempotent installer (venv + deps + models + unit + enable)          |
| `README.md`            | this file                                                            |

## Install / update

```bash
# from a checkout that has this directory copied to the server:
sudo ./install.sh
```

`install.sh` installs the code into `/home/momo/mmm-analyzer/`, ensures the
model files in `/home/momo/mmm-analyzer/models/`, creates the venv, installs
the pinned requirements, drops the unit in `/etc/systemd/system/`, then
`daemon-reload` + `enable` + `restart`.

## Operate

```bash
sudo systemctl restart mmm-analyzer     # restart
sudo systemctl status  mmm-analyzer     # status
sudo journalctl -u mmm-analyzer -f      # logs
```

## Configuration (env vars, set in the unit)

| Var                          | Default                                  | Meaning                                           |
| ---------------------------- | ---------------------------------------- | ------------------------------------------------- |
| `ANALYZER_HOST`              | `127.0.0.1`                              | bind address (keep on loopback)                   |
| `ANALYZER_PORT`              | `8711`                                   | bind port                                         |
| `ANALYZER_SAMPLE_RATE`       | `44100`                                  | analysis sample rate (BPM/key)                    |
| `ANALYZER_BPM_METHOD`        | `multifeature`                           | `multifeature` \| `degara` \| `percival`          |
| `ANALYZER_MAX_SECONDS`       | `0`                                      | analyse only the first N seconds (0 = whole file) |
| `ANALYZER_LOG_LEVEL`         | `INFO`                                   | log level                                         |
| `ANALYZER_MODELS_DIR`        | `<script dir>/models`                    | directory with the `.pb`/`.json` models           |
| `ANALYZER_ENABLE_EMBEDDINGS` | `1`                                      | set `0` to disable the embedding path entirely    |
| `ANALYZER_EMBED_MODEL`       | `discogs-effnet-bs64-1.pb`               | embedding graph                                   |
| `ANALYZER_GENRE_MODEL`       | `genre_discogs400-discogs-effnet-1.pb`   | optional genre head                               |
| `ANALYZER_GENRE_META`        | `genre_discogs400-discogs-effnet-1.json` | genre labels/layers                               |
| `ANALYZER_EMBED_SAMPLE_RATE` | `16000`                                  | Discogs-EffNet input sample rate                  |
| `ANALYZER_TOP_GENRES`        | `5`                                      | number of top styles returned                     |

## Example calls

```bash
curl -s localhost:8711/health

# BPM/key only (backward compatible)
curl -s -X POST localhost:8711/analyze \
  -H 'Content-Type: application/json' \
  -d '{"path":"/data/public/media/music-api/flac/SE6XW2510722.flac"}'

# BPM/key + 1280-dim embedding + top-5 genres
curl -s -X POST localhost:8711/analyze \
  -H 'Content-Type: application/json' \
  -d '{"path":"/data/public/media/music-api/flac/SE6XW2510722.flac","embed":true}'
```

## Host-specific notes

The target host (`music-catalog`) is a KVM guest whose virtual CPU reports as a
"Common KVM processor" and exposes **only x86-64 baseline (v1)** — no SSE4.2/AVX.
This forces a few pins / choices:

- **`numpy==1.26.4`**. numpy ≥ 2.0 wheels are built with an `x86-64-v2`
  baseline and refuse to import on this CPU
  (`RuntimeError: ... your machine doesn't support: (X86_V2)`). 1.26.4 is built
  for the plain baseline and works.
- **`essentia-tensorflow`, not `essentia`**. The plain `essentia` wheel is
  built **without** TensorFlow support, so it exposes only `TensorflowInput*`
  helpers and has no `TensorflowPredictEffnetDiscogs` / `TensorflowPredict2D` —
  embeddings are impossible with it. The `essentia-tensorflow` wheel is a
  pre-release (`essentia-tensorflow==2.1b6.dev1389`), so `pip install --pre` is
  required.
- **The bundled TensorFlow runs on baseline v1.** Its `cpu_feature_guard`
  reports oneDNN acceleration for **SSE3 only** (no AVX/SSE4.2), and a real
  inference on this host completes without `SIGILL`. A generic TensorFlow build
  or a prebuilt ONNX Runtime would receive an illegal-instruction signal here —
  which is why we deliberately stay on the Essentia wheel.
- `six` (+ `PyYAML`) are pulled in explicitly because installing essentia with
  `--no-deps`/pre-release handling does not always resolve them.

No fallback engine (librosa/aubio) was needed: Essentia installs and runs on
this host.

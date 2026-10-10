#!/usr/bin/env python3
"""mmm-analyzer — headless BPM/key + Discogs-EffNet embedding service.

A tiny local HTTP service that analyzes a server-local audio file (typically a
FLAC) and returns its tempo (BPM) plus musical key, along with the Camelot
wheel notation used by the hub for harmonic mixing. On request it can
additionally return a mean-pooled **Discogs-EffNet** audio embedding (1280-dim)
and, if the genre classifier is installed, the top Discogs-400 styles.

Endpoints
---------
GET  /health
     -> 200 {"ok": true, "engine": "<engine string>", "embeddings": {...}}

POST /analyze
     body: {"path": "/absolute/path/to/file.flac"}
     body: {"path": "/absolute/path/to/file.flac", "embed": true}
     -> 200 {"bpm": 128.0, "key": "A-Minor", "camelot": "8A",
             "confidence": {"bpm": 0.0, "key": 0.0},
             "embedding": [<1280 floats>],
             "genres": [{"label": "...", "score": 0.0}, ...]}
     -> 4xx {"error": "..."} on bad input / missing file

Backward compatibility
----------------------
When ``embed`` is absent/false the response is exactly as before. When
``embed`` is true but the embedding model is missing or inference fails, the
BPM/key response is still returned and ``embedding``/``genres`` are simply
omitted (the request never turns into a 500).

The service binds to 127.0.0.1 only and is designed to run headless under
systemd. Analysis uses the Essentia C++ library via its Python bindings
(`RhythmExtractor2013` for BPM, `KeyExtractor` for the key,
`TensorflowPredictEffnetDiscogs` + `TensorflowPredict2D` for the embeddings /
genres). Essentia is AGPL-3.0 and the EffNet model weights are CC BY-NC-SA 4.0;
see README.md for the licensing note.
"""

from __future__ import annotations

import json
import logging
import os
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

try:
    import numpy as np
    import essentia
    import essentia.standard as es
except Exception as exc:  # pragma: no cover - import-time environment check
    sys.stderr.write(
        "FATAL: could not import essentia/numpy (%s).\n"
        "Install the pinned requirements with: pip install --pre -r requirements.txt\n"
        % exc
    )
    raise

# --------------------------------------------------------------------------- #
# Configuration (overridable via environment variables)
# --------------------------------------------------------------------------- #

HOST = os.environ.get("ANALYZER_HOST", "127.0.0.1")
PORT = int(os.environ.get("ANALYZER_PORT", "8711"))

# Sample rate used for both analysis algorithms. 44100 is the Essentia default
# and what KeyExtractor's profiles were trained on.
SAMPLE_RATE = int(os.environ.get("ANALYZER_SAMPLE_RATE", "44100"))

# RhythmExtractor2013 method: "multifeature" (best quality, slowest),
# "degara" or "percival" (faster). Overridable for speed/latency trade-offs.
BPM_METHOD = os.environ.get("ANALYZER_BPM_METHOD", "multifeature")

# Optional analysis window in seconds (0 = analyze the whole file).
MAX_SECONDS = float(os.environ.get("ANALYZER_MAX_SECONDS", "0"))

MAX_BODY_BYTES = 64 * 1024

# --- embedding model (Discogs-EffNet) ------------------------------------- #

# Directory holding the TensorFlow (.pb) model + metadata (.json) files.
MODELS_DIR = os.environ.get(
    "ANALYZER_MODELS_DIR",
    os.path.join(os.path.dirname(os.path.abspath(__file__)), "models"),
)

# Allow disabling the embedding path entirely (BPM/key only).
EMBED_ENABLED = os.environ.get(
    "ANALYZER_ENABLE_EMBEDDINGS", "1"
).strip().lower() not in ("0", "false", "no", "off", "")

EMBED_MODEL_FILE = os.environ.get("ANALYZER_EMBED_MODEL", "discogs-effnet-bs64-1.pb")
GENRE_MODEL_FILE = os.environ.get(
    "ANALYZER_GENRE_MODEL", "genre_discogs400-discogs-effnet-1.pb"
)
GENRE_META_FILE = os.environ.get(
    "ANALYZER_GENRE_META", "genre_discogs400-discogs-effnet-1.json"
)

# Discogs-EffNet operates on 16 kHz audio (see the model's metadata).
EMBED_SAMPLE_RATE = int(os.environ.get("ANALYZER_EMBED_SAMPLE_RATE", "16000"))

# Layer names come from the models' metadata JSON files.
EMBED_OUTPUT = os.environ.get("ANALYZER_EMBED_OUTPUT", "PartitionedCall:1")
GENRE_INPUT = os.environ.get("ANALYZER_GENRE_INPUT", "serving_default_model_Placeholder")
GENRE_OUTPUT = os.environ.get("ANALYZER_GENRE_OUTPUT", "PartitionedCall:0")

# Number of top styles to return when the genre head is available.
TOP_GENRES = int(os.environ.get("ANALYZER_TOP_GENRES", "5"))

# Expected embedding dimensionality (mean-pooled over the patch axis).
EMBEDDING_DIM = 1280

ENGINE = "essentia %s (RhythmExtractor2013/%s + KeyExtractor)" % (
    essentia.__version__,
    BPM_METHOD,
)

ALLOWED_EXTENSIONS = {
    ".flac", ".wav", ".mp3", ".m4a", ".mp4", ".aac", ".ogg",
    ".oga", ".opus", ".aiff", ".aif", ".wv", ".wma",
}

# --------------------------------------------------------------------------- #
# Key -> Camelot wheel
# --------------------------------------------------------------------------- #

# Canonical spelling uses sharps; flats and enharmonics are normalised onto it.
_ENHARMONIC = {
    "DB": "C#", "EB": "D#", "GB": "F#", "AB": "G#", "BB": "A#",
    "CB": "B", "FB": "E", "E#": "F", "B#": "C",
}

# pitch -> {"major": "<camelot>", "minor": "<camelot>"}
_CAMELOT = {
    "C":  {"major": "8B",  "minor": "5A"},
    "C#": {"major": "3B",  "minor": "12A"},
    "D":  {"major": "10B", "minor": "7A"},
    "D#": {"major": "5B",  "minor": "2A"},
    "E":  {"major": "12B", "minor": "9A"},
    "F":  {"major": "7B",  "minor": "4A"},
    "F#": {"major": "2B",  "minor": "11A"},
    "G":  {"major": "9B",  "minor": "6A"},
    "G#": {"major": "4B",  "minor": "1A"},
    "A":  {"major": "11B", "minor": "8A"},
    "A#": {"major": "6B",  "minor": "3A"},
    "B":  {"major": "1B",  "minor": "10A"},
}


def normalize_pitch(pitch: str) -> str:
    """Normalise an Essentia key name to a sharp-spelled pitch class."""
    p = (pitch or "").strip().upper()
    if p in _ENHARMONIC:
        return _ENHARMONIC[p]
    return p


def to_camelot(pitch: str, scale: str) -> str | None:
    """Return the Camelot code for a pitch + scale, or None if unknown."""
    p = normalize_pitch(pitch)
    s = (scale or "").strip().lower()
    entry = _CAMELOT.get(p)
    if entry is None or s not in entry:
        return None
    return entry[s]


# --------------------------------------------------------------------------- #
# Analysis — BPM / key
# --------------------------------------------------------------------------- #

def analyze_file(path: str) -> dict:
    """Run BPM + key extraction on a server-local audio file."""
    loader = es.MonoLoader(filename=path, sampleRate=SAMPLE_RATE)
    audio = loader()

    if MAX_SECONDS > 0:
        audio = audio[: int(MAX_SECONDS * SAMPLE_RATE)]

    if len(audio) == 0:
        raise ValueError("decoded audio is empty")

    bpm, _beats, bpm_confidence, _estimates, _intervals = es.RhythmExtractor2013(
        method=BPM_METHOD
    )(audio)
    key, scale, key_strength = es.KeyExtractor()(audio)

    camelot = to_camelot(key, scale)
    pretty_scale = "Major" if str(scale).strip().lower().startswith("maj") else "Minor"
    pretty_key = "%s-%s" % (normalize_pitch(key), pretty_scale)

    return {
        "bpm": round(float(bpm), 3),
        "key": pretty_key,
        "camelot": camelot,
        "confidence": {
            "bpm": round(float(bpm_confidence), 4),
            "key": round(float(key_strength), 4),
        },
    }


# --------------------------------------------------------------------------- #
# Analysis — Discogs-EffNet embedding (+ optional genre head)
# --------------------------------------------------------------------------- #

log = logging.getLogger("mmm-analyzer")

# The Essentia TensorFlow algorithms are stateful; serialise access when
# serving concurrent requests with ThreadingHTTPServer.
_embed_lock = threading.Lock()
_embed_loaded = False
_embed_error: str | None = None
_effnet = None
_genre_head = None
_genre_classes: list[str] | None = None


def _model_path(name: str) -> str:
    return os.path.join(MODELS_DIR, name)


def embedding_model_available() -> bool:
    """Cheap (no-load) check whether the embedding model file is present."""
    return EMBED_ENABLED and os.path.isfile(_model_path(EMBED_MODEL_FILE))


def genre_model_available() -> bool:
    """Cheap check whether the optional genre head + metadata are present."""
    return EMBED_ENABLED and (
        os.path.isfile(_model_path(GENRE_MODEL_FILE))
        and os.path.isfile(_model_path(GENRE_META_FILE))
    )


def _ensure_embedding_models() -> None:
    """Lazily load the embedding (and genre) models; cache the result.

    Never raises: on failure ``_embed_error`` is set and ``_effnet`` stays
    None, which the caller treats as "embeddings unavailable".
    """
    global _embed_loaded, _embed_error, _effnet, _genre_head, _genre_classes
    if _embed_loaded:
        return
    _embed_loaded = True

    if not EMBED_ENABLED:
        _embed_error = "embeddings disabled via ANALYZER_ENABLE_EMBEDDINGS"
        return

    try:
        effnet_path = _model_path(EMBED_MODEL_FILE)
        if not os.path.isfile(effnet_path):
            raise FileNotFoundError(effnet_path)
        _effnet = es.TensorflowPredictEffnetDiscogs(
            graphFilename=effnet_path, output=EMBED_OUTPUT
        )
        log.info("loaded embedding model %s", effnet_path)
    except Exception as exc:  # noqa: BLE001
        _effnet = None
        _embed_error = "embedding model unavailable: %s" % exc
        log.warning(_embed_error)
        return

    # Optional genre classifier on top of the embedding.
    try:
        genre_path = _model_path(GENRE_MODEL_FILE)
        meta_path = _model_path(GENRE_META_FILE)
        if os.path.isfile(genre_path) and os.path.isfile(meta_path):
            _genre_head = es.TensorflowPredict2D(
                graphFilename=genre_path, input=GENRE_INPUT, output=GENRE_OUTPUT
            )
            with open(meta_path, "r", encoding="utf-8") as fh:
                _genre_classes = json.load(fh).get("classes") or None
            log.info("loaded genre model %s", genre_path)
    except Exception as exc:  # noqa: BLE001
        _genre_head = None
        _genre_classes = None
        log.warning("genre model unavailable, returning embeddings only: %s", exc)


def compute_embedding(path: str) -> tuple[list[float], list[dict] | None]:
    """Return ``(embedding, genres)`` for a server-local audio file.

    ``embedding`` is the mean-pooled 1280-dim Discogs-EffNet track embedding.
    ``genres`` is the top-N Discogs-400 styles, or None if the genre head is
    not installed. Raises on failure (caller decides to omit the fields).
    """
    with _embed_lock:
        _ensure_embedding_models()
        if _effnet is None:
            raise RuntimeError(_embed_error or "embedding model not loaded")

        loader = es.MonoLoader(
            filename=path, sampleRate=EMBED_SAMPLE_RATE, resampleQuality=4
        )
        audio = loader()
        if len(audio) == 0:
            raise ValueError("decoded audio is empty")

        raw = _effnet(audio)
        patches = np.asarray(raw, dtype="float32")
        if patches.ndim == 1:
            patches = patches.reshape(1, -1)

        embedding = patches.mean(axis=0)
        if embedding.shape[-1] != EMBEDDING_DIM:
            raise RuntimeError(
                "unexpected embedding dimension %d (expected %d)"
                % (embedding.shape[-1], EMBEDDING_DIM)
            )

        genres = None
        if _genre_head is not None and _genre_classes:
            scores = np.asarray(_genre_head(patches), dtype="float32").mean(axis=0)
            if scores.shape[-1] == len(_genre_classes):
                top = np.argsort(scores)[::-1][: max(0, TOP_GENRES)]
                genres = [
                    {
                        "label": _genre_classes[int(i)],
                        "score": round(float(scores[int(i)]), 6),
                    }
                    for i in top
                ]

        return [float(x) for x in embedding], genres


# --------------------------------------------------------------------------- #
# HTTP
# --------------------------------------------------------------------------- #

class Handler(BaseHTTPRequestHandler):
    server_version = "mmm-analyzer"
    protocol_version = "HTTP/1.1"

    # -- helpers ----------------------------------------------------------- #

    def _send_json(self, status: int, payload: dict) -> None:
        body = json.dumps(payload).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _error(self, status: int, message: str) -> None:
        self._send_json(status, {"error": message})

    def log_message(self, fmt: str, *args) -> None:  # noqa: A003
        log.info("%s - %s", self.address_string(), fmt % args)

    @staticmethod
    def _as_bool(value) -> bool:
        if isinstance(value, bool):
            return value
        if isinstance(value, str):
            return value.strip().lower() in ("1", "true", "yes", "on")
        return bool(value)

    # -- routes ------------------------------------------------------------ #

    def do_GET(self) -> None:  # noqa: N802
        if self.path.rstrip("/") in ("/health", ""):
            self._send_json(
                200,
                {
                    "ok": True,
                    "engine": ENGINE,
                    "embeddings": {
                        "enabled": EMBED_ENABLED,
                        "available": embedding_model_available(),
                        "model": EMBED_MODEL_FILE,
                        "dim": EMBEDDING_DIM,
                        "genres_available": genre_model_available(),
                    },
                },
            )
        else:
            self._error(404, "not found")

    def do_POST(self) -> None:  # noqa: N802
        if self.path.rstrip("/") != "/analyze":
            self._error(404, "not found")
            return

        raw_len = self.headers.get("Content-Length")
        if raw_len is None:
            self._error(411, "missing Content-Length")
            return
        try:
            length = int(raw_len)
        except ValueError:
            self._error(400, "invalid Content-Length")
            return
        if length <= 0 or length > MAX_BODY_BYTES:
            self._error(400, "invalid or oversized request body")
            return

        raw = self.rfile.read(length)
        try:
            data = json.loads(raw.decode("utf-8"))
        except (UnicodeDecodeError, json.JSONDecodeError):
            self._error(400, "body is not valid JSON")
            return
        if not isinstance(data, dict):
            self._error(400, "body must be a JSON object")
            return

        path = data.get("path")
        if not isinstance(path, str) or not path.strip():
            self._error(400, "missing required string field 'path'")
            return
        path = path.strip()

        if not os.path.isabs(path):
            self._error(400, "'path' must be an absolute path")
            return
        if not os.path.exists(path):
            self._error(404, "file not found: %s" % path)
            return
        if not os.path.isfile(path):
            self._error(400, "path is not a regular file: %s" % path)
            return

        ext = os.path.splitext(path)[1].lower()
        if ext not in ALLOWED_EXTENSIONS:
            self._error(415, "unsupported audio extension: %s" % (ext or "(none)"))
            return

        want_embed = self._as_bool(data.get("embed", False))

        try:
            result = analyze_file(path)
        except Exception as exc:  # noqa: BLE001
            log.exception("analysis failed for %s", path)
            self._error(422, "analysis failed: %s" % exc)
            return

        result["path"] = path

        # Embeddings are best-effort: never fail the request over them.
        if want_embed:
            try:
                embedding, genres = compute_embedding(path)
                result["embedding"] = embedding
                if genres is not None:
                    result["genres"] = genres
            except Exception as exc:  # noqa: BLE001
                log.warning("embedding failed for %s: %s", path, exc)

        self._send_json(200, result)


def main() -> None:
    logging.basicConfig(
        level=os.environ.get("ANALYZER_LOG_LEVEL", "INFO"),
        format="%(asctime)s %(levelname)s %(name)s: %(message)s",
    )
    server = ThreadingHTTPServer((HOST, PORT), Handler)
    server.daemon_threads = True
    log.info(
        "mmm-analyzer listening on http://%s:%d (engine: %s, embeddings: %s)",
        HOST,
        PORT,
        ENGINE,
        "available" if embedding_model_available() else "unavailable",
    )
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        log.info("shutting down")
    finally:
        server.server_close()


if __name__ == "__main__":
    main()

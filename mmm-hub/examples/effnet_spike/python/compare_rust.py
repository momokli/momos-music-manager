#!/usr/bin/env python3
"""Cross-validate the Rust prototype against the (validated) Python pipeline.

Reads:  audio/test_clip_16k_mono.wav  (PCM16, same samples both sides)
        audio/mel_rust.f32            (raw f32 LE, [n_frames, 96])
        audio/embedding_rust.f32      (raw f32 LE, [1280])
Compares Rust mel vs pure-numpy mel, and Rust embedding vs Python embedding.
"""
import numpy as np
import soundfile as sf
import onnxruntime as ort

import mel_numpy as mn

AUDIO = "audio/test_clip_16k_mono.wav"
N_BANDS = 96
PATCH_SIZE, PATCH_HOP = 128, 62

signal, sr = sf.read(AUDIO, dtype="float64")
assert sr == 16000
print(f"audio: {len(signal)} samples @16k")

# --- mel ---
mel_py = mn.mel_frames(signal)
mel_rust = np.fromfile("audio/mel_rust.f32", dtype="<f4").reshape(-1, N_BANDS)
print(f"mel py={mel_py.shape}  rust={mel_rust.shape}")
n = min(len(mel_py), len(mel_rust))
dmel = np.abs(mel_py[:n].astype(np.float32) - mel_rust[:n])
print(f"mel max abs diff : {dmel.max():.6e}")
print(f"mel mean abs diff: {dmel.mean():.6e}")
print(f"mel relative max : {dmel.max()/(np.abs(mel_py[:n]).max()+1e-9):.3e}")

# --- embedding ---
def make_patches(mel):
    n = mel.shape[0]
    npch = 1 + (n - PATCH_SIZE) // PATCH_HOP
    out = np.empty((npch, PATCH_SIZE, N_BANDS), dtype=np.float32)
    for p in range(npch):
        out[p] = mel[p * PATCH_HOP: p * PATCH_HOP + PATCH_SIZE]
    return out

sess = ort.InferenceSession("models/discogs-effnet-bsdynamic-1.onnx",
                            providers=["CPUExecutionProvider"])
in_name = sess.get_inputs()[0].name
emb_py, _ = sess.run(["embeddings", "activations"], {in_name: make_patches(mel_py)})
emb_py = emb_py.mean(axis=0)
emb_rust = np.fromfile("audio/embedding_rust.f32", dtype="<f4")
cos = float(emb_py @ emb_rust / (np.linalg.norm(emb_py) * np.linalg.norm(emb_rust)))
print(f"\nembedding py norm={np.linalg.norm(emb_py):.4f}  rust norm={np.linalg.norm(emb_rust):.4f}")
print(f"cosine(python, rust) = {cos:.6f}")
print(f"embedding max abs diff = {np.abs(emb_py - emb_rust).max():.6e}")

print("\nRESULT:", "PASS" if (dmel.max() < 1e-4 and cos > 0.99999) else "FAIL")

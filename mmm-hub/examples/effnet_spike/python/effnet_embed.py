#!/usr/bin/env python3
"""Discogs-EffNet embedding + genre, reference pipeline.

Mel stage : Essentia `TensorflowInputMusiCNN` (ground truth for the recipe).
Inference : the HuqgingFace ONNX mirror of discogs-effnet via onnxruntime.

Recipe (verified against Essentia source `tensorflowinputmusicnn.{h,cpp}` and
`tensorflowpredicteffnetdiscogs.{h,cpp}`):
  - mono, 16 kHz
  - FrameCutter frameSize=512, hopSize=256 (zero-padded last frame)
  - per frame: Hann window (unnormalized) -> |rFFT|^2 (size=512 -> 257 bins)
    -> 96 Mel bands (slaneyMel warping, linear weighting, unit_tri norm)
    -> scale*10000 + shift*1 -> log10
  - patches of patchSize=128 frames, patchHopSize=62 (Essentia default)
  - model input  [batch, 128, 96]  (frames x mel bands)
  - model outputs: embeddings [batch,1280], activations [batch,400] (sigmoid)

Usage: python effnet_embed.py [audio.wav]
"""
import json
import resource
import sys
import time

import numpy as np
import onnxruntime as ort
from essentia.standard import MonoLoader, FrameCutter, TensorflowInputMusiCNN

MODEL = "models/discogs-effnet-bsdynamic-1.onnx"
LABELS = "models/genre_discogs400-discogs-effnet-1.json"

FRAME_SIZE = 512
HOP_SIZE = 256
N_BANDS = 96
PATCH_SIZE = 128
PATCH_HOP = 62  # Essentia default


def peak_rss_mb():
    # ru_maxrss is bytes on macOS, KiB on Linux
    rss = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    if sys.platform == "darwin":
        return rss / (1024 * 1024)
    return rss / 1024


def compute_mel_frames(audio):
    """Return mel matrix [n_frames, 96] via Essentia (ground-truth recipe)."""
    fc = FrameCutter(frameSize=FRAME_SIZE, hopSize=HOP_SIZE)
    mel_in = TensorflowInputMusiCNN()
    frames = []
    while True:
        frame = fc(audio)
        if not len(frame):
            break
        frames.append(mel_in(frame))
    return np.asarray(frames, dtype=np.float32)


def make_patches(mel, patch_size=PATCH_SIZE, hop=PATCH_HOP, last_mode="discard"):
    n = mel.shape[0]
    if n < patch_size:
        return None, 0
    if last_mode == "discard":
        n_patches = 1 + (n - patch_size) // hop
    else:  # repeat
        n_patches = 1 + int(np.ceil((n - patch_size) / hop))
    patches = np.empty((n_patches, patch_size, N_BANDS), dtype=np.float32)
    for p in range(n_patches):
        s = p * hop
        e = s + patch_size
        if e <= n:
            patches[p] = mel[s:e]
        else:
            take = n - s
            patches[p, :take] = mel[s:]
            patches[p, take:] = mel[n - 1]  # "repeat" last frame
    return patches, n_patches


def main():
    path = sys.argv[1] if len(sys.argv) > 1 else "audio/test_clip_44k_stereo.wav"

    t0 = time.perf_counter()
    audio = MonoLoader(filename=path, sampleRate=16000, resampleQuality=4)()
    t_load = time.perf_counter() - t0
    dur = len(audio) / 16000.0
    print(f"audio: {path}  {dur:.2f}s @16k mono ({len(audio)} samples), load+resample {t_load*1000:.0f} ms")

    t0 = time.perf_counter()
    mel = compute_mel_frames(audio)
    t_mel = time.perf_counter() - t0
    print(f"mel: {mel.shape} (frames x 96 bands) in {t_mel*1000:.0f} ms  "
          f"({mel.shape[0]/(t_mel):.0f} frames/s)")

    patches, n_patches = make_patches(mel)
    print(f"patches: {patches.shape} (patchHop={PATCH_HOP}, lastPatchMode=discard)")

    so = ort.SessionOptions()
    so.intra_op_num_threads = 0  # 0 => ORT default (all cores)
    sess = ort.InferenceSession(MODEL, sess_options=so, providers=["CPUExecutionProvider"])
    in_name = sess.get_inputs()[0].name

    # warmup
    sess.run(None, {in_name: patches[:1]})

    t0 = time.perf_counter()
    emb, act = sess.run(["embeddings", "activations"], {in_name: patches})
    t_inf = time.perf_counter() - t0
    print(f"onnx: embeddings {emb.shape}, activations {act.shape} in {t_inf*1000:.0f} ms "
          f"({n_patches/t_inf:.0f} patches/s, batch={n_patches})")

    total = t_load + t_mel + t_inf
    embed = emb.mean(axis=0)  # 1280-d track embedding (mean over patches)
    print(f"embedding: dim={embed.shape[0]}, norm={np.linalg.norm(embed):.4f}")
    print(f"TOTAL wall (load+mel+onnx): {total*1000:.0f} ms for {dur:.1f}s audio => "
          f"{dur/total:.1f}x realtime")
    print(f"peak RSS: {peak_rss_mb():.1f} MB")

    # genre
    meta = json.load(open(LABELS))
    classes = meta["classes"]
    probs = act.mean(axis=0)
    order = np.argsort(probs)[::-1][:10]
    print("\ntop-10 Discogs400 styles (mean over patches):")
    for i in order:
        print(f"  {probs[i]:6.3f}  {classes[i]}")

    np.save("audio/embedding.npy", embed)
    print("\nsaved audio/embedding.npy")


if __name__ == "__main__":
    main()

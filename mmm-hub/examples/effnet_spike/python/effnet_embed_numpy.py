#!/usr/bin/env python3
"""End-to-end embedding using the PURE-NUMPY mel (mel_numpy.py) + ONNX.

Proves the whole pipeline is reproducible without Essentia for the feature
stage. (MonoLoader is used only for 16 kHz decode/resample, which in Rust will
be handled by symphonia + rubato.) Compares against the Essentia-mel embedding.
"""
import json
import resource
import sys
import time

import numpy as np
import onnxruntime as ort
from essentia.standard import MonoLoader

import mel_numpy as mn

MODEL = "models/discogs-effnet-bsdynamic-1.onnx"
LABELS = "models/genre_discogs400-discogs-effnet-1.json"
PATCH_SIZE = 128
PATCH_HOP = 62


def make_patches(mel, patch_size=PATCH_SIZE, hop=PATCH_HOP, last_mode="discard"):
    n = mel.shape[0]
    if n < patch_size:
        return None
    n_patches = 1 + (n - patch_size) // hop if last_mode == "discard" \
        else 1 + int(np.ceil((n - patch_size) / hop))
    patches = np.empty((n_patches, patch_size, mn.N_BANDS), dtype=np.float32)
    for p in range(n_patches):
        s = p * hop
        e = s + patch_size
        if e <= n:
            patches[p] = mel[s:e]
        else:
            take = n - s
            patches[p, :take] = mel[s:]
            patches[p, take:] = mel[n - 1]
    return patches


def peak_rss_mb():
    rss = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    return rss / (1024 * 1024) if sys.platform == "darwin" else rss / 1024


def main():
    path = sys.argv[1] if len(sys.argv) > 1 else "audio/test_clip_44k_stereo.wav"

    t0 = time.perf_counter()
    audio = MonoLoader(filename=path, sampleRate=16000, resampleQuality=4)()
    t_load = time.perf_counter() - t0

    t0 = time.perf_counter()
    mel = mn.mel_frames(audio)
    t_mel = time.perf_counter() - t0
    print(f"numpy mel: {mel.shape} in {t_mel*1000:.0f} ms "
          f"({mel.shape[0]/t_mel:.0f} frames/s)")

    patches = make_patches(mel)
    print(f"patches: {patches.shape}")

    sess = ort.InferenceSession(MODEL, providers=["CPUExecutionProvider"])
    in_name = sess.get_inputs()[0].name
    sess.run(None, {in_name: patches[:1]})  # warmup

    t0 = time.perf_counter()
    emb, act = sess.run(["embeddings", "activations"], {in_name: patches})
    t_inf = time.perf_counter() - t0
    print(f"onnx: {emb.shape} / {act.shape} in {t_inf*1000:.0f} ms")

    embed = emb.mean(axis=0)
    total = t_load + t_mel + t_inf
    dur = len(audio) / 16000.0
    print(f"TOTAL numpy-pipeline: {total*1000:.0f} ms ({dur/total:.1f}x realtime), "
          f"peak RSS {peak_rss_mb():.1f} MB")

    # compare to essentia-pipeline embedding
    try:
        ref = np.load("audio/embedding.npy")
        cos = float(embed @ ref / (np.linalg.norm(embed) * np.linalg.norm(ref)))
        print(f"cosine(numpy-embed, essentia-embed) = {cos:.6f}")
    except FileNotFoundError:
        print("(run python/effnet_embed.py first to compare against essentia mel)")

    meta = json.load(open(LABELS))
    probs = act.mean(axis=0)
    order = np.argsort(probs)[::-1][:10]
    print("top-10 styles:")
    for i in order:
        print(f"  {probs[i]:6.3f}  {meta['classes'][i]}")


if __name__ == "__main__":
    main()

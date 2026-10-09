#!/usr/bin/env python3
"""Validate the pure-NumPy mel (mel_numpy.py) against Essentia's ground truth.

Steps:
  1. Compare Essentia FrameCutter frames vs. our frames_from_signal (alignment).
  2. Feed identical frames through Essentia TensorflowInputMusiCNN and through
     our numpy mel; report max/mean abs error.
"""
import sys

import numpy as np
from essentia.standard import MonoLoader, FrameCutter, TensorflowInputMusiCNN

import mel_numpy as mn

AUDIO = sys.argv[1] if len(sys.argv) > 1 else "audio/test_clip_44k_stereo.wav"


def essentia_frames_and_mel(audio):
    fc = FrameCutter(frameSize=512, hopSize=256)
    mel_in = TensorflowInputMusiCNN()
    frames, mels = [], []
    while True:
        frame = fc(audio)
        if not len(frame):
            break
        frames.append(np.asarray(frame))
        mels.append(np.asarray(mel_in(frame)))
    return np.asarray(frames), np.asarray(mels)


def main():
    audio = MonoLoader(filename=AUDIO, sampleRate=16000, resampleQuality=4)()
    print(f"audio {AUDIO}: {len(audio)} samples @16k")

    e_frames, e_mels = essentia_frames_and_mel(audio)
    print(f"essentia frames: {e_frames.shape}, mel: {e_mels.shape}")

    # --- 1. frame alignment ---
    my_frames = mn.frames_from_signal(audio)
    print(f"numpy   frames: {my_frames.shape}")
    if my_frames.shape[0] != e_frames.shape[0]:
        # essentia may drop the final padded frame; truncate to compare overlap
        n = min(my_frames.shape[0], e_frames.shape[0])
        print(f"  !! frame count differs (essentia {e_frames.shape[0]} vs numpy "
              f"{my_frames.shape[0]}); comparing first {n}")
    else:
        n = my_frames.shape[0]
    fd = np.abs(my_frames[:n] - e_frames[:n])
    print(f"frame max abs diff (first {n}): {fd.max():.6e}")

    # --- 2. mel on identical frames ---
    # use essentia's frames so we isolate the mel math
    window = mn.hann_window(512)
    fb = mn.mel_filterbank()
    my_mels = np.empty_like(e_mels, dtype=np.float32)
    for k in range(e_frames.shape[0]):
        spec = np.abs(np.fft.rfft(e_frames[k] * window, n=512))
        energy = fb @ (spec * spec)
        my_mels[k] = np.log10(1.0 + 10000.0 * energy)

    d = np.abs(my_mels - e_mels)
    print(f"mel max abs diff : {d.max():.6e}")
    print(f"mel mean abs diff: {d.mean():.6e}")
    print(f"mel ref range    : [{e_mels.min():.4f}, {e_mels.max():.4f}]")
    print(f"relative max err : {d.max()/ (np.abs(e_mels).max()+1e-9):.3e}")

    # full-pipeline numpy mel (own frames) vs essentia
    my_full = mn.mel_frames(audio)
    n2 = min(my_full.shape[0], e_mels.shape[0])
    dfull = np.abs(my_full[:n2] - e_mels[:n2])
    print(f"full-pipeline mel max abs diff (first {n2}): {dfull.max():.6e}")

    ok = d.max() < 1e-4
    print("\nRESULT:", "PASS (numpy mel matches Essentia)" if ok else "FAIL")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())

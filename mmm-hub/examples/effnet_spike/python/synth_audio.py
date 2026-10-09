#!/usr/bin/env python3
"""Synthesize a ~30 s pseudo-electronic test clip (kick + bass + chord stabs + hats).

Written as 44.1 kHz stereo WAV so the downstream pipeline also exercises the
16 kHz resampling path (MonoLoader). Deterministic (fixed RNG seed).
"""
import numpy as np
import soundfile as sf

SR = 44100
DUR = 30.0
BPM = 124.0
SEED = 1234

rng = np.random.default_rng(SEED)
n = int(SR * DUR)
t = np.arange(n) / SR

beat = 60.0 / BPM
out = np.zeros(n, dtype=np.float64)

def add(sig, start):
    i = int(start * SR)
    j = min(n, i + len(sig))
    if i >= n:
        return
    out[i:j] += sig[: j - i]

# --- Kick: decaying sine sweep 120->45 Hz, on every beat ---
kick_len = int(0.35 * SR)
klen = np.arange(kick_len) / SR
kick_env = np.exp(-klen * 18.0)
kick_freq = 45 + 75 * np.exp(-klen * 30.0)
kick = np.sin(2 * np.pi * np.cumsum(kick_freq) / SR) * kick_env
nb_beats = int(DUR / beat)
for b in range(nb_beats):
    add(kick * 0.9, b * beat)

# --- Offbeat hat: filtered noise ---
hat_len = int(0.06 * SR)
hl = np.arange(hat_len) / SR
hat_env = np.exp(-hl * 60.0)
hat = rng.standard_normal(hat_len) * hat_env * 0.3
for b in range(nb_beats):
    add(hat, b * beat + beat / 2)

# --- Bassline: root notes following a simple progression (A minor-ish) ---
def note(freq, dur, amp=0.25, harmonics=(1, 2, 3)):
    ln = int(dur * SR)
    lt = np.arange(ln) / SR
    env = np.minimum(1.0, lt * 40) * np.exp(-lt * 1.5)
    s = np.zeros(ln)
    for h in harmonics:
        s += (1.0 / h) * np.sin(2 * np.pi * freq * h * lt)
    return s * env * amp

a2, c3, e3, g3 = 110.0, 130.81, 164.81, 196.0
prog = [a2, c3, e3, g3]
nb_bars = int(DUR / (4 * beat))
for bar in range(nb_bars):
    root = prog[bar % len(prog)]
    for e in range(4):
        add(note(root, beat * 0.9), bar * 4 * beat + e * beat)

# --- Chord stabs (pad) ---
for bar in range(nb_bars):
    root = prog[bar % len(prog)]
    stab = note(root * 2, beat * 1.5, amp=0.12, harmonics=(1, 2, 3, 4, 5))
    stab += note(root * 2.5, beat * 1.5, amp=0.10)
    stab += note(root * 3, beat * 1.5, amp=0.08)
    add(stab, bar * 4 * beat + 2 * beat)

# Normalize and make stereo (identical channels -> MonoLoader averages to same)
out /= np.max(np.abs(out)) + 1e-9
out *= 0.85
stereo = np.stack([out, out], axis=1).astype(np.float32)

sf.write("audio/test_clip_44k_stereo.wav", stereo, SR, subtype="PCM_16")
print(f"wrote audio/test_clip_44k_stereo.wav  {DUR}s @ {SR}Hz stereo, {n} samples/ch")

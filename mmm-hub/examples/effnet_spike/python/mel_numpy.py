#!/usr/bin/env python3
"""Pure-NumPy re-implementation of Essentia's MusiCNN/EffNet mel front-end.

This is the *portable recipe* (for the eventual Rust port). It reproduces:

  Windowing(hann, normalized=False)
    -> Spectrum (size=512, linear magnitude)
    -> MelBands(numberBands=96, slaneyMel, weighting=linear,
                normalize=unit_tri, type=power, log=false)
    -> UnaryOperator(identity, scale=10000, shift=1)   # 10000*mel + 1
    -> UnaryOperator(log10)

Verified against essentia.standard.TensorflowInputMusiCNN (see validate_mel.py).

FrameCutter note: numpy frames here start at sample 0 with hop 256 and the final
frame is zero-padded (matching Essentia FrameCutter default startFromZero=True,
validFrameThresholdRatio=0). See validate_mel.py for the exact frame alignment.
"""
import math

import numpy as np

SR = 16000
N_FFT = 512
HOP = 256
N_BANDS = 96
FMIN = 0.0
FMAX = 8000.0


# --- Slaney mel scale (essentiamath.h: hz2melSlaney / mel2hzSlaney) ---
_MIN_LOG_HZ = 1000.0
_LIN_SLOPE = 3.0 / 200.0
_MIN_LOG_MEL = _MIN_LOG_HZ * _LIN_SLOPE
_LOG_STEP = math.log(6.4) / 27.0


def hz2mel_slaney(hz):
    if hz < _MIN_LOG_HZ:
        return hz * _LIN_SLOPE
    return _MIN_LOG_MEL + math.log(hz / _MIN_LOG_HZ) / _LOG_STEP


def mel2hz_slaney(mel):
    if mel < _MIN_LOG_MEL:
        return mel / _LIN_SLOPE
    return _MIN_LOG_HZ * math.exp((mel - _MIN_LOG_MEL) * _LOG_STEP)


def hann_window(n=N_FFT):
    i = np.arange(n)
    return 0.5 - 0.5 * np.cos(2.0 * np.pi * i / (n - 1.0))


def mel_filterbank(sr=SR, n_fft=N_FFT, n_bands=N_BANDS, fmin=FMIN, fmax=FMAX):
    """Reproduce Essentia TriangularBands::createFilters with weighting=linear,
    normalize=unit_tri."""
    spectrum_size = n_fft // 2 + 1
    low_mel = hz2mel_slaney(fmin)
    high_mel = hz2mel_slaney(fmax)
    mel_inc = (high_mel - low_mel) / (n_bands + 1)
    freqs = np.array([mel2hz_slaney(low_mel + i * mel_inc) for i in range(n_bands + 2)])

    frequency_scale = (sr / 2.0) / (spectrum_size - 1)
    filters = np.zeros((n_bands, spectrum_size))
    for i in range(n_bands):
        fstep1 = freqs[i + 1] - freqs[i]  # weighter = identity (linear)
        fstep2 = freqs[i + 2] - freqs[i + 1]
        jbegin = int(math.ceil(freqs[i] / frequency_scale))
        jend = int(math.floor(freqs[i + 2] / frequency_scale))
        for j in range(jbegin, jend + 1):
            binfreq = j * frequency_scale
            if binfreq < freqs[i + 1]:
                c = (binfreq - freqs[i]) / fstep1
            else:
                c = (freqs[i + 2] - binfreq) / fstep2
            filters[i, j] = c
        weight = (fstep1 + fstep2) / 2.0  # unit_tri -> theoretical triangular area
        filters[i, jbegin:jend + 1] /= weight
    return filters


def frames_from_signal(signal, frame_size=N_FFT, hop=HOP, start_from_zero=False):
    """Reproduce Essentia FrameCutter (default startFromZero=False).

    With startFromZero=False the first frame is *zero-centered at sample 0*:
    startIndex init = -(frame_size+1)/2 (integer division, truncation toward 0)
    = -256 for frame_size=512. Frame k starts at startIndex + k*hop and is
    left/right zero-padded. The last frame is the one whose center
    (start + frame_size/2) reaches or passes the end of the signal.
    """
    n = len(signal)
    off = (frame_size + 1) // 2  # C++ truncating division of (frame_size+1)/2
    base = 0 if start_from_zero else -off
    frames = []
    k = 0
    while True:
        start = base + k * hop
        if start >= n:
            break
        frame = np.zeros(frame_size, dtype=np.float64)
        s = max(0, start)
        e = min(n, start + frame_size)
        if e > s:
            frame[s - start: s - start + (e - s)] = signal[s:e]
        frames.append(frame)
        if start_from_zero:
            if start + frame_size >= n:  # last frame (end at/past EOF)
                break
        else:
            if start + frame_size // 2 >= n:  # last frame (center at/past EOF)
                break
        k += 1
    if not frames:
        return np.zeros((1, frame_size), dtype=np.float64)
    return np.asarray(frames)


def mel_frames(signal, sr=SR, n_fft=N_FFT, hop=HOP, n_bands=N_BANDS):
    """signal: 1-D float at sr. Returns [n_frames, n_bands] log-mel."""
    window = hann_window(n_fft)
    fb = mel_filterbank(sr, n_fft, n_bands)
    frames = frames_from_signal(signal, n_fft, hop)
    out = np.empty((frames.shape[0], n_bands), dtype=np.float32)
    for k, frame in enumerate(frames):
        spec = np.abs(np.fft.rfft(frame * window, n=n_fft))  # linear magnitude
        energy = fb @ (spec * spec)                           # type=power
        out[k] = np.log10(1.0 + 10000.0 * energy)             # 10000*x+1 -> log10
    return out


if __name__ == "__main__":
    fb = mel_filterbank()
    print("filterbank:", fb.shape)
    print("band 0  nonzero bins:", int((fb[0] != 0).sum()))
    print("band 95 nonzero bins:", int((fb[95] != 0).sum()))
    print("first center freqs (Hz):",
          np.round([mel2hz_slaney(hz2mel_slaney(0) + i * (hz2mel_slaney(8000) / 97))
                    for i in (0, 1, 2, 95, 96, 97)], 1))

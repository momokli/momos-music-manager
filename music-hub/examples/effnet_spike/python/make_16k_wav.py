#!/usr/bin/env python3
"""Write the 16 kHz mono reference WAV using Essentia's MonoLoader (resampleQuality=4)
as PCM16, so the Rust prototype reads identical samples for validation."""
import numpy as np
import soundfile as sf
from essentia.standard import MonoLoader

audio = MonoLoader(filename="audio/test_clip_44k_stereo.wav", sampleRate=16000,
                   resampleQuality=4)()
sf.write("audio/test_clip_16k_mono.wav", audio.astype(np.float32), 16000, subtype="PCM_16")
print(f"wrote audio/test_clip_16k_mono.wav: {len(audio)} samples @16k (PCM16)")

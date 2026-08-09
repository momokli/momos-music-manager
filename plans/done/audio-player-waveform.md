## Plan: audio-player-waveform

**Status**: done ✅
**Branch**: `feat/audio-player-waveform`
**Ready for review**: yes
**Depends on**: `feat/digging-frontend`
**Migration needed**: no

### Description

Replace the basic play/pause button in suggestion and staging cards with a mini audio player featuring a seekable progress bar and waveform visualization. Waveform is rendered client-side using Web Audio API (no backend changes).

### Design

Each suggestion/staging card gets a mini player:

```
▶ ████████████░░░░░░░░░░  1:23 / 3:24
   ▂▃▄▅▆▇██▇▆▅▄▃▂▁▁▂▃▄▅▆▇██▇▆▅▄▃▂
```

### How it works

1. Click ▶ → `<audio>` plays (existing behavior)
2. On first play, fetch audio as ArrayBuffer, decode via `AudioContext.decodeAudioData()`
3. Downsample PCM data to ~200 peak bars, draw on `<canvas>`
4. Progress shown as colored fill on top of waveform
5. Click anywhere on waveform/progress to seek
6. Only one audio at a time (existing behavior)

### Key functions

| Function                                                                      | Purpose                                          |
| ----------------------------------------------------------------------------- | ------------------------------------------------ |
| `loadWaveform(fileId)`                                                        | Fetch + decode audio, compute peaks, draw canvas |
| `drawWaveform(fileId, peaks, progress)`                                       | Render waveform bars with progress fill          |
| `wireWaveformSeek()`                                                          | Click/drag handler for seeking                   |
| `setupProgressUpdates()`                                                      | setInterval to update progress + redraw waveform |
| Audio format: fetch full stream, decode PCM, store in Map (cached per fileId) |

### Player rendering (replaces current `.sugg-player`)

```html
<div class="audio-player" data-file-id="${s.fileId}">
  <button class="btn-play" data-file-id="${s.fileId}"><i class="fas fa-play"></i></button>
  <div class="waveform-wrap" data-file-id="${s.fileId}">
    <canvas
      class="waveform-canvas"
      data-file-id="${s.fileId}"
      width="200"
      height="40"
    ></canvas>
    <div class="waveform-progress" data-file-id="${s.fileId}"></div>
  </div>
  <span class="time-display" data-file-id="${s.fileId}"
    >0:00 / ${formatTime(duration)}</span
  >
  <audio class="audio-el" data-file-id="${s.fileId}" preload="none">
    <source src="/api/files/${s.fileId}/stream" />
  </audio>
</div>
```

### Files: `frontend/pages/digging.js` + `frontend/style.css` (no backend)

### Acceptance Criteria

- [ ] ▶ fetches audio, decodes, renders waveform as peak bars
- [ ] Progress shown as colored fill over waveform
- [ ] Click on waveform/progress bar seeks to position
- [ ] Only one audio at a time
- [ ] Waveform cached per fileId (no re-fetch on re-play)
- [ ] Waveform works in both suggestion cards and staging cards
- [ ] Time display shows current/total
- [ ] No regressions: Add, Remove, Refine, Save, key coverage still work
- [ ] Backend unchanged

---


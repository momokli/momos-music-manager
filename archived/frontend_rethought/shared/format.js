/**
 * format.js — Shared formatting utilities.
 */

export function formatDate(dateStr) {
  if (!dateStr) return "—";
  const d = new Date(dateStr);
  return d.toLocaleDateString("de-DE", { year: "numeric", month: "2-digit", day: "2-digit" });
}

export function formatDateTime(dateStr) {
  if (!dateStr) return "—";
  const d = new Date(dateStr);
  return d.toLocaleDateString("de-DE", {
    year: "numeric", month: "2-digit", day: "2-digit",
    hour: "2-digit", minute: "2-digit",
  });
}

export function formatDuration(seconds) {
  if (seconds == null || seconds === 0) return "—";
  const m = Math.floor(seconds / 60);
  const s = Math.floor(seconds % 60);
  return `${m}:${s.toString().padStart(2, "0")}`;
}

export function formatBPM(bpm) {
  if (bpm == null || bpm === 0) return "—";
  return `${Math.round(bpm)} BPM`;
}

export function formatNumber(n) {
  if (n == null) return "0";
  return n.toLocaleString();
}

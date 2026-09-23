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

/**
 * Placeholder for a value that is genuinely not set (e.g. never played).
 * Distinct from `placeholderUnknown` so the UI no longer uses a universal "—".
 */
export function placeholderUnset(label = "Not set") {
  return `<span class="cell-placeholder cell-placeholder-unset" title="${label}">—</span>`;
}

/**
 * Placeholder for a value that is unknown / could not be determined
 * (e.g. duration unavailable). Rendered as "?" to differ from "not set".
 */
export function placeholderUnknown(label = "Unknown") {
  return `<span class="cell-placeholder cell-placeholder-unknown" title="${label}">?</span>`;
}

/**
 * ui-events.js — fire-and-forget UI telemetry reporting (full-package
 * feature, plan E2/E5).
 *
 * The SPA cannot know whether the server tracks UI events (config flag,
 * pipeline state) — and it must never care: `reportViewOpened` posts to the
 * internal `/api/ui-events` endpoint and **never throws**. The server
 * answers 204 in every case (disabled flag, unknown type, no pipeline) and
 * only ingests when `[telemetry] ui_events_enabled` is on.
 *
 * `keepalive` lets the request survive page unloads (hash navigation keeps
 * the document alive anyway, but an app quit right after a navigation must
 * not lose the event).
 */

/** Send one `ui.view.opened` event. Fire-and-forget; never throws. */
export function reportViewOpened(view) {
  try {
    fetch("/api/ui-events", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        type: "ui.view.opened",
        payload: { view },
      }),
      keepalive: true,
    }).catch(() => {});
  } catch (_err) {
    /* never throw from the telemetry hook */
  }
}

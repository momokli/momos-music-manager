/**
 * api.js — Single source of API configuration and fetch logic.
 */
export const API_BASE = window.location.origin;

/**
 * Fetch JSON from the backend with error handling.
 * @param {string} url - Full URL or path (relative to API_BASE)
 * @param {object} [options] - fetch options
 * @returns {Promise<any>}
 */
export async function fetchJSON(url, options = {}) {
  const fullUrl = url.startsWith("http") ? url : `${API_BASE}${url}`;
  const res = await fetch(fullUrl, {
    headers: { "Content-Type": "application/json", ...options.headers },
    ...options,
  });
  if (!res.ok) {
    let detail = res.statusText;
    try {
      const err = await res.json();
      detail = err.error || err.message || detail;
    } catch {}
    throw new Error(`HTTP ${res.status}: ${detail}`);
  }
  return res.json();
}

/**
 * Quick health check.
 * @returns {Promise<boolean>}
 */
export async function checkBackend() {
  try {
    await fetchJSON("/api/health");
    return true;
  } catch {
    return false;
  }
}

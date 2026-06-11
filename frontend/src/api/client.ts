const API_BASE = "/api";

/**
 * Generic fetch wrapper for the Rust backend API.
 * All responses follow the `{ data: T }` envelope from ApiResponse<T>.
 */
export async function fetchJSON<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(`${API_BASE}${path}`, {
    headers: { "Content-Type": "application/json", ...init?.headers },
    ...init,
  });
  if (!res.ok) {
    const body = await res.text();
    throw new Error(`${res.status}: ${body}`);
  }
  const json = await res.json();
  return json.data as T;
}

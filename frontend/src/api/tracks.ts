import { useQuery } from "@tanstack/react-query";
import { fetchJSON } from "./client";

// ── Types ──────────────────────────────────────────────────────────────

export interface TrackFormatInfo {
  fileType: string;
  local: boolean;
  backup: boolean;
}

export interface PlaylistTagInfo {
  playlistName: string;
  tagName: string;
  category: string;
  prefix: string;
  icon: string;
}

/** A service track from the API. */
export interface ApiServiceTrack {
  id: number;
  service: string;
  serviceId: string;
  title: string;
  artist: string;
  album: string | null;
  isrc: string | null;
  durationMs: number | null;
  metadataJson: string | null;
  importedAt: number;
  updatedAt: number;
  maxAddedAt: number | null;
  localFiles: string[];
  playlistNames: string[];
  playlistTags: PlaylistTagInfo[];
  formatInfo: TrackFormatInfo[];
  inBackpack: boolean;
  bpm: number | null;
  bpmDisplay: string | null;
  musicalKey: string | null;
  rating: number | null;
  playCount: number | null;
  lastPlayed: number | null;
}

export interface TracksResponse {
  tracks: ApiServiceTrack[];
  total: number;
}

export interface TracksQueryParams {
  page?: number;
  pageSize?: number;
  search?: string;
  services?: string;
  bpmMin?: number;
  bpmMax?: number;
  keys?: string;
  sort?: string;
  order?: string;
  tags?: string;
  playlists?: string;
  hasLocal?: boolean;
  hasBackup?: boolean;
  ratingMin?: number;
  playCountMin?: number;
}

// ── Fetch Functions ────────────────────────────────────────────────────

/**
 * Fetch tracks and total count in parallel.
 * GET /api/tracks?... and GET /api/tracks/count?...
 */
async function fetchTracks(params: TracksQueryParams): Promise<TracksResponse> {
  const query = new URLSearchParams();
  const pageSize = params.pageSize ?? 50;
  const page = params.page ?? 1;
  query.set("limit", String(pageSize));
  query.set("offset", String((page - 1) * pageSize));

  if (params.search) query.set("search", params.search);
  if (params.sort) query.set("sort", params.sort);
  if (params.order) query.set("order", params.order);
  if (params.services) query.set("services", params.services);
  if (params.bpmMin && params.bpmMin > 0) query.set("bpmMin", String(params.bpmMin));
  if (params.bpmMax && params.bpmMax < 300) query.set("bpmMax", String(params.bpmMax));
  if (params.keys) query.set("keys", params.keys);
  if (params.tags) query.set("tags", params.tags);
  if (params.playlists) query.set("playlists", params.playlists);
  if (params.hasLocal) query.set("hasLocal", "true");
  if (params.hasBackup) query.set("hasBackup", "true");
  if (params.ratingMin && params.ratingMin > 0) query.set("ratingMin", String(params.ratingMin));
  if (params.playCountMin && params.playCountMin > 0)
    query.set("playCountMin", String(params.playCountMin));

  const qs = query.toString();

  const [tracksResp, countResp] = await Promise.all([
    fetchJSON<ApiServiceTrack[]>(`/tracks${qs ? `?${qs}` : ""}`),
    fetchJSON<number>(`/tracks/count${qs ? `?${qs}` : ""}`),
  ]);

  return { tracks: tracksResp, total: countResp };
}

// ── React Query Hooks ──────────────────────────────────────────────────

/**
 * React Query hook for fetching paginated, searchable, filterable tracks.
 */
export function useTracks(params: TracksQueryParams = {}) {
  return useQuery<TracksResponse>({
    queryKey: ["tracks", params],
    queryFn: () => fetchTracks(params),
    placeholderData: (prev) => prev,
  });
}

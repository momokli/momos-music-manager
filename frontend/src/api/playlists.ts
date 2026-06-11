import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { fetchJSON } from "./client";

// ── Types ──────────────────────────────────────────────────────────────

export interface Playlist {
  id: number;
  service: string;
  playlistId: string;
  name: string;
  description: string | null;
  trackCount: number;
  localTrackCount: number;
  totalTrackCount: number;
  remoteTrackCount: number;
  remoteUniqueCount: number;
  lastFetchedAt: number | null;
  importedAt: number;
  updatedAt: number;
  metadataJson: string | null;
  tagName: string | null;
  archiveDeleted: boolean;
  deemixStatus: string | null;
  deemixId: number | null;
  services: string | null;
}

export interface PlaylistsResponse {
  playlists: Playlist[];
  total: number;
  limit: number;
  offset: number;
}

export interface PlaylistsQueryParams {
  page?: number;
  pageSize?: number;
  search?: string;
  service?: string;
  archive?: string;
}

// ── Fetch Functions ────────────────────────────────────────────────────

/**
 * Fetch paginated playlists from the API.
 * GET /api/playlists?page=1&pageSize=50&search=...&service=...&archive=...
 */
async function fetchPlaylists(
  params: PlaylistsQueryParams,
): Promise<PlaylistsResponse> {
  const query = new URLSearchParams();
  if (params.pageSize) query.set("pageSize", String(params.pageSize));
  if (params.page) query.set("offset", String((params.page - 1) * (params.pageSize ?? 50)));
  if (params.search) query.set("search", params.search);
  if (params.service) query.set("service", params.service);
  if (params.archive) query.set("archive", params.archive);

  const qs = query.toString();
  return fetchJSON<PlaylistsResponse>(`/playlists${qs ? `?${qs}` : ""}`);
}

/**
 * Toggle archive status for a playlist.
 * PUT /api/playlists/{id}/archive
 */
async function toggleArchive(id: number): Promise<void> {
  await fetchJSON<void>(`/playlists/${id}/archive`, { method: "PUT" });
}

// ── Service helpers ────────────────────────────────────────────────────

export const SERVICE_META: Record<string, { icon: string; label: string; cssClass: string }> = {
  spotify: { icon: "fa-brands fa-spotify", label: "Spotify", cssClass: "spotify" },
  soundcloud: { icon: "fa-brands fa-soundcloud", label: "SoundCloud", cssClass: "soundcloud" },
  youtube: { icon: "fa-brands fa-youtube", label: "YouTube", cssClass: "youtube" },
  local: { icon: "fa-solid fa-circle", label: "Local", cssClass: "local" },
  deemix: { icon: "fa-solid fa-download", label: "Deemix", cssClass: "deemix" },
};

export function getServiceMeta(service: string) {
  return (
    SERVICE_META[service] ?? {
      icon: "fa-solid fa-circle",
      label: service.charAt(0).toUpperCase() + service.slice(1),
      cssClass: service,
    }
  );
}

// ── React Query Hooks ──────────────────────────────────────────────────

/**
 * React Query hook for fetching paginated playlists.
 * Pass query params for search, pagination, and filtering.
 */
export function usePlaylists(params: PlaylistsQueryParams = {}) {
  return useQuery<PlaylistsResponse>({
    queryKey: ["playlists", params],
    queryFn: () => fetchPlaylists(params),
  });
}

/**
 * React Query mutation hook for toggling archive status.
 * Invalidates the playlists query cache on success.
 */
export function useToggleArchive() {
  const queryClient = useQueryClient();

  return useMutation<void, Error, number>({
    mutationFn: toggleArchive,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["playlists"] });
    },
  });
}

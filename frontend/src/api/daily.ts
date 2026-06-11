import { useMutation } from "@tanstack/react-query";
import { fetchJSON } from "./client";

// ── Types ──────────────────────────────────────────────────────────────

export interface DailyGenerateRequest {
  tags: string[];
  bpmMin?: number;
  bpmMax?: number;
  limit?: number;
  excludeFullyTagged?: boolean;
}

export interface DailyGenerateResponse {
  playlistId: number;
  playlistName: string;
  trackCount: number;
  spotifyPushStatus: string;
  spotifyUrl?: string;
}

// ── Fetch Functions ────────────────────────────────────────────────────

/**
 * Generate a daily playlist from tag + BPM criteria.
 * POST /api/daily/generate
 */
async function generateDailyPlaylist(
  params: DailyGenerateRequest,
): Promise<DailyGenerateResponse> {
  return fetchJSON<DailyGenerateResponse>("/daily/generate", {
    method: "POST",
    body: JSON.stringify({
      tags: params.tags,
      bpmMin: params.bpmMin,
      bpmMax: params.bpmMax,
      limit: params.limit ?? 20,
      excludeFullyTagged: params.excludeFullyTagged ?? true,
    }),
  });
}

// ── React Query Hooks ──────────────────────────────────────────────────

/**
 * React Query mutation hook for generating a daily playlist.
 * Call `mutate()` with the generation parameters.
 */
export function useDailyGenerate() {
  return useMutation<DailyGenerateResponse, Error, DailyGenerateRequest>({
    mutationFn: generateDailyPlaylist,
  });
}

import { fetchJSON } from "./client";

/** A tag from the API. */
export interface ApiTag {
  id: number;
  name: string;
  category: string;
  categoryIcon: string | null;
  categoryId: number | null;
  fileCount: number;
  createdAt: number;
  backpack: boolean;
}

/** A single suggestion from the digging engine. */
export interface DiggingSuggestion {
  fileId: number;
  title: string;
  artist: string;
  bpm: number | null;
  musicalKey: string | null;
  genre: string | null;
  isrc: string | null;
  filePath: string;
  fileType: string;
  playCount: number;
  score: number;
  camelotCompatibility: string;
  sharedTags: string[];
  bpmDiff: number | null;
  energyLevel: number | null;
}

/** Response from the suggest endpoint. */
export interface DiggingSuggestResponse {
  seeds: unknown[];
  suggestions: DiggingSuggestion[];
  bpmMin: number;
  bpmMax: number;
  candidatesConsidered: number;
}

/**
 * Search tags by name.
 * GET /api/tags?search=<query>&limit=20
 */
export async function searchTags(query: string): Promise<ApiTag[]> {
  const params = new URLSearchParams({
    search: query,
    limit: "20",
  });
  return fetchJSON<ApiTag[]>(`/tags?${params.toString()}`);
}

/**
 * Get digging suggestions for a seed tag.
 * POST /api/digging/suggest
 */
export async function getDiggingSuggestions(
  seedTag: string,
  bpmRange: number,
  limit: number = 10,
): Promise<DiggingSuggestResponse> {
  return fetchJSON<DiggingSuggestResponse>("/digging/suggest", {
    method: "POST",
    body: JSON.stringify({
      seedTag,
      bpmRange,
      limit,
    }),
  });
}

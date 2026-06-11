import { useQuery, useMutation } from "@tanstack/react-query";
import { fetchJSON } from "./client";
import type { ApiTag } from "./tags";

// ── Types ──────────────────────────────────────────────────────────────

export interface BackpackSizeStats {
  tagCount: number;
  trackCount: number;
  localBytes: number;
  targetBytes: number;
  needsPullBytes: number;
}

export interface SyncBackpackResponse {
  taskId: string | null;
  message?: string;
}

export interface TaskProgress {
  id: string;
  taskType: string;
  status: string;
  service: string | null;
  progress: string;
}

// ── Fetch functions ────────────────────────────────────────────────────

async function fetchBackpackTags(): Promise<ApiTag[]> {
  const params = new URLSearchParams({ limit: "500" });
  const tags = await fetchJSON<ApiTag[]>(`/tags?${params.toString()}`);
  return tags.filter((t) => t.backpack);
}

async function fetchBackpackSizeStats(): Promise<BackpackSizeStats> {
  return fetchJSON<BackpackSizeStats>("/storage/backpack-size");
}

async function syncBackpack(): Promise<SyncBackpackResponse> {
  return fetchJSON<SyncBackpackResponse>("/storage/sync-backpack", {
    method: "POST",
  });
}

async function fetchTask(taskId: string): Promise<TaskProgress> {
  return fetchJSON<TaskProgress>(`/tasks/${taskId}`);
}

// ── React Query hooks ──────────────────────────────────────────────────

export function useBackpackTags() {
  return useQuery<ApiTag[]>({
    queryKey: ["backpack", "tags"],
    queryFn: fetchBackpackTags,
    staleTime: 30_000,
  });
}

export function useBackpackSizeStats() {
  return useQuery<BackpackSizeStats>({
    queryKey: ["backpack", "size-stats"],
    queryFn: fetchBackpackSizeStats,
    staleTime: 5_000,
    refetchInterval: 5_000,
  });
}

export function useSyncBackpack() {
  return useMutation<SyncBackpackResponse, Error>({
    mutationFn: syncBackpack,
  });
}

export function useTaskPoll(taskId: string | null) {
  return useQuery<TaskProgress>({
    queryKey: ["task", taskId],
    queryFn: () => fetchTask(taskId!),
    enabled: !!taskId,
    refetchInterval: (query) => {
      const status = query.state.data?.status;
      if (status === "completed" || status === "failed" || status === "cancelled") {
        return false;
      }
      return 2_000;
    },
    staleTime: 1_000,
  });
}

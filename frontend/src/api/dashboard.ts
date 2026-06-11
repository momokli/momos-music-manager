import { useQuery } from "@tanstack/react-query";
import { fetchJSON } from "./client";

export interface ServiceConnection {
  service: string;
  configured: boolean;
  connected: boolean;
  isSyncing: boolean;
  lastSync: number | null;
  playlistsLocal: number;
  tracksLocal: number;
  playlistsRemote: number;
  tracksRemote: number;
  syncCurrentPlaylist: number | null;
  syncCurrentTrack: number | null;
  syncTotalPlaylists: number | null;
  syncTotalTracks: number | null;
  syncLog: string | null;
}

export interface RecentFile {
  id: number;
  title: string;
  artist: string;
  filePath: string;
  fileType: string;
  createdAt?: string;
  lastScanned?: number;
}

export interface DashboardStats {
  files: number;
  tracks: number;
  playlists: number;
  tags: number;
}

/**
 * Fetch the dashboard stats: files count, tracks and playlists via services, tags count.
 */
async function fetchStats(): Promise<DashboardStats> {
  const [filesCount, services, tagsCount] = await Promise.all([
    fetchJSON<number>("/files/count"),
    fetchJSON<ServiceConnection[]>("/services"),
    fetchJSON<number>("/tags/count"),
  ]);

  const tracks = services.reduce((sum, s) => sum + s.tracksLocal, 0);
  const playlists = services.reduce((sum, s) => sum + s.playlistsLocal, 0);

  return {
    files: filesCount,
    tracks,
    playlists,
    tags: tagsCount,
  };
}

/**
 * React Query hook for dashboard aggregate stats.
 */
export function useDashboardStats() {
  return useQuery<DashboardStats>({
    queryKey: ["dashboard", "stats"],
    queryFn: fetchStats,
  });
}

/**
 * Fetch service connections for the service status section.
 */
async function fetchServices(): Promise<ServiceConnection[]> {
  return fetchJSON<ServiceConnection[]>("/services");
}

/**
 * React Query hook for service connection statuses.
 */
export function useServices() {
  return useQuery<ServiceConnection[]>({
    queryKey: ["dashboard", "services"],
    queryFn: fetchServices,
  });
}

/**
 * Fetch recent files for the recent activity section.
 */
async function fetchRecentFiles(): Promise<RecentFile[]> {
  return fetchJSON<RecentFile[]>("/files/latest");
}

/**
 * React Query hook for recent files.
 */
export function useRecentFiles() {
  return useQuery<RecentFile[]>({
    queryKey: ["dashboard", "recent"],
    queryFn: fetchRecentFiles,
  });
}

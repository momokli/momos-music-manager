import { useQuery } from "@tanstack/react-query";
import { fetchJSON } from "./client";
import type { ServiceConnection } from "./dashboard";

/* ── Types ─────────────────────────────────────────────────────── */

export interface FolderInfo {
  id: number;
  path: string;
  watchEnabled: boolean;
  scanRecursive: boolean;
  fixedExtensions: boolean;
  fileExtensions: string;
  maxDepth: number;
  fileCount: number;
  lastScanned: number | null;
  backupPath: string | null;
  scanSources: boolean;
  autoBackup: boolean;
}

export interface StorageStatus {
  localFileCount: number;
  trackedFileCount: number;
  localSizeBytes: number;
  trackedSizeBytes: number;
  localStems: number;
  localFlacs: number;
  localMp3s: number;
  localWavs: number;
  localOther: number;
  localStemsSize: number;
  localFlacsSize: number;
  localWavsSize: number;
  localMp3sSize: number;
  backupCount: number;
  wavSourceDirs: number;
  pruneCandidateCount: number;
  pruneCandidateBytes: number;
  wavIndexed: number;
  wavBackedUp: number;
}

export interface TaskProgress {
  id: string;
  task_type: string;
  status: string;
  service: string | null;
  progress: string;
  percent: number | null;
  sub_items: unknown[];
  logs: string[];
  created_at_secs: number;
  task_details: unknown | null;
}

export interface TasksResponse {
  tasks: TaskProgress[];
  total: number;
  limit: number;
  offset: number;
}

export interface TraktorStatus {
  path: string | null;
  modifiedAt: number | null;
}

export interface DeemixQueueItem {
  id: number;
  trackId: string;
  title: string;
  artist: string;
  status: string;
  createdAt: string;
}

export interface DeemixQueueResponse {
  items: DeemixQueueItem[];
  total: number;
}

/* ── Fetch Functions ─────────────────────────────────────────────── */

async function fetchServices(): Promise<ServiceConnection[]> {
  return fetchJSON<ServiceConnection[]>("/services");
}

async function fetchFolders(): Promise<FolderInfo[]> {
  return fetchJSON<FolderInfo[]>("/folders");
}

async function fetchStorageStatus(): Promise<StorageStatus> {
  return fetchJSON<StorageStatus>("/storage/status");
}

async function fetchTasks(): Promise<TasksResponse> {
  return fetchJSON<TasksResponse>("/tasks?limit=10");
}

async function fetchTraktorStatus(): Promise<TraktorStatus> {
  return fetchJSON<TraktorStatus>("/traktor/status");
}

async function fetchDeemixQueue(): Promise<DeemixQueueResponse> {
  return fetchJSON<DeemixQueueResponse>("/services/deemix/queue");
}

/* ── React Query Hooks ───────────────────────────────────────────── */

export function useSetupServices() {
  return useQuery<ServiceConnection[]>({
    queryKey: ["setup", "services"],
    queryFn: fetchServices,
  });
}

export function useSetupFolders() {
  return useQuery<FolderInfo[]>({
    queryKey: ["setup", "folders"],
    queryFn: fetchFolders,
  });
}

export function useSetupStorageStatus() {
  return useQuery<StorageStatus>({
    queryKey: ["setup", "storage"],
    queryFn: fetchStorageStatus,
  });
}

export function useSetupTasks() {
  return useQuery<TasksResponse>({
    queryKey: ["setup", "tasks"],
    queryFn: fetchTasks,
  });
}

export function useSetupTraktorStatus() {
  return useQuery<TraktorStatus>({
    queryKey: ["setup", "traktor"],
    queryFn: fetchTraktorStatus,
  });
}

export function useSetupDeemixQueue() {
  return useQuery<DeemixQueueResponse>({
    queryKey: ["setup", "deemix"],
    queryFn: fetchDeemixQueue,
  });
}

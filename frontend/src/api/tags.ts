import { useQuery } from "@tanstack/react-query";
import { fetchJSON } from "./client";

// ── Types ──────────────────────────────────────────────────────────────

export interface TagCategory {
  id: number;
  name: string;
  prefix: string;
  icon: string;
  sortOrder: number;
  isDefault: boolean;
  tagCount: number;
  createdAt: number;
}

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

export interface TagWithEnergy {
  tag_id: number;
  tag_name: string;
  category_name: string;
  energy_level: number | null;
  sort_order: number;
}

export interface BundleTag {
  id: number;
  name: string;
  categoryId: number | null;
  categoryName: string | null;
  memberCount: number;
  backpack: boolean;
}

export interface DynamicBundle {
  id: number;
  name: string;
  tagId: number;
  tag_name: string;
  tag_backpack: boolean;
  matching_file_count: number;
  baseTags: string | null;
  includeAllTracks: boolean;
  bpmMin: number | null;
  bpmMax: number | null;
  pmvCategories: string | null;
  fileTypes: string | null;
  excludeWavSources: boolean;
  keys: string | null;
  ratingMin: number | null;
  playCountMin: number | null;
  createdAt: number;
  updatedAt: number;
}

// ── Fetch functions ────────────────────────────────────────────────────

async function fetchTagCategories(): Promise<TagCategory[]> {
  return fetchJSON<TagCategory[]>("/tag-categories");
}

async function fetchTagEnergyLevels(): Promise<TagWithEnergy[]> {
  return fetchJSON<TagWithEnergy[]>("/tag-energy-levels");
}

async function fetchTagsByCategory(categoryName: string): Promise<ApiTag[]> {
  const params = new URLSearchParams({ category: categoryName, limit: "500" });
  return fetchJSON<ApiTag[]>(`/tags?${params.toString()}`);
}

async function fetchTagBundles(): Promise<BundleTag[]> {
  return fetchJSON<BundleTag[]>("/tags/bundles");
}

async function fetchDynamicBundles(): Promise<DynamicBundle[]> {
  return fetchJSON<DynamicBundle[]>("/dynamic-bundles");
}

// ── React Query hooks ──────────────────────────────────────────────────

export function useTagCategories() {
  return useQuery<TagCategory[]>({
    queryKey: ["tag-categories"],
    queryFn: fetchTagCategories,
    staleTime: 60_000,
  });
}

export function useTagEnergyLevels() {
  return useQuery<TagWithEnergy[]>({
    queryKey: ["tag-energy-levels"],
    queryFn: fetchTagEnergyLevels,
    staleTime: 30_000,
  });
}

export function useTagsByCategory(categoryName: string | null) {
  return useQuery<ApiTag[]>({
    queryKey: ["tags", "category", categoryName],
    queryFn: () => fetchTagsByCategory(categoryName!),
    enabled: !!categoryName,
    staleTime: 30_000,
  });
}

export function useTagBundles() {
  return useQuery<BundleTag[]>({
    queryKey: ["tag-bundles"],
    queryFn: fetchTagBundles,
    staleTime: 30_000,
  });
}

export function useDynamicBundles() {
  return useQuery<DynamicBundle[]>({
    queryKey: ["dynamic-bundles"],
    queryFn: fetchDynamicBundles,
    staleTime: 30_000,
  });
}

import { create } from "zustand";
import * as api from "../lib/tauri";

export interface BrollVideo {
  id: string;
  width: number;
  height: number;
  url: string;
}

export interface BrollResult {
  concept: string;
  videos: BrollVideo[];
  cached_path?: string;
}

export interface MusicResult {
  title: string;
  artist: string;
  path: string;
  duration_ms: number;
  mood: string;
  energy: string;
}

export interface SFXResult {
  id: string;
  filename: string;
  path: string;
  category: string;
  editorial_role: string;
  duration_ms: number;
}

export interface AssetState {
  brollResults: BrollResult[];
  musicResults: MusicResult[];
  sfxResults: SFXResult[];
  isSearching: boolean;

  searchBroll: (concepts: string[], download?: boolean) => Promise<void>;
  searchMusic: (mood?: string, energy?: string) => Promise<void>;
  searchSFX: (query?: string, role?: string) => Promise<void>;
  assignBroll: (concept: string, positionMs: number, durationMs: number) => Promise<void>;
  assignMusic: (mood: string, energy: string) => Promise<void>;
  assignSFX: (role: string, positionMs: number) => Promise<void>;
}

export const useAssetStore = create<AssetState>((set) => ({
  brollResults: [],
  musicResults: [],
  sfxResults: [],
  isSearching: false,

  searchBroll: async (concepts: string[], download?: boolean) => {
    set({ isSearching: true });
    try {
      const results = await api.brollFetch(concepts, download);
      set({ brollResults: results as any, isSearching: false });
    } catch {
      set({ isSearching: false });
    }
  },

  searchMusic: async (mood?: string, energy?: string) => {
    set({ isSearching: true });
    try {
      const results = await api.musicSearch(mood, energy);
      set({ musicResults: results as any, isSearching: false });
    } catch {
      set({ isSearching: false });
    }
  },

  searchSFX: async (query?: string, role?: string) => {
    set({ isSearching: true });
    try {
      const results = await api.sfxSearch(query, role);
      set({ sfxResults: results as any, isSearching: false });
    } catch {
      set({ isSearching: false });
    }
  },

  assignBroll: async (concept: string, positionMs: number, durationMs: number) => {
    try {
      await api.brollAssign(concept, positionMs, durationMs);
    } catch {
      // Assignment errors are handled by the calling component
    }
  },

  assignMusic: async (mood: string, energy: string) => {
    try {
      await api.musicAssign(mood, energy);
    } catch {
      // Assignment errors are handled by the calling component
    }
  },

  assignSFX: async (role: string, positionMs: number) => {
    try {
      await api.sfxAssign(role, positionMs);
    } catch {
      // Assignment errors are handled by the calling component
    }
  },
}));

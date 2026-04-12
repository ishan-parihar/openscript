import { create } from "zustand";
import * as api from "../lib/tauri";

export interface BrollResult {
  concept: string;
  matched_concept: string | null;
  videos: api.BrollVideoItem[];
}

export interface MusicResult {
  id: string;
  title: string;
  artist: string;
  path: string;
  duration_ms: number;
  mood: string;
  energy: string;
  bpm?: number;
  loopability?: boolean;
  intro_friendly?: boolean;
  cta_friendly?: boolean;
  loudness_target_lufs?: number;
  tags?: string[];
  genre?: string;
}

export interface SFXResult {
  id: string;
  filename: string;
  path: string;
  category: string;
  subcategory: string;
  editorial_role: string;
  duration_ms: number;
  sample_rate?: number;
  peak_db?: number;
  loudness_lufs?: number;
  recommended_gain_db?: number;
  recommended_use?: string;
  safe_overlay?: boolean;
  tags?: string[];
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
      // broll_fetch returns raw array: [{ concept, matched_concept, videos: [...] }]
      set({ brollResults: results as BrollResult[], isSearching: false });
    } catch {
      set({ isSearching: false });
    }
  },

  searchMusic: async (mood?: string, energy?: string) => {
    set({ isSearching: true });
    try {
      const response = await api.musicSearch(mood, energy);
      // music_search returns { total, tracks: [...] } — extract tracks
      set({ musicResults: response.tracks as MusicResult[], isSearching: false });
    } catch {
      set({ isSearching: false });
    }
  },

  searchSFX: async (query?: string, role?: string) => {
    set({ isSearching: true });
    try {
      const response = await api.sfxSearch(query, role);
      // sfx_search returns { total, sfx: [...] } — extract sfx
      set({ sfxResults: response.sfx as SFXResult[], isSearching: false });
    } catch {
      set({ isSearching: false });
    }
  },

  assignBroll: async (concept: string, positionMs: number, durationMs: number) => {
    await api.brollAssign(concept, positionMs, durationMs).catch(() => {});
  },

  assignMusic: async (mood: string, energy: string) => {
    await api.musicAssign(mood, energy).catch(() => {});
  },

  assignSFX: async (role: string, positionMs: number) => {
    await api.sfxAssign(role, positionMs).catch(() => {});
  },
}));

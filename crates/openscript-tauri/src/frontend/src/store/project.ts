import { create } from "zustand";
import * as api from "../lib/tauri";

export interface Segment {
  id: string;
  source_start_ms: number;
  source_end_ms: number;
  caption: string;
  semantic_role?: string;
  crossfade_ms?: number;
}

export interface ProjectState {
  projectId: string | null;
  projectName: string;
  sourceVideo: string | null;
  segments: Segment[];
  isLoading: boolean;
  error: string | null;

  createProject: (sourceVideo: string) => Promise<void>;
  loadProject: (projectId: string) => Promise<void>;
  refreshTimeline: () => Promise<void>;
  addSegment: (start: number, end: number, caption: string, role?: string) => Promise<void>;
  undo: () => Promise<void>;
  redo: () => Promise<void>;
  save: () => Promise<void>;
}

export const useProjectStore = create<ProjectState>((set, get) => ({
  projectId: null,
  projectName: "Untitled",
  sourceVideo: null,
  segments: [],
  isLoading: false,
  error: null,

  createProject: async (sourceVideo: string) => {
    set({ isLoading: true, error: null });
    try {
      const result = await api.createProject(sourceVideo);
      set({
        projectId: result.project_id,
        projectName: result.name,
        sourceVideo,
        segments: [],
        isLoading: false,
      });
    } catch (e) {
      set({ error: String(e), isLoading: false });
    }
  },

  loadProject: async (projectId: string) => {
    set({ isLoading: true, error: null });
    try {
      await api.loadProject(projectId);
      await get().refreshTimeline();
      set({ projectId, isLoading: false });
    } catch (e) {
      set({ error: String(e), isLoading: false });
    }
  },

  refreshTimeline: async () => {
    try {
      const data = await api.getTimeline();
      const timeline = data as any;
      set({
        projectName: timeline.name || "Untitled",
        sourceVideo: timeline.source_video,
        segments: timeline.timeline?.segments || [],
      });
    } catch (e) {
      console.error("Failed to refresh timeline:", e);
    }
  },

  addSegment: async (start, end, caption, role) => {
    try {
      await api.addSegment(start, end, caption, role);
      await get().refreshTimeline();
    } catch (e) {
      set({ error: String(e) });
    }
  },

  undo: async () => {
    try {
      await api.undoAction();
      await get().refreshTimeline();
    } catch (e) {
      set({ error: String(e) });
    }
  },

  redo: async () => {
    try {
      await api.redoAction();
      await get().refreshTimeline();
    } catch (e) {
      set({ error: String(e) });
    }
  },

  save: async () => {
    try {
      await api.saveProject();
    } catch (e) {
      set({ error: String(e) });
    }
  },
}));

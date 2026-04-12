import { create } from "zustand";

interface EditorState {
  playbackPosition: number;
  isPlaying: boolean;
  zoom: number;
  selectedSegmentId: string | null;
  selectedTrack: string | null;
  activePanel: "transcript" | "timeline" | "assets" | "ai" | "voice" | "render";

  setPlaybackPosition: (position: number) => void;
  setIsPlaying: (playing: boolean) => void;
  setZoom: (zoom: number) => void;
  setSelectedSegmentId: (id: string | null) => void;
  setSelectedTrack: (track: string | null) => void;
  setActivePanel: (panel: "transcript" | "timeline" | "assets" | "ai" | "voice" | "render") => void;
}

export const useEditorStore = create<EditorState>((set) => ({
  playbackPosition: 0,
  isPlaying: false,
  zoom: 100,
  selectedSegmentId: null,
  selectedTrack: null,
  activePanel: "transcript",

  setPlaybackPosition: (position) => set({ playbackPosition: position }),
  setIsPlaying: (playing) => set({ isPlaying: playing }),
  setZoom: (zoom) => set({ zoom }),
  setSelectedSegmentId: (id) => set({ selectedSegmentId: id }),
  setSelectedTrack: (track) => set({ selectedTrack: track }),
  setActivePanel: (panel) => set({ activePanel: panel }),
}));

import { create } from "zustand";
import * as api from "../lib/tauri";

export interface VoiceProfile {
  id: string;
  name: string;
  language: string;
}

export interface VoiceState {
  profiles: VoiceProfile[];
  selectedProfileId: string | null;
  text: string;
  isGenerating: boolean;
  generatedAudioPath: string | null;
  estimatedDurationMs: number | null;

  loadProfiles: () => Promise<void>;
  setText: (text: string) => void;
  setSelectedProfile: (id: string | null) => void;
  generate: () => Promise<void>;
  estimateDuration: () => Promise<void>;
}

export const useVoiceStore = create<VoiceState>((set, get) => ({
  profiles: [],
  selectedProfileId: null,
  text: "",
  isGenerating: false,
  generatedAudioPath: null,
  estimatedDurationMs: null,

  loadProfiles: async () => {
    try {
      const response = await api.voiceProfileList();
      set({ profiles: response.profiles as VoiceProfile[] });
    } catch {
      set({ profiles: [] });
    }
  },

  setText: (text: string) => set({ text }),

  setSelectedProfile: (id: string | null) => set({ selectedProfileId: id }),

  generate: async () => {
    const { text, selectedProfileId } = get();
    if (!text) return;
    set({ isGenerating: true });
    try {
      const result = await api.ttsGenerate(text, selectedProfileId ?? undefined);
      set({ generatedAudioPath: result.output_path, isGenerating: false });
    } catch {
      set({ isGenerating: false });
    }
  },

  estimateDuration: async () => {
    const { text, selectedProfileId } = get();
    if (!text) return;
    try {
      const result = await api.ttsEstimateDuration(text, selectedProfileId ?? undefined);
      set({ estimatedDurationMs: result.estimated_duration_ms });
    } catch {
    }
  },
}));

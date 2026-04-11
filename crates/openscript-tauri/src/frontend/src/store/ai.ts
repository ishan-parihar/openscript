import { create } from "zustand";
import * as api from "../lib/tauri";
import { useProjectStore } from "./project";

export interface ChatMessage {
  id: string;
  role: "user" | "assistant";
  content: string;
  timestamp: number;
}

interface AIState {
  messages: ChatMessage[];
  isProcessing: boolean;
  suggestions: string[];

  sendMessage: (content: string) => void;
  runReelize: (videoPath: string) => Promise<void>;
  clear: () => void;
}

const DEFAULT_SUGGESTIONS = [
  "Create a 30s reel from this video",
  "Add b-roll every 5 seconds",
  "Suggest background music",
  "Generate intro voiceover",
];

export const useAIStore = create<AIState>((set, get) => ({
  messages: [],
  isProcessing: false,
  suggestions: DEFAULT_SUGGESTIONS,

  sendMessage: (content: string) => {
    const userMsg: ChatMessage = {
      id: crypto.randomUUID(),
      role: "user",
      content,
      timestamp: Date.now(),
    };

    set({ messages: [...get().messages, userMsg], isProcessing: true });

    setTimeout(() => {
      const assistantMsg: ChatMessage = {
        id: crypto.randomUUID(),
        role: "assistant",
        content: `Got it — "${content}". I'll work on that for you.`,
        timestamp: Date.now(),
      };
      set({ messages: [...get().messages, assistantMsg], isProcessing: false });
    }, 1000);
  },

  runReelize: async (videoPath: string) => {
    set({ isProcessing: true });
    try {
      const result = await api.reelizeTimeline(videoPath);
      const assistantMsg: ChatMessage = {
        id: crypto.randomUUID(),
        role: "assistant",
        content: `Reel created! ${result.segments_count} segments, ${result.tracks_rendered} tracks rendered. Output: ${result.output_path}`,
        timestamp: Date.now(),
      };
      set({ messages: [...get().messages, assistantMsg], isProcessing: false });
      await useProjectStore.getState().refreshTimeline();
    } catch (e) {
      const errorMsg: ChatMessage = {
        id: crypto.randomUUID(),
        role: "assistant",
        content: `Error: ${String(e)}`,
        timestamp: Date.now(),
      };
      set({ messages: [...get().messages, errorMsg], isProcessing: false });
    }
  },

  clear: () => set({ messages: [], isProcessing: false }),
}));

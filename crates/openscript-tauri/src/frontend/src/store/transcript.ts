import { create } from "zustand";
import * as api from "../lib/tauri";

export interface TranscriptEntry {
  index: number;
  start: number;
  end: number;
  text: string;
}

export interface FillerAnalysis {
  filler_word_count: number;
  total_words: number;
  filler_percentage: number;
  filler_words: string[];
}

interface TranscriptState {
  entries: TranscriptEntry[];
  isTranscribing: boolean;
  transcriptionProgress: number;
  fillerAnalysis: FillerAnalysis | null;
  isEditing: boolean;

  transcribe: (videoPath: string) => Promise<void>;
  loadTranscript: (srtPath: string) => Promise<void>;
  prepareTranscript: (wordSrtPath: string, maxWords?: number, maxChars?: number) => Promise<void>;
  analyzeFillerWords: (srtPath: string) => Promise<void>;
  removeFillerWords: () => Promise<void>;
  applyEdit: (videoPath: string, segments: unknown[]) => Promise<unknown>;
}

export const useTranscriptStore = create<TranscriptState>((set, get) => ({
  entries: [],
  isTranscribing: false,
  transcriptionProgress: 0,
  fillerAnalysis: null,
  isEditing: false,

  transcribe: async (videoPath: string) => {
    set({ isTranscribing: true, transcriptionProgress: 0 });
    try {
      await api.transcribeVideo(videoPath);
      set({ isTranscribing: false, transcriptionProgress: 100 });
    } catch (e) {
      set({ isTranscribing: false, transcriptionProgress: 0 });
      throw e;
    }
  },

  loadTranscript: async (srtPath: string) => {
    const result = await api.readTranscript(srtPath);
    const data = result as { count: number; entries: TranscriptEntry[] };
    set({ entries: data.entries ?? [] });
  },

  prepareTranscript: async (wordSrtPath: string, maxWords?: number, maxChars?: number) => {
    await api.prepareTranscript(wordSrtPath, maxWords, maxChars);
    await get().loadTranscript(wordSrtPath.replace("word", "phrase"));
  },

  analyzeFillerWords: async (srtPath: string) => {
    const result = await api.analyzeTranscript(srtPath);
    const data = result as FillerAnalysis;
    set({ fillerAnalysis: data });
  },

  removeFillerWords: async () => {
    const { entries } = get();
    if (!entries.length) return;

    const cleaned: TranscriptEntry[] = [];
    for (const entry of entries) {
      const result = await api.removeFillerWordsFromText(entry.text);
      const data = result as { cleaned_text: string; removed_count: number };
      cleaned.push({ ...entry, text: data.cleaned_text });
    }
    set({ entries: cleaned });
  },

  applyEdit: async (videoPath: string, segments: unknown[]) => {
    return api.applyTranscriptEdit(videoPath, segments);
  },
}));

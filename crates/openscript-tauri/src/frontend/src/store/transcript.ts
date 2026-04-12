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
  wordSrtPath: string | null;
  phraseSrtPath: string | null;

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
  wordSrtPath: null,
  phraseSrtPath: null,

  transcribe: async (videoPath: string) => {
    set({ isTranscribing: true, transcriptionProgress: 0 });
    try {
      const result = await api.transcribeVideo(videoPath);
      const data = result as { srt_path: string; word_srt_path: string; phrase_srt_path: string; entry_count: number };
      set({
        isTranscribing: false,
        transcriptionProgress: 100,
        wordSrtPath: data.word_srt_path ?? null,
        phraseSrtPath: data.phrase_srt_path ?? null,
      });
    } catch (e) {
      set({ isTranscribing: false, transcriptionProgress: 0 });
      throw e;
    }
  },

  loadTranscript: async (srtPath: string) => {
    const result = await api.readTranscript(srtPath);
    const data = result as { count: number; entries?: TranscriptEntry[]; segments?: TranscriptEntry[] };
    set({ entries: data.segments ?? data.entries ?? [] });
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

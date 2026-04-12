export type PipelineStage =
  | "idle"
  | "transcribing"
  | "transcribed"
  | "analyzing"
  | "editing"
  | "building-timeline"
  | "assigning-assets"
  | "generating-tts"
  | "rendering"
  | "rendered"
  | "verifying"
  | "complete"
  | "error";

export interface PipelineState {
  stage: PipelineStage;
  progress: number;
  error: string | null;
  output: string | null;
  startedAt: number | null;
  completedAt: number | null;
}

export interface WordEntry {
  index: number;
  text: string;
  start: number; // seconds
  end: number; // seconds
  included: boolean; // true = keep in video, false = clip out
  isFiller: boolean;
  segmentIndex: number;
}

export interface TranscriptSegment {
  index: number;
  start: number;
  end: number;
  text: string;
  words: WordEntry[];
}

export interface RenderOptions {
  outputPath?: string;
  quality: "preview" | "standard" | "high";
  includeCaptions: boolean;
  includeMusic: boolean;
  includeSfx: boolean;
  includeBroll: boolean;
  aspectRatio: "9:16" | "16:9" | "1:1";
}

export interface VoiceProfile {
  id: string;
  name: string;
  language: string;
}

export interface TTSRequest {
  text: string;
  voiceProfileId?: string;
  outputPath?: string;
}

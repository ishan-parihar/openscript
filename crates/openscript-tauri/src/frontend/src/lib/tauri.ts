import { invoke } from "@tauri-apps/api/core";

// Project commands
export async function createProject(sourceVideo: string) {
  return invoke<{ project_id: string; name: string; timeline_path: string }>(
    "create_project",
    { sourceVideo }
  );
}

export async function loadProject(projectId: string) {
  return invoke("load_project", { projectId });
}

export async function listProjects() {
  return invoke<
    Array<{ id: string; name: string; source_video: string; active: boolean }>
  >("list_projects");
}

export async function saveProject() {
  return invoke<{ saved: boolean; path: string }>("save_project");
}

// Timeline commands
export async function addSegment(
  start: number,
  end: number,
  caption: string,
  semanticRole?: string
) {
  return invoke<{ segment_id: string }>("add_segment", {
    start,
    end,
    caption,
    semanticRole,
  });
}

export async function getTimeline() {
  return invoke("get_timeline");
}

export async function timelinePreview() {
  return invoke<{
    total_duration_ms: number;
    segment_count: number;
    render_ready: boolean;
  }>("timeline_preview");
}

export async function undoAction() {
  return invoke<{ undone: string }>("undo");
}

export async function redoAction() {
  return invoke<{ redone: string }>("redo");
}

// Transcript commands
export interface TranscriptEntry {
  index: number;
  start: number;
  end: number;
  text: string;
}

export async function transcribeVideo(videoPath: string) {
  return invoke<{ srt_path: string; phrase_srt_path: string; word_srt_path: string; entry_count: number }>(
    "transcribe_video",
    { videoPath }
  );
}

export async function readTranscript(srtPath: string) {
  return invoke<{ count: number; entries: TranscriptEntry[] }>(
    "read_transcript",
    { srtPath }
  );
}

export async function prepareTranscript(wordSrtPath: string, maxWords?: number, maxChars?: number) {
  return invoke<{ output_path: string; count: number }>(
    "prepare_transcript",
    { wordSrtPath, maxWords: maxWords ?? 10, maxChars: maxChars ?? 64 }
  );
}

export async function analyzeTranscript(srtPath: string) {
  return invoke<{ filler_word_count: number; total_words: number; filler_percentage: number; filler_words: string[] }>(
    "analyze_transcript",
    { srtPath }
  );
}

export async function removeFillerWordsFromText(text: string) {
  return invoke<{ cleaned_text: string; removed_count: number }>(
    "remove_filler_words_from_text",
    { text }
  );
}

export async function applyTranscriptEdit(videoPath: string, segments: unknown[]) {
  return invoke<{ output_path: string; segments_count: number; total_duration_s: number }>(
    "apply_transcript_edit",
    { videoPath, segments }
  );
}

// Asset commands
export async function brollFetch(concepts: string[], download?: boolean) {
  return invoke("broll_fetch", { concepts, download: download ?? false });
}

export async function brollAssign(concept: string, positionMs: number, durationMs: number) {
  return invoke("broll_assign", { concept, positionMs, durationMs });
}

export async function musicSearch(mood?: string, energy?: string) {
  return invoke("music_search", { mood, energy });
}

export async function musicAssign(mood: string, energy: string) {
  return invoke("music_assign", { mood, energy });
}

export async function sfxSearch(query?: string, role?: string) {
  return invoke("sfx_search", { query, role });
}

export async function sfxAssign(role: string, positionMs: number) {
  return invoke("sfx_assign", { role, positionMs });
}

// Reelize pipeline
export async function reelizeTimeline(videoPath: string) {
  return invoke<{ output_path: string; file_size_bytes: number; timeline_path: string; segments_count: number; tracks_rendered: number }>(
    "reelize_timeline",
    { videoPath }
  );
}

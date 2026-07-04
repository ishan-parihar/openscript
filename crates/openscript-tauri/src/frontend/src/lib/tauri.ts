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

export async function applyTranscriptEdit(
  videoPath: string,
  editedSegments: unknown[],
  outputPath?: string,
) {
  // Auto-generate an output path if the caller did not supply one. Prior
  // versions sent { segments } which the Rust backend rejected because it
  // expects { edited_segments, output_path }. This is a CRITICAL bug fix.
  const resolvedOutputPath = outputPath
    ?? `${videoPath.replace(/\.[^.]+$/, "")}.edited.mp4`;
  return invoke<{ output_path: string; segments_count: number; total_duration_s: number }>(
    "apply_transcript_edit",
    { videoPath, editedSegments, outputPath: resolvedOutputPath }
  );
}

// Asset commands
export interface BrollVideoItem {
  id: string;
  width: number;
  height: number;
  url: string;
  image: string;
  cached_path?: string;
}

export interface BrollConceptResult {
  concept: string;
  matched_concept: string | null;
  videos: BrollVideoItem[];
}

export interface MusicTrackItem {
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

export interface MusicSearchResponse {
  total: number;
  tracks: MusicTrackItem[];
}

export interface SFXItem {
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

export interface SFXSearchResponse {
  total: number;
  sfx: SFXItem[];
}

export async function brollFetch(concepts: string[], download?: boolean): Promise<BrollConceptResult[]> {
  return invoke("broll_fetch", { concepts, download: download ?? false });
}

export async function brollAssign(concept: string, positionMs: number, durationMs: number) {
  return invoke("broll_assign", { concept, positionMs, durationMs });
}

export async function musicSearch(mood?: string, energy?: string): Promise<MusicSearchResponse> {
  return invoke("music_search", { mood, energy });
}

export async function musicAssign(mood: string, energy: string) {
  return invoke("music_assign", { mood, energy });
}

export async function sfxSearch(query?: string, role?: string): Promise<SFXSearchResponse> {
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

// TTS commands
export async function voiceProfileList() {
  return invoke<{ profiles: Array<{ id: string; name: string; language: string }> }>(
    "voice_profile_list"
  );
}

export async function voiceProfileAdd(name: string, language: string, audioFilePath: string) {
  return invoke<{ profile_id: string }>(
    "voice_profile_add",
    { name, language, audioFilePath }
  );
}

export async function voiceProfileRemove(profileId: string) {
  return invoke<{ removed: boolean }>(
    "voice_profile_remove",
    { profileId }
  );
}

export async function ttsGenerate(text: string, voiceProfileId?: string, outputPath?: string) {
  return invoke<{ output_path: string; duration_ms: number }>(
    "tts_generate",
    { text, voiceProfileId, outputPath }
  );
}

export async function ttsEstimateDuration(text: string, voiceProfileId?: string) {
  return invoke<{ estimated_duration_ms: number }>(
    "tts_estimate_duration",
    { text, voiceProfileId }
  );
}

// Render commands
export interface RenderResult {
  output_path: string | null;
  file_size_bytes: number;
  duration_ms?: number;
  status: "completed" | "cancelled";
}
export async function renderTimeline(options?: { outputPath?: string; quality?: string }) {
  return invoke<RenderResult>(
    "render_timeline",
    { outputPath: options?.outputPath, quality: options?.quality }
  );
}

export async function getRenderProgress() {
  return invoke<{ progress: number; status: string; eta_seconds?: number }>(
    "get_render_progress"
  );
}

export async function cancelRender() {
  return invoke<{ cancelled: boolean }>("cancel_render");
}

// Verification commands
export async function verifyAudio(filePath: string) {
  return invoke<{ passed: boolean; issues: string[]; loudness_lufs: number }>(
    "verify_audio",
    { filePath }
  );
}

export async function verifyCaptions(filePath: string) {
  return invoke<{ passed: boolean; issues: string[]; caption_count: number }>(
    "verify_captions",
    { filePath }
  );
}

// Timeline validation
export async function validateTimeline() {
  return invoke<{ valid: boolean; issues: string[]; segment_count: number }>(
    "validate_timeline"
  );
}

// Segment manipulation
export async function removeSegment(segmentId: string) {
  return invoke<{ removed: boolean }>("remove_segment", { segmentId });
}

export async function updateSegment(segmentId: string, start: number, end: number, caption: string) {
  return invoke<{ updated: boolean }>(
    "update_segment",
    { segmentId, start, end, caption }
  );
}

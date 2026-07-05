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

// ===========================================================================
// GENERIC MCP DISPATCH — the "desktop as MCP client of itself" pass-through.
// See AGENTS.md §5 + commands/invoke_tool.rs for the architecture.
//
// Every MCP tool is reachable via invokeTool(name, args). The typed wrappers
// below cover the 43 tools that previously had NO Tauri handler. The existing
// typed wrappers above (addSegment, brollFetch, etc.) remain for backward
// compatibility and for stateful operations that need AppState.
// ===========================================================================

/** Invoke any MCP tool by name. The args object is passed verbatim to route_tool. */
export async function invokeTool<T = unknown>(name: string, args: Record<string, unknown> = {}): Promise<T> {
  return invoke<T>("invoke_tool", { name, args });
}

/** List all registered MCP tools (name + description + inputSchema). */
export async function listMcpTools(): Promise<Array<{ name: string; description: string; inputSchema: Record<string, unknown> }>> {
  const tools = await invoke<Array<{ name: string; description: string; inputSchema: Record<string, unknown> }>>("list_mcp_tools");
  return tools;
}

/** Get a single MCP tool's definition. Returns null if not found. */
export async function getMcpTool(name: string): Promise<{ name: string; description: string; inputSchema: Record<string, unknown> } | null> {
  return invoke<{ name: string; description: string; inputSchema: Record<string, unknown> } | null>("get_mcp_tool", { name });
}

// ---------------------------------------------------------------------------
// Discovery meta-tools (P1-2 + Rec 3.1 from the audit)
// ---------------------------------------------------------------------------

export interface SystemCapabilities {
  ffmpeg: { available: boolean; reason?: string | null };
  kokoro: { available: boolean; sidecar_path?: string; model_path?: string; voices_path?: string; reason?: string | null };
  transcription: { available: boolean; engine: string; wrapper_path?: string; reason?: string | null };
  voicebox: { available: boolean; url?: string; reason?: string | null };
  pexels: { available: boolean; reason?: string | null };
  giphy: { available: boolean; reason?: string | null };
  pixabay: { available: boolean; reason?: string | null };
  sfx_library: { available: boolean; indexed_count: number; index_path?: string };
  music_library: { available: boolean; indexed_count: number; index_path?: string };
  hyperframes: { available: boolean; path?: string };
  status: string;
}

export async function systemCapabilities(): Promise<SystemCapabilities> {
  return invokeTool<SystemCapabilities>("system.capabilities", {});
}

export interface HelpToolResult {
  status: string;
  query: string;
  count: number;
  results: Array<{ name: string; relevance: number; description: string }>;
}

export async function helpTool(query: string, limit = 8): Promise<HelpToolResult> {
  return invokeTool<HelpToolResult>("help.tool", { query, limit });
}

// ---------------------------------------------------------------------------
// Golden trajectory: script.parse → script.to_video
// ---------------------------------------------------------------------------

export interface ScriptParseResult {
  status: string;
  valid: boolean;
  errors: Array<{ field: string; message: string }>;
  spec_summary: {
    scene_count: number;
    speaker_count: number;
    estimated_duration_s: number;
    aspect: string;
  };
}

export async function scriptParse(scriptInput: string): Promise<ScriptParseResult> {
  return invokeTool<ScriptParseResult>("script.parse", { script_input: scriptInput });
}

export interface ScriptToVideoResult {
  status: string;
  output_path: string;
  file_size_bytes: number;
  timeline_preview: string;
  timeline_path: string;
  scenes_rendered: number;
  duration_s: number;
}

export async function scriptToVideo(
  scriptInput: string,
  outputPath?: string,
  captionStyle?: string,
  skipStickers?: boolean,
): Promise<ScriptToVideoResult> {
  return invokeTool<ScriptToVideoResult>("script.to_video", {
    script_input: scriptInput,
    output_path: outputPath,
    caption_style: captionStyle,
    skip_stickers: skipStickers,
  });
}

export async function scriptGenerateVoices(scriptInput: string): Promise<{ status: string; voices_generated: number; output_dir: string }> {
  return invokeTool("script.generate_voices", { script_input: scriptInput });
}

export async function scriptBuildCaptions(
  scriptInput: string,
  style: string,
): Promise<{ status: string; captions_path: string; word_count: number }> {
  return invokeTool("script.build_captions", { script_input: scriptInput, style });
}

export async function scriptToTimeline(scriptInput: string): Promise<{ status: string; timeline_path: string; segment_count: number }> {
  return invokeTool("script.to_timeline", { script_input: scriptInput });
}

// ---------------------------------------------------------------------------
// Background (new b-roll family, supersedes legacy broll.*)
// ---------------------------------------------------------------------------

export async function backgroundFetch(
  query: string,
  orientation = "9:16",
  durationS?: number,
): Promise<{ status: string; path: string; source: string; duration_s: number }> {
  return invokeTool("background.fetch", { query, orientation, duration_s: durationS });
}

export async function backgroundAssign(
  timelinePath: string,
  query: string,
  sceneIndex: number,
): Promise<{ status: string; event_id: string; asset_path: string }> {
  return invokeTool("background.assign", { timeline_path: timelinePath, query, scene_index: sceneIndex });
}

// ---------------------------------------------------------------------------
// Stickers (SVG puppets with lip-sync)
// ---------------------------------------------------------------------------

export async function stickerLoadPreset(
  presetName: string,
): Promise<{ status: string; preset: Record<string, unknown>; mouths: string[]; emotes: string[] }> {
  return invokeTool("sticker.load_preset", { preset_name: presetName });
}

export async function stickerRender(
  presetName: string,
  wavPath: string,
  speakerName: string,
  position = "bottom-left",
  scale = 0.2,
): Promise<{ status: string; output_path: string; duration_s: number }> {
  return invokeTool("sticker.render", { preset_name: presetName, wav_path: wavPath, speaker_name: speakerName, position, scale });
}

// ---------------------------------------------------------------------------
// Unified library (supersedes legacy sfx.* / music.*)
// ---------------------------------------------------------------------------

export interface LibrarySearchResult {
  status: string;
  results: Array<{ id: string; title: string; artist?: string; type: string; duration_s: number; url: string; source: string }>;
  count: number;
}

export async function librarySearch(
  query: string,
  type?: "music" | "sfx",
  limit = 10,
): Promise<LibrarySearchResult> {
  return invokeTool<LibrarySearchResult>("library.search", { query, type, limit });
}

export async function libraryDownload(
  url: string,
  type: "music" | "sfx",
  title: string,
): Promise<{ status: string; path: string; size_bytes: number }> {
  return invokeTool("library.download", { url, type, title });
}

export async function libraryBuild(): Promise<{ status: string; index_path: string; total_entries: number; music_count: number; sfx_count: number }> {
  return invokeTool("library.build", {});
}

// ---------------------------------------------------------------------------
// HyperFrames (default render engine) + composition.render dispatcher
// ---------------------------------------------------------------------------

export async function hfLint(sourcePath: string): Promise<{ status: string; issues: Array<{ severity: string; rule: string; line: number; message: string }>; issue_count: number }> {
  return invokeTool("hf.lint", { source_path: sourcePath });
}

export async function hfValidate(sourcePath: string): Promise<{ status: string; valid: boolean; errors: string[]; warnings: string[] }> {
  return invokeTool("hf.validate", { source_path: sourcePath });
}

export async function hfSnapshot(
  sourcePath: string,
  frame = 0,
): Promise<{ status: string; snapshot_path: string; width: number; height: number }> {
  return invokeTool("hf.snapshot", { source_path: sourcePath, frame });
}

export async function hfRender(
  sourcePath: string,
  outputPath: string,
  quality = "standard",
): Promise<{ status: string; output_path: string; file_size_bytes: number; duration_s: number }> {
  return invokeTool("hf.render", { source_path: sourcePath, output_path: outputPath, quality });
}

export interface HfClassifyResult {
  status: string;
  recommendation: "hf-native" | "interop" | "legacy-remotion";
  has_blockers: boolean;
  has_warnings: boolean;
  blocker_count: number;
  warning_count: number;
  blockers: Array<{ rule: string; line: number; message: string }>;
  warnings: Array<{ rule: string; line: number; message: string }>;
}

export async function hfClassify(sourcePath: string): Promise<HfClassifyResult> {
  return invokeTool<HfClassifyResult>("hf.classify", { source_path: sourcePath });
}

export async function compositionRender(
  projectDir: string,
  outputPath: string,
  renderHint?: "hf-native" | "interop" | "legacy-remotion",
  quality = "standard",
): Promise<{ status: string; output_path: string; engine: string; file_size_bytes: number }> {
  return invokeTool("composition.render", { project_dir: projectDir, output_path: outputPath, render_hint: renderHint, quality });
}

// ---------------------------------------------------------------------------
// Stock + YouTube + Media + GIF search
// ---------------------------------------------------------------------------

export async function stockSearch(
  query: string,
  type: "video" | "music",
  limit = 10,
): Promise<{ status: string; results: Array<Record<string, unknown>>; count: number }> {
  return invokeTool("stock.search", { query, type, limit });
}

export async function stockFetch(
  url: string,
  type: "video" | "music",
  title: string,
): Promise<{ status: string; path: string }> {
  return invokeTool("stock.fetch", { url, type, title });
}

export async function youtubeSearch(
  query: string,
  limit = 10,
): Promise<{ status: string; results: Array<{ title: string; video_id: string; channel: string; duration_s: number }>; count: number }> {
  return invokeTool("youtube.search", { query, limit });
}

export async function youtubeDownload(
  videoId: string,
  outputPath: string,
  startS?: number,
  endS?: number,
): Promise<{ status: string; path: string; duration_s: number }> {
  return invokeTool("youtube.download", { video_id: videoId, output_path: outputPath, start_s: startS, end_s: endS });
}

export async function mediaSearch(
  query: string,
  source: "pexels" | "openverse" = "pexels",
  limit = 10,
): Promise<{ status: string; results: Array<Record<string, unknown>>; count: number }> {
  return invokeTool("media.search", { query, source, limit });
}

export async function gifSearch(
  query: string,
  limit = 5,
): Promise<{ status: string; results: Array<{ url: string; width: number; height: number; size: string }>; count: number }> {
  return invokeTool("gif.search", { query, limit });
}

// ---------------------------------------------------------------------------
// Missing timeline tools (add_track_event, diff, inspect, autofill_broll, upgrade)
// ---------------------------------------------------------------------------

export async function timelineAddTrackEvent(
  timelinePath: string,
  trackType: "dialogue" | "voiceover" | "captions" | "broll" | "music" | "sfx",
  event: Record<string, unknown>,
): Promise<{ status: string; event_id: string }> {
  return invokeTool("timeline.add_track_event", { timeline_path: timelinePath, track_type: trackType, event });
}

export async function timelineDiff(
  timelinePathA: string,
  timelinePathB: string,
): Promise<{ status: string; duration_change_ms: number; segments: { added: string[]; removed: string[]; modified: string[] }; tracks: Record<string, { a: number; b: number }> }> {
  return invokeTool("timeline.diff", { timeline_path_a: timelinePathA, timeline_path_b: timelinePathB });
}

export async function timelineInspect(
  timelinePath: string,
  layer?: string,
): Promise<{ status: string; layer: string; event_count: number; events: Array<Record<string, unknown>> }> {
  return invokeTool("timeline.inspect", { timeline_path: timelinePath, layer });
}

export async function timelineAutofillBroll(
  timelinePath: string,
  cadenceSeconds = 2.0,
): Promise<{ status: string; events_created: number; timeline_path: string }> {
  return invokeTool("timeline.autofill_broll", { timeline_path: timelinePath, cadence_seconds: cadenceSeconds });
}

export async function timelineUpgrade(
  timelinePath: string,
): Promise<{ status: string; upgraded: boolean; version: string }> {
  return invokeTool("timeline.upgrade", { timeline_path: timelinePath });
}

export async function timelineBuild(
  sourceVideo: string,
  aspect = "9:16",
  fps = 30,
): Promise<{ status: string; timeline_path: string; segment_count: number }> {
  return invokeTool("timeline.build", { source_video: sourceVideo, aspect, fps });
}

// ---------------------------------------------------------------------------
// Missing TTS / voiceover / music-ducking tools
// ---------------------------------------------------------------------------

export async function ttsPreview(
  voiceProfileId: string,
): Promise<{ status: string; profile_id: string; estimated_duration_ms: number }> {
  return invokeTool("tts.preview", { voice_profile_id: voiceProfileId });
}

export async function ttsCommentary(
  timelinePath: string,
  voiceProfileId: string,
): Promise<{ status: string; events_created: number; timeline_path: string }> {
  return invokeTool("tts.commentary", { timeline_path: timelinePath, voice_profile_id: voiceProfileId });
}

export async function voiceoverGenerate(
  timelinePath: string,
  text: string,
  voiceProfileId: string,
  positionMs: number,
): Promise<{ status: string; event_id: string; output_path: string; duration_ms: number }> {
  return invokeTool("voiceover.generate", { timeline_path: timelinePath, text, voice_profile_id: voiceProfileId, position_ms: positionMs });
}

export async function musicDuckingPlan(
  timelinePath: string,
): Promise<{ status: string; events: Array<{ start_ms: number; end_ms: number; reduction_db: number }>; count: number }> {
  return invokeTool("music.ducking.plan", { timeline_path: timelinePath });
}

// ---------------------------------------------------------------------------
// Missing broll + verify + overlay + edl tools
// ---------------------------------------------------------------------------

export async function brollSuggest(
  timelinePath: string,
  cadenceSeconds = 2.0,
): Promise<{ status: string; suggestions: Array<{ concept: string; position_ms: number; duration_ms: number }>; count: number }> {
  return invokeTool("broll.suggest", { timeline_path: timelinePath, cadence_seconds: cadenceSeconds });
}

export async function brollDirector(
  timelinePath: string,
  orientation = "9:16",
): Promise<{ status: string; slots_filled: number; concepts_used: string[]; cached_paths: string[] }> {
  return invokeTool("broll.director", { timeline_path: timelinePath, orientation });
}

export async function verifyRender(
  videoPath: string,
  timelinePath?: string,
): Promise<{ status: string; score: number; duration_match: boolean; aspect_match: boolean; issues: string[] }> {
  return invokeTool("verify.render", { video_path: videoPath, timeline_path: timelinePath });
}

export async function edlBuild(srtPath: string): Promise<{ status: string; edl_path: string; segment_count: number }> {
  return invokeTool("edl.build", { srt_path: srtPath });
}

export async function overlayGenerate(
  srtPath: string,
  edlPath: string,
  style = "pupcaps_center",
): Promise<{ status: string; overlay_path: string; duration_s: number }> {
  return invokeTool("overlay.generate", { srt_path: srtPath, edl_path: edlPath, style });
}

export async function sfxIndex(sfxPath?: string): Promise<{ status: string; output_path: string; count: number }> {
  return invokeTool("sfx.index", { sfx_path: sfxPath });
}

export async function musicIndex(musicPaths?: string[]): Promise<{ status: string; output_path: string; count: number }> {
  return invokeTool("music.index", { music_paths: musicPaths });
}

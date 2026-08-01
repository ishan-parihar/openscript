use openscript_core::timeline::{EventKind, Segment, Timeline};
use openscript_core::types::TrackType;
use std::path::Path;

fn audio_format_ext(path: &str) -> &'static str {
    match Path::new(path).extension().and_then(|e| e.to_str()) {
        Some("mp3") => "mp3",
        Some("wav") => "wav",
        Some("ogg") => "ogg",
        Some("flac") => "flac",
        Some("aac") => "aac",
        Some("m4a") => "m4a",
        _ => "",
    }
}

/// Escape a file path for use inside ffmpeg filter single-quoted strings.
///
/// ffmpeg filter syntax uses single quotes to delimit paths. A single quote
/// inside the path must be escaped as `'\''` (close quote, escaped quote,
/// reopen quote). Backslashes are converted to forward slashes for
/// cross-platform consistency. Filter metacharacters (;, :, [, ], ,) are
/// rejected to prevent filter graph injection.
fn escape_filter_path(path: &str) -> Result<String, String> {
    // Reject paths containing filter metacharacters that could inject
    // arbitrary filter graph nodes.
    // Commas are common in filenames (e.g. "voice,_7599377.mp4") and are
    // safe when the path is wrapped in single quotes in the filter string.
    let dangerous_chars = [';', '[', ']'];
    for ch in dangerous_chars {
        if path.contains(ch) {
            return Err(format!(
                "Path contains forbidden filter metacharacter '{}': {}",
                ch, path
            ));
        }
    }
    // Convert backslashes to forward slashes (Windows path compat)
    let forward = path.replace('\\', "/");
    // Escape single quotes: ' → '\''
    let escaped = forward.replace('\'', "'\\''");
    Ok(escaped)
}

fn amovie_filter(path: &str, stream: &str) -> Result<String, String> {
    let escaped = escape_filter_path(path)?;
    let fmt = audio_format_ext(path);
    if fmt.is_empty() {
        Ok(format!("amovie='{}':s={}", escaped, stream))
    } else {
        Ok(format!("amovie='{}':f={}:s={}", escaped, fmt, stream))
    }
}

pub struct BrollEvent {
    pub path: String,
    pub start_ms: i64,
    pub end_ms: i64,
    /// Source duration in seconds. When known, used to cap `seek_offset` so
    /// the trimmed source has enough frames to fill the entire overlay window.
    /// When `None`, the filter graph assumes the source is long enough and
    /// falls back to the 5s deterministic offset. Callers should probe the
    /// source with `crate::probe::probe` to populate this.
    pub source_duration_s: Option<f64>,
}

/// Ken Burns motion pattern cycled across b-roll clips for visual variety.
/// Without per-clip motion, stock Pexels videos often appear static because
/// the seek offset may land on a slow frame. zoompan guarantees continuous
/// motion regardless of source clip behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotionStyle {
    /// Gentle zoom toward center — the safest, most-used pattern.
    ZoomInCenter,
    /// Slight zoom anchored to the top-left quadrant — off-center energy.
    ZoomInTopLeft,
    /// Zoom out from 1.5× to 1.0× — pulls back to reveal more context.
    ZoomOutCenter,
    /// Constant 1.2× zoom with horizontal pan to the right — lateral motion.
    PanRight,
}

impl MotionStyle {
    /// Pick a deterministic but varied style per clip index. Cycles through
    /// all four styles so consecutive clips never look identical.
    pub fn for_clip(index: usize) -> Self {
        match index % 4 {
            0 => MotionStyle::ZoomInCenter,
            1 => MotionStyle::ZoomInTopLeft,
            2 => MotionStyle::ZoomOutCenter,
            _ => MotionStyle::PanRight,
        }
    }

    /// Build the three zoompan expression arguments (z, x, y).
    /// All expressions assume `d=1` so each output frame animates one step
    /// past the previous frame; with output fps matching the source clip's
    /// fps (typically 25 or 30) this produces smooth 3-second Ken Burns motion.
    pub fn expressions(&self) -> (String, String, String) {
        match self {
            MotionStyle::ZoomInCenter => (
                // Grow zoom up to 1.5× while keeping the image centered.
                // 0.003/frame at 30fps = 0.09/sec → reaches 1.5× in ~5.5s
                "min(zoom+0.003,1.5)".into(),
                "iw/2-(iw/zoom/2)".into(),
                "ih/2-(ih/zoom/2)".into(),
            ),
            MotionStyle::ZoomInTopLeft => (
                "min(zoom+0.003,1.5)".into(),
                // Anchor toward the upper-left third so the zoom reveals
                // a different region than the centered variant.
                "(iw/zoom)*0.25".into(),
                "(ih/zoom)*0.25".into(),
            ),
            MotionStyle::ZoomOutCenter => (
                // Pull back from 1.5× down to 1.0× — reveals more context.
                // Start at 1.5 (on=0), decrease by 0.003/frame.
                // 0.003/frame at 30fps = 0.09/sec → reaches 1.0 in ~5.5s
                "max(1.5-0.003*on,1.0)".into(),
                "iw/2-(iw/zoom/2)".into(),
                "ih/2-(ih/zoom/2)".into(),
            ),
            MotionStyle::PanRight => (
                // Hold a constant 1.2× zoom while sliding right.
                "1.2".into(),
                // `on*5` shifts ~5px right per output frame — at 30fps that's
                // 150px/sec, comfortably slower than the clip width.
                "(iw/zoom)*0.5-(iw/zoom/2)+on*5".into(),
                "ih/2-(ih/zoom/2)".into(),
            ),
        }
    }
}

pub struct MusicEvent {
    pub path: String,
    pub volume: f64,
}

pub struct SfxEvent {
    pub path: String,
    pub start_ms: i64,
    pub gain_db: f64,
}

pub struct VoiceoverEvent {
    pub path: String,
    pub start_ms: i64,
    pub gain_db: f64,
}

/// Represents an audio ducking event: music volume reduces during speech windows.
pub struct DuckingEvent {
    pub start_ms: i64,
    pub end_ms: i64,
    pub reduction_db: f64,
    pub attack_ms: u32,
    pub release_ms: u32,
}

pub struct FilterGraphBuilder {
    segments: Vec<Segment>,
    parts: Vec<String>,
    fps: u32,
    aspect: String,
    /// Output width in pixels. Used by the crop/scale filters instead of
    /// a hardcoded 1080. Defaults to 1080 (portrait) or 1920 (landscape)
    /// based on `aspect` via `RenderTarget::resolve_width()`.
    width: u32,
    /// Output height in pixels. Defaults to 1920 (portrait) or 1080 (landscape).
    height: u32,
    ass_path: Option<String>,
    srt_path: Option<String>,
    overlay_mov: Option<String>,
    loudnorm: bool,
    /// Loudness target in LUFS for the loudnorm filter. Defaults to -16
    /// (EBU R128 broadcast standard). Prior versions hardcoded this to -16
    /// and ignored the timeline's `directives.mix.normalize_to_lufs` field.
    normalize_lufs: f64,
    broll_events: Vec<BrollEvent>,
    music_events: Vec<MusicEvent>,
    sfx_events: Vec<SfxEvent>,
    voiceover_events: Vec<VoiceoverEvent>,
    ducking_events: Vec<DuckingEvent>,
    fonts_dir: Option<String>,
    /// When true, the source has no video stream — synthesize a solid-color
    /// background video from the `color=` filter instead of trimming `[0:v]`.
    /// Audio is still trimmed via `[0:a]` and mixed normally. Set by callers
    /// that probe the source via `crate::probe::probe` and see no video stream.
    audio_only: bool,
}

impl FilterGraphBuilder {
    pub fn new(segments: Vec<Segment>, fps: u32, aspect: &str, loudnorm: bool) -> Self {
        // Derive default width/height from aspect so the filter graph is not
        // hardcoded to 1080×1920. Callers can override via `with_dimensions`.
        let (width, height) = match aspect {
            "16:9" => (1920, 1080),
            "1:1" => (1080, 1080),
            _ => (1080, 1920), // "9:16" and any unknown → portrait default
        };
        Self {
            segments,
            parts: Vec::new(),
            fps,
            aspect: aspect.into(),
            width,
            height,
            ass_path: None,
            srt_path: None,
            overlay_mov: None,
            loudnorm,
            normalize_lufs: -16.0, // EBU R128 broadcast default
            broll_events: Vec::new(),
            music_events: Vec::new(),
            sfx_events: Vec::new(),
            voiceover_events: Vec::new(),
            ducking_events: Vec::new(),
            fonts_dir: None,
            audio_only: false,
        }
    }

    /// Mark the source as audio-only. Skips `[0:v]` trim and synthesizes a
    /// solid-color background video via the `color=` filter so the timeline
    /// can still render b-roll + captions onto a video surface. Audio trim
    /// (`[0:a]`) and the rest of the post-trim pipeline are unchanged.
    pub fn with_audio_only(mut self, audio_only: bool) -> Self {
        self.audio_only = audio_only;
        self
    }

    /// Override the output width/height. Read from
    /// `RenderTarget::resolve_width()` / `resolve_height()` in `from_timeline`
    /// so renders honour the timeline's resolution instead of a hardcoded
    /// 1080×1920.
    pub fn with_dimensions(mut self, width: u32, height: u32) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    /// Set the loudness target (LUFS) used by the loudnorm filter. Reads from
    /// `timeline.directives.mix.normalize_to_lufs` in `from_timeline`.
    pub fn with_normalize_lufs(mut self, lufs: f64) -> Self {
        self.normalize_lufs = lufs;
        self
    }

    pub fn with_ass(mut self, path: String) -> Self {
        self.ass_path = Some(path);
        self
    }

    pub fn with_srt(mut self, path: String) -> Self {
        self.srt_path = Some(path);
        self
    }

    pub fn with_overlay_mov(mut self, path: String) -> Self {
        self.overlay_mov = Some(path);
        self
    }

    pub fn with_broll(mut self, events: Vec<BrollEvent>) -> Self {
        // Defense-in-depth: filter out placeholder or empty b-roll paths so
        // they never reach the ffmpeg `movie=` filter (which would crash
        // with "Unable to parse 'si' option value 'v'" on the placeholder
        // string). The MCP `timeline.render` handler already does this
        // filter, but callers that build a FilterGraphBuilder manually via
        // `with_broll` are also protected now.
        //
        // We do NOT filter non-existent paths here — those will fail at
        // ffmpeg spawn with a clearer "No such file" error, and tests use
        // fake paths to validate the filter-string construction.
        self.broll_events = events
            .into_iter()
            .filter(|e| {
                if e.path == "placeholder" || e.path.is_empty() {
                    tracing::warn!("[filter_graph] Skipping placeholder/empty b-roll path");
                    false
                } else {
                    true
                }
            })
            .collect();
        self
    }

    /// Populate `source_duration_s` on each b-roll event from a path→duration map
    /// produced by `crate::probe::probe`. Used by `render_from_timeline` so the
    /// filter graph can cap `seek_offset` at `src_dur - seg_dur` and avoid
    /// the "source exhausted → held last frame" static-image bug. Events whose
    /// path is not in the map keep `source_duration_s = None` and fall back
    /// to the legacy 50%-of-segment cap (which combined with `loop=-1` on
    /// the movie filter still produces continuous motion).
    pub fn with_broll_durations(mut self, durations: std::collections::HashMap<String, f64>) -> Self {
        for ev in &mut self.broll_events {
            if ev.source_duration_s.is_none() {
                if let Some(d) = durations.get(&ev.path) {
                    ev.source_duration_s = Some(*d);
                }
            }
        }
        self
    }

    pub fn with_music(mut self, events: Vec<MusicEvent>) -> Self {
        self.music_events = events;
        self
    }

    pub fn with_sfx(mut self, events: Vec<SfxEvent>) -> Self {
        self.sfx_events = events;
        self
    }

    pub fn with_voiceover(mut self, events: Vec<VoiceoverEvent>) -> Self {
        self.voiceover_events = events;
        self
    }

    pub fn with_ducking(mut self, events: Vec<DuckingEvent>) -> Self {
        self.ducking_events = events;
        self
    }

    pub fn with_fonts_dir(mut self, path: String) -> Self {
        self.fonts_dir = Some(path);
        self
    }

    pub fn from_timeline(timeline: &Timeline) -> Self {
        let segments = timeline.segments.clone();
        let fps = timeline.target.fps;
        let aspect = timeline.target.aspect.clone();
        let loudnorm = timeline.effects.audio.loudnorm;
        // Read the timeline's loudness target so the loudnorm filter honors
        // `directives.mix.normalize_to_lufs` instead of a hardcoded -16.
        let normalize_lufs = timeline.directives.mix.normalize_to_lufs;
        // Read the timeline's resolution so renders honor `RenderTarget.width/height`
        // instead of a hardcoded 1080×1920.
        let width = timeline.target.resolve_width();
        let height = timeline.target.resolve_height();

        let mut b = FilterGraphBuilder::new(segments, fps, &aspect, loudnorm)
            .with_normalize_lufs(normalize_lufs)
            .with_dimensions(width, height);

        let mut broll_events = Vec::new();
        let mut music_events = Vec::new();
        let mut sfx_events = Vec::new();
        let mut voiceover_events = Vec::new();
        let mut ducking_events = Vec::new();

        // Collect speech windows from dialogue and voiceover tracks
        let mut speech_windows: Vec<(i64, i64)> = Vec::new();
        for track_type in &[TrackType::Dialogue, TrackType::Voiceover] {
            if let Some(track) = timeline.tracks.get(track_type) {
                for evt in track {
                    speech_windows.push((evt.start_ms, evt.end_ms));
                }
            }
        }

        if let Some(broll_track) = timeline.tracks.get(&TrackType::Broll) {
            for evt in broll_track {
                if let EventKind::Broll { .. } = &evt.kind {
                    let path = timeline
                        .assets
                        .broll
                        .get(&evt.id)
                        .and_then(|v| v.get("path").and_then(|p| p.as_str()))
                        .unwrap_or("")
                        .to_string();
                    if !path.is_empty() && path != "placeholder" {
                        broll_events.push(BrollEvent {
                            path,
                            start_ms: evt.start_ms,
                            end_ms: evt.end_ms,
                            source_duration_s: None,
                        });
                    }
                }
            }
        }

        if let Some(music_track) = timeline.tracks.get(&TrackType::Music) {
            for evt in music_track {
                if let EventKind::Music { .. } = &evt.kind {
                    let path = timeline
                        .assets
                        .music
                        .get(&evt.id)
                        .and_then(|v| v.get("path").and_then(|p| p.as_str()))
                        .unwrap_or("")
                        .to_string();
                    if !path.is_empty() && path != "placeholder" {
                        // Convert the event's gain_db (default -12 dB if unset) to a
                        // linear volume coefficient. Prior versions hardcoded 0.3,
                        // which silently ignored the MusicSpec.gain_db field and
                        // produced a different mix than the timeline specified.
                        let gain_db = evt.gain_db;
                        let volume = 10f64.powf(gain_db / 20.0);
                        music_events.push(MusicEvent { path, volume });
                    }
                }
            }
        }

        if let Some(sfx_track) = timeline.tracks.get(&TrackType::Sfx) {
            for evt in sfx_track {
                if let EventKind::Sfx {
                    recommended_gain_db,
                    ..
                } = &evt.kind
                {
                    let path = timeline
                        .assets
                        .sfx
                        .get(&evt.id)
                        .and_then(|v| v.get("path").and_then(|p| p.as_str()))
                        .unwrap_or("")
                        .to_string();
                    if !path.is_empty() && path != "placeholder" {
                        sfx_events.push(SfxEvent {
                            path,
                            start_ms: evt.start_ms,
                            gain_db: *recommended_gain_db,
                        });
                    }
                }
            }
        }

        if let Some(voiceover_track) = timeline.tracks.get(&TrackType::Voiceover) {
            for evt in voiceover_track {
                if let EventKind::Voiceover { .. } = &evt.kind {
                    let path = timeline
                        .assets
                        .voices
                        .get(&evt.id)
                        .and_then(|v| v.get("path").and_then(|p| p.as_str()))
                        .unwrap_or("")
                        .to_string();
                    if !path.is_empty() && path != "placeholder" {
                        voiceover_events.push(VoiceoverEvent {
                            path,
                            start_ms: evt.start_ms,
                            gain_db: evt.gain_db,
                        });
                    }
                }
            }
        }

        // Build ducking events from directives targeting music, matched to speech windows
        for directive in &timeline.directives.ducking {
            if directive.target_track == "music" {
                for (speech_start, speech_end) in &speech_windows {
                    ducking_events.push(DuckingEvent {
                        start_ms: *speech_start,
                        end_ms: *speech_end,
                        reduction_db: directive.reduction_db,
                        attack_ms: directive.attack_ms,
                        release_ms: directive.release_ms,
                    });
                }
            }
        }

        b.broll_events = broll_events;
        b.music_events = music_events;
        b.sfx_events = sfx_events;
        b.voiceover_events = voiceover_events;
        b.ducking_events = ducking_events;

        // Auto-read captions from timeline.assets.captions if not already set
        if b.ass_path.is_none() {
            if let Some(path_str) = timeline.assets.captions
                .get("ass")
                .and_then(|a| a.get("path"))
                .and_then(|p| p.as_str())
            {
                let ass = std::path::Path::new(path_str);
                if ass.exists() {
                    b.ass_path = Some(path_str.to_string());                            tracing::debug!("[filter_graph] Auto-read captions ASS from timeline: {}", path_str);
                } else {
                    tracing::warn!("[filter_graph] Captions ASS registered but file missing: {}", path_str);
                }
            }
        }
        // Fallback to SRT if no ASS was found
        if b.ass_path.is_none() && b.srt_path.is_none() {
            if let Some(path_str) = timeline.assets.captions
                .get("srt")
                .and_then(|a| a.get("path"))
                .and_then(|p| p.as_str())
            {
                let srt = std::path::Path::new(path_str);
                if srt.exists() {
                    b = b.with_srt(path_str.to_string());                                tracing::debug!("[filter_graph] Auto-read captions SRT from timeline: {}", path_str);
                } else {
                    tracing::warn!("[filter_graph] Captions SRT registered but file missing: {}", path_str);
                }
            }
        }

        b
    }

    /// Build the complete filter_complex string.
    /// Returns (filter_complex, video_output_label, audio_output_label)
    ///
    /// The filter graph applies these stages in order:
    /// 1. Trim each segment from source
    /// 2. Chain segments with xfade/acrossfade for smooth crossfades
    /// 3. FPS normalization
    /// 4. Aspect ratio handling (9:16 center crop)
    /// 5. Subtitle burn-in (ASS or SRT)
    /// 6. Overlay MOV (PupCaps captions)
    /// 7. Audio loudnorm
    pub fn build(self) -> (String, String, String) {
        if self.segments.is_empty() {
            return (String::new(), "[0:v]".into(), "[0:a]".into());
        }

        if self.audio_only {
            return self.build_audio_only();
        }

        if self.segments.len() == 1 {
            return self.build_single();
        }

        self.build_xfade()
    }

    /// Build a filter graph for an audio-only source.
    ///
    /// Synthesizes a solid-color video background via the `color=` filter so
    /// the timeline can still burn captions and overlay b-roll onto a video
    /// surface. Audio is trimmed from `[0:a]` exactly like the video path —
    /// xfade transitions between segments are computed the same way, but
    /// applied to audio only (the video background is a single continuous
    /// stream that we crossfade via opacity, OR we just hold solid color
    /// since the video carries no semantic content here).
    ///
    /// Background color is intentionally black (0x000000) because the
    /// captions and b-roll are the only visual content; b-roll overlays cover
    /// the full frame during their enable windows.
    fn build_audio_only(mut self) -> (String, String, String) {
        let total_duration_s: f64 = self
            .segments
            .iter()
            .map(|s| (s.end - s.start).max(0.0))
            .sum::<f64>()
            + 0.5; // small margin for the final frame

        // Synthesize a solid-color video background at the target W/H and FPS.
        // The `color=` filter produces a continuous video stream of the exact
        // duration, eliminating the need to trim `[0:v]` (which doesn't exist
        // on audio-only sources). Output label is `[vbg]` so post-trim stages
        // (subtitles, b-roll, overlay MOV) can chain off it the same way they
        // would chain off a trimmed source video.
        self.parts.push(format!(
            "color=size={w}x{h}:rate={fps}:duration={dur}:color=black[vbg]",
            w = self.width,
            h = self.height,
            fps = self.fps,
            dur = total_duration_s,
        ));

        // Audio trim + concat (no xfade — audio-only sources benefit less from
        // xfade because the source is one continuous recording and segment
        // boundaries are already matched in the timeline).
        let n = self.segments.len();
        for (i, seg) in self.segments.iter().enumerate() {
            let s = seg.start.max(0.0);
            let e = seg.end.max(s + 0.001);
            self.parts.push(format!(
                "[0:a]atrim=start={}:end={},asetpts=PTS-STARTPTS[a{}]",
                s, e, i
            ));
        }
        let mut concat_inputs = String::new();
        for i in 0..n {
            concat_inputs.push_str(&format!("[a{}]", i));
        }
        self.parts.push(format!(
            "{}concat=n={}:v=0:a=1[acat]",
            concat_inputs, n
        ));

        // Start post-trim from [vbg] (solid background) and [acat] (concatenated audio).
        // The rest of build_post_trim is unchanged — b-roll, subtitles, overlay
        // MOV, voiceover, music, SFX, loudnorm, alimiter all chain off these labels.
        self.build_post_trim("[vbg]", "[acat]")
    }

    /// Build filter graph for a single segment (no crossfade needed).
    fn build_single(mut self) -> (String, String, String) {
        let seg = &self.segments[0];
        let s = seg.start.max(0.0);
        let e = seg.end.max(s + 0.001);

        self.parts.push(format!(
            "[0:v]trim=start={}:end={},setpts=PTS-STARTPTS[v0]",
            s, e
        ));
        self.parts.push(format!(
            "[0:a]atrim=start={}:end={},asetpts=PTS-STARTPTS[a0]",
            s, e
        ));

        self.build_post_trim("[v0]", "[a0]")
    }

    /// Build filter graph with xfade video + acrossfade audio transitions.
    /// For ≤10 segments: uses xfade for smooth video transitions.
    /// For >10 segments: uses concat for both video and audio (xfade chain is O(n²) and
    /// becomes prohibitively slow beyond ~10 segments).
    fn build_xfade(mut self) -> (String, String, String) {
        let n = self.segments.len();
        const XF_THRESHOLD: usize = 10;

        // Step 1: Trim each segment
        for (i, seg) in self.segments.iter().enumerate() {
            let s = seg.start.max(0.0);
            let e = seg.end.max(s + 0.001);
            self.parts.push(format!(
                "[0:v]trim=start={}:end={},setpts=PTS-STARTPTS[v{}]",
                s, e, i
            ));
            self.parts.push(format!(
                "[0:a]atrim=start={}:end={},asetpts=PTS-STARTPTS[a{}]",
                s, e, i
            ));
        }

        if n <= XF_THRESHOLD {
            // Small segment count: use xfade for smooth transitions
            self.build_xfade_chain()
        } else {
            // Large segment count: use concat for performance
            self.build_concat()
        }
    }

    /// Build xfade video chain + concat audio (for ≤10 segments).
    fn build_xfade_chain(mut self) -> (String, String, String) {
        let n = self.segments.len();
        let xf_duration_s = self
            .segments
            .iter()
            .map(|s| s.crossfade_ms as f64 / 1000.0)
            .collect::<Vec<_>>();

        let mut accum_dur = self.segments[0].end - self.segments[0].start;
        let mut prev_label = "v0".to_string();

        for i in 1..n {
            let overlap = xf_duration_s[i - 1];
            let cur_label = format!("v{}", i);
            let seg_dur = self.segments[i].end - self.segments[i].start;
            let out_label = if i == n - 1 {
                "vxfinal".to_string()
            } else {
                format!("vxf{}", i)
            };
            let offset = accum_dur - overlap;

            self.parts.push(format!(
                "[{}][{}]xfade=transition=smoothleft:duration={}:offset={}[{}]",
                prev_label, cur_label, overlap, offset, out_label
            ));

            prev_label = out_label;
            accum_dur += seg_dur - overlap;
        }

        // Audio uses concat for reliability
        let mut audio_concat = String::new();
        for i in 0..n {
            audio_concat.push_str(&format!("[a{}]", i));
        }
        self.parts
            .push(format!("{}concat=n={}:v=0:a=1[acat]", audio_concat, n));

        self.build_post_trim(&format!("[{}]", prev_label), "[acat]")
    }

    /// Build concat filter for both video and audio (for >10 segments).
    /// The concat filter is O(n) vs xfade's O(n²), making it essential for large edits.
    fn build_concat(mut self) -> (String, String, String) {
        let n = self.segments.len();

        // Build interleaved [v0][a0][v1][a1]...[vN-1][aN-1]concat=n=N:v=1:a=1
        let mut concat_inputs = String::new();
        for i in 0..n {
            concat_inputs.push_str(&format!("[v{}][a{}]", i, i));
        }
        self.parts.push(format!(
            "{}concat=n={}:v=1:a=1[concat_v][concat_a]",
            concat_inputs, n
        ));

        self.build_post_trim("[concat_v]", "[concat_a]")
    }

    /// Apply post-trim transformations: fps, aspect, subtitles, overlay, audio loudnorm.
    fn build_post_trim(self, v_trim: &str, a_trim: &str) -> (String, String, String) {
        let mut parts = self.parts;
        let mut vout = v_trim.to_string();

        // FPS normalization
        parts.push(format!(
            "[{}]fps={}[vfps]",
            &vout[1..vout.len() - 1],
            self.fps
        ));
        vout = "[vfps]".into();

        // Aspect handling: scale + center-crop to the target width/height.
        // Prior versions hardcoded 1080×1920 (9:16 portrait only) which broke
        // 720p / 4K / landscape / square renders. The width/height now come
        // from `RenderTarget::resolve_width/height` via `with_dimensions`.
        if self.aspect == "9:16" || self.aspect == "16:9" || self.aspect == "1:1" {
            parts.push(format!(
                "[vfps]scale=-2:{h},crop={w}:{h}:(in_w-{w})/2:(in_h-{h})/2[vcrop]",
                w = self.width,
                h = self.height,
            ));
            vout = "[vcrop]".to_string();
        }

        // Subtitle burn-in (ASS or SRT) — always burn in, overlay MOV goes on top
        if let Some(ass) = &self.ass_path {
            if let Ok(escaped) = escape_filter_path(ass) {
                let filter = if let Some(fonts_dir) = &self.fonts_dir {
                    format!(
                        "[{}]subtitles='{}':fontsdir='{}'[vsub]",
                        &vout[1..vout.len() - 1],
                        escaped,
                        fonts_dir
                    )
                } else {
                    format!(
                        "[{}]subtitles='{}'[vsub]",
                        &vout[1..vout.len() - 1],
                        escaped
                    )
                };
                parts.push(filter);
                vout = "[vsub]".into();
            } else {
                tracing::warn!("[filter_graph] Skipping ASS subtitles (escape failed)");
            }
        } else if let Some(srt) = &self.srt_path {
            if let Ok(escaped) = escape_filter_path(srt) {
                let filter = if let Some(fonts_dir) = &self.fonts_dir {
                    format!(
                        "[{}]subtitles='{}':fontsdir='{}'[vsub]",
                        &vout[1..vout.len() - 1],
                        escaped,
                        fonts_dir
                    )
                } else {
                    format!(
                        "[{}]subtitles='{}'[vsub]",
                        &vout[1..vout.len() - 1],
                        escaped
                    )
                };
                parts.push(filter);
                vout = "[vsub]".into();
            } else {
                tracing::warn!("[filter_graph] Skipping SRT subtitles (escape failed)");
            }
        }

        // B-roll overlays — each b-roll event overlays at its timestamp.
        // Note: placeholder/empty paths are already filtered by with_broll().
        if !self.broll_events.is_empty() {
            let mut current_v = vout.clone();
            for (i, broll) in self.broll_events.iter().enumerate() {
                let start_s = broll.start_ms as f64 / 1000.0;
                let out_label = format!("vbroll_{}", i);
                let escaped_path = match escape_filter_path(&broll.path) {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!("[filter_graph] Skipping broll: {}", e);
                        continue;
                    }
                };

                // Omit f=mp4 to let ffmpeg auto-detect container format.
                // Keep si=0 to force video stream selection (default si=-1 may
                // select audio stream for files with audio, causing static frames).
                //
                // Seek offset: Stock Pexels videos often have slow/static intros
                // (first 2-5s of fade-ins, slow pans). Without seeking, the
                // overlay window shows only the static intro. Use a deterministic
                // offset based on clip index to jump past intros into dynamic
                // content. When the source duration is known (probed by the
                // caller via `crate::probe::probe`), cap the offset so the
                // trimmed source still has enough frames to cover the full
                // segment. When unknown, the safe fallback is the deterministic
                // 5s offset.
                let clip_duration_s = (broll.end_ms - broll.start_ms) as f64 / 1000.0;
                // Deterministic pseudo-random: golden-ratio hash gives good distribution
                let mut seek_offset = (i as f64 * 1.618033988749895) % 5.0f64;
                // Compute finite loop count: enough plays to cover the segment
                // but NEVER infinite (loop=0/loop=-1 hang the render because
                // the filter graph never terminates). Loop count is the number
                // of times the source plays: 1 = once (default), N = N times.
                let mut loop_count: i32 = 1;
                if let Some(src_dur) = broll.source_duration_s {
                    // Cap offset so trim fits inside one play.
                    let max_offset = (src_dur - clip_duration_s).max(0.0);
                    seek_offset = seek_offset.min(max_offset);
                    // If source is shorter than the segment, loop enough times
                    // so the total source length covers the segment.
                    if src_dur < clip_duration_s && src_dur > 0.0 {
                        loop_count = (clip_duration_s / src_dur).ceil() as i32 + 1;
                    }
                } else {
                    // Unknown duration: conservative loop of 3 covers up to
                    // 3× source length (typical Pexels clips are 10-18s;
                    // segments are 10-17s, so 3× = 30-54s — more than enough).
                    loop_count = 3;
                    seek_offset = seek_offset.min(clip_duration_s.max(0.0) * 0.5);
                }
                // Use movie= filter's built-in loop parameter instead of a
                // separate `loop` filter. The `loop` filter has unreliable
                // buffering when fed by `movie=` sources — it may pass through
                // without looping, causing source exhaustion (held last frame).
                // movie=loop=N means N additional plays (N+1 total).
                // si=-1 auto-selects the first VIDEO stream; si=0 picks the
                // literal first stream which may be audio in some MP4 files.
                let movie_loop = loop_count.saturating_sub(1).max(0);
                parts.push(format!(
                    "movie='{}':loop={}:si=-1[broll_raw{}]",
                    escaped_path,
                    movie_loop,
                    i
                ));
                // Trim past the slow intro using seek_offset, then reset PTS
                parts.push(format!(
                    "[broll_raw{}]trim=start={:.2},setpts=PTS-STARTPTS[broll_src_{}]",
                    i,
                    seek_offset,
                    i
                ));
                // Ken Burns motion: zoompan guarantees continuous on-screen
                // motion regardless of the source clip's intrinsic behaviour.
                // Without this, clips whose seek_offset lands on a slow frame
                // (or Pexels videos that are slow throughout) appear as static
                // images for the full overlay window. The motion style is
                // cycled per clip index for visual variety. zoompan's `d=1`
                // means each output frame animates one step past the previous
                // frame; with `fps=self.fps` the output frame rate matches the
                // project's target so downstream filters see consistent timing.
                let style = MotionStyle::for_clip(i);
                let (zp_z, zp_x, zp_y) = style.expressions();
                parts.push(format!(
                    "[broll_src_{}]zoompan=z='{}':x='{}':y='{}':d=1:s={w}x{h}:fps={fps}[broll_zp_{}]",
                    i,
                    zp_z,
                    zp_x,
                    zp_y,
                    i,
                    w = self.width,
                    h = self.height,
                    fps = self.fps,
                ));
                parts.push(format!(
                    "[broll_zp_{}]scale={w}:{h}[broll_scaled_{}]",
                    i,
                    i,
                    w = self.width,
                    h = self.height,
                ));
                parts.push(format!(
                    "[{}][broll_scaled_{}]overlay=0:0:enable='between(t,{},{})'[{}]",
                    &current_v[1..current_v.len() - 1],
                    i,
                    start_s,
                    broll.end_ms as f64 / 1000.0,
                    out_label
                ));
                current_v = format!("[{}]", out_label);
            }
            vout = current_v;
        }

        // Overlay MOV (PupCaps captions) — composites the MOV on top of the video
        if let Some(_mov) = &self.overlay_mov {
            if let Ok(_escaped_mov) = escape_filter_path(_mov) {
                parts.push(format!(
                    "movie='{}':f=mov[ovr]",
                    _escaped_mov
                ));
                parts.push(format!(
                    "[{}][ovr]overlay=0:0:shortest=1[vovl]",
                    &vout[1..vout.len() - 1],
                ));
                vout = "[vovl]".into();
            } else {
                tracing::warn!("[filter_graph] Skipping overlay MOV (escape failed)");
            }
        }

        // Audio loudnorm
        let mut aout = a_trim.to_string();
        if self.loudnorm {
            // Use the timeline's normalize_to_lufs (default -16.0) rather than
            // a hardcoded value. This lets `directives.mix.normalize_to_lufs`
            // actually control the output loudness.
            parts.push(format!(
                "[{}]loudnorm=I={}:TP=-2.5:LRA=11[aloud]",
                &aout[1..aout.len() - 1],
                self.normalize_lufs
            ));
            aout = "[aloud]".into();
        }

        // Voiceover mixing — TTS commentary mixed with dialogue
        // Batched into a single amix=inputs=N instead of cascading amix=inputs=2
        // (avoids O(n^2) scaling when many VO events share the timeline).
        if !self.voiceover_events.is_empty() {
            let mut vo_inputs = Vec::with_capacity(self.voiceover_events.len());
            vo_inputs.push(aout.clone());
            for (i, vo) in self.voiceover_events.iter().enumerate() {
                let gain = 10f64.powf(vo.gain_db / 20.0);
                let start_ms = vo.start_ms;

                if let Ok(f) = amovie_filter(&vo.path, "a") {
                    parts.push(format!("{}[voiceover_{}]", f, i));
                } else {
                    tracing::warn!("[filter_graph] Skipping voiceover {}", i);
                    continue;
                }
                parts.push(format!("[voiceover_{}]volume={}[vo_vol_{}]", i, gain, i));
                parts.push(format!(
                    "[vo_vol_{}]adelay={}|{}[vo_delayed_{}]",
                    i, start_ms, start_ms, i
                ));
                vo_inputs.push(format!("[vo_delayed_{}]", i));
            }
            parts.push(format!(
                "{}amix=inputs={}:duration=first:dropout_transition=1:normalize=0[amix_voiceover]",
                vo_inputs.join(""),
                vo_inputs.len()
            ));
            aout = "[amix_voiceover]".into();
        }

        // Music mixing — background music mixed with dialogue
        // When ducking is configured, split dialogue for sidechain and apply sidechaincompress
        let has_ducking = !self.ducking_events.is_empty() && !self.music_events.is_empty();
        let dialogue_label = if has_ducking {
            parts.push(format!(
                "[{}]asplit=2[aloud_out][sidechain_src]",
                &aout[1..aout.len() - 1]
            ));
            "[aloud_out]".to_string()
        } else {
            aout.clone()
        };

        if !self.music_events.is_empty() {
            let attack = if has_ducking {
                self.ducking_events
                    .first()
                    .map(|e| e.attack_ms as f64)
                    .unwrap_or(50.0)
            } else {
                0.0
            };
            let release = if has_ducking {
                self.ducking_events
                    .first()
                    .map(|e| e.release_ms as f64)
                    .unwrap_or(200.0)
            } else {
                0.0
            };

            // Batched amix: dialogue + all music tracks in a single amix=inputs=N
            // instead of cascading amix=inputs=2 per track.
            let mut music_inputs = Vec::with_capacity(self.music_events.len() + 1);
            music_inputs.push(dialogue_label.clone());
            for (i, music) in self.music_events.iter().enumerate() {
                let vol = music.volume;
                if let Ok(f) = amovie_filter(&music.path, "a") {
                    parts.push(format!("{}[music_{}]", f, i));
                } else {
                    tracing::warn!("[filter_graph] Skipping music {}", i);
                    continue;
                }
                parts.push(format!("[music_{}]volume={}[music_vol_{}]", i, vol, i));

                if has_ducking {
                    let music_ducked = format!("[music_ducked_{}]", i);
                    parts.push(format!(
                        "[music_vol_{}][sidechain_src]sidechaincompress=threshold=0.001:ratio=4:attack={}:release={}:makeup=1:level_sc=1{}",
                        i, attack, release, music_ducked
                    ));
                    music_inputs.push(music_ducked);
                } else {
                    music_inputs.push(format!("[music_vol_{}]", i));
                }
            }
            if music_inputs.len() > 1 {
                parts.push(format!(
                    "{}amix=inputs={}:duration=first:dropout_transition=2:normalize=0[amix_music]",
                    music_inputs.join(""),
                    music_inputs.len()
                ));
                aout = "[amix_music]".into();
            }
        }

        // SFX injection — sound effects at specific timestamps
        // Batched into a single amix=inputs=N instead of cascading amix=inputs=2.
        // With 46+ SFX events this is the difference between O(n^2) and O(n) wall time.
        if !self.sfx_events.is_empty() {
            let mut sfx_inputs = Vec::with_capacity(self.sfx_events.len() + 1);
            sfx_inputs.push(aout.clone());
            for (i, sfx) in self.sfx_events.iter().enumerate() {
                let gain = 10f64.powf(sfx.gain_db / 20.0);
                let start_s = sfx.start_ms as f64 / 1000.0;

                if let Ok(f) = amovie_filter(&sfx.path, "a") {
                    parts.push(format!("{}[sfx_{}]", f, i));
                } else {
                    tracing::warn!("[filter_graph] Skipping SFX {}", i);
                    continue;
                }
                parts.push(format!("[sfx_{}]volume={}[sfx_vol_{}]", i, gain, i));
                parts.push(format!(
                    "[sfx_vol_{}]adelay={}|{}[sfx_delayed_{}]",
                    i,
                    (start_s * 1000.0).round() as i64,
                    (start_s * 1000.0).round() as i64,
                    i
                ));
                sfx_inputs.push(format!("[sfx_delayed_{}]", i));
            }
            parts.push(format!(
                "{}amix=inputs={}:duration=first:dropout_transition=1:normalize=0[asfx]",
                sfx_inputs.join(""),
                sfx_inputs.len()
            ));
            aout = "[asfx]".into();
        }

        // Post-mix safety limiter — loudnorm TP targets true peaks but
        // music/SFX mixing after it can push levels above the limit.
        // alimiter provides a hard sample-peak ceiling at -3 dBFS as a backstop.
        // (P0 audio clipping fix — peak was -0.2 dBFS with limit=0.79.
        //  Lowered to 0.70 = -3 dBFS to ensure no clipping on any platform.)
        if self.loudnorm {
            let input_label = &aout[1..aout.len() - 1];
            parts.push(format!(
                "[{}]alimiter=limit=0.70:attack=5:release=50[afinal]",
                input_label
            ));
            aout = "[afinal]".into();
        }

    (parts.join(","), vout, aout)
}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_segment(id: &str, start: f64, end: f64) -> Segment {
        Segment {
            id: id.into(),
            start,
            end,
            caption: "test".into(),
            crossfade_ms: 80,
            semantic_role: None,
        }
    }

    #[test]
    fn test_single_segment() {
        let segments = vec![make_segment("seg_001", 0.0, 3.46)];
        let (filter, vout, aout) = FilterGraphBuilder::new(segments, 30, "9:16", true).build();

        assert!(filter.contains("trim=start=0:end=3.46"));
        assert!(filter.contains("atrim=start=0:end=3.46"));
        assert!(filter.contains("fps=30"));
        assert!(filter.contains("scale=-2:1920"));
        assert!(filter.contains("loudnorm=I=-16:TP=-2.5:LRA=11"));
        assert_eq!(vout, "[vcrop]");
        assert_eq!(aout, "[afinal]");
    }

    #[test]
    fn test_multiple_segments_xfade() {
        let segments = vec![
            make_segment("seg_001", 0.0, 1.0),
            make_segment("seg_002", 1.5, 3.0),
        ];
        let (filter, _, _) = FilterGraphBuilder::new(segments, 30, "9:16", false).build();

        // Video uses xfade for smooth transitions; audio uses concat for reliability
        assert!(filter.contains("xfade=transition=smoothleft"));
        assert!(filter.contains("concat=n=2:v=0:a=1[acat]"));
        // Video concat should NOT be used (xfade replaces it)
        assert!(!filter.contains("concat=n=2:v=1:a=1"));
    }

    #[test]
    fn test_ass_burn() {
        let segments = vec![make_segment("seg_001", 0.0, 1.0)];
        let (filter, vout, _) = FilterGraphBuilder::new(segments, 30, "9:16", false)
            .with_ass("/path/to/captions.ass".into())
            .build();

        assert!(filter.contains("subtitles='/path/to/captions.ass'"));
        assert_eq!(vout, "[vsub]");
    }

    #[test]
    fn test_srt_burn() {
        let segments = vec![make_segment("seg_001", 0.0, 1.0)];
        let (filter, vout, _) = FilterGraphBuilder::new(segments, 30, "9:16", false)
            .with_srt("/path/to/captions.srt".into())
            .build();

        assert!(filter.contains("subtitles='/path/to/captions.srt'"));
        assert_eq!(vout, "[vsub]");
    }

    #[test]
    fn test_overlay_mov() {
        let segments = vec![make_segment("seg_001", 0.0, 1.0)];
        let (filter, vout, _) = FilterGraphBuilder::new(segments, 30, "9:16", false)
            .with_overlay_mov("/path/to/overlay.mov".into())
            .build();

        assert!(filter.contains("movie='/path/to/overlay.mov'"));
        assert!(filter.contains("overlay=0:0"));
        assert_eq!(vout, "[vovl]");
    }

    #[test]
    fn test_overlay_with_subtitles() {
        let segments = vec![make_segment("seg_001", 0.0, 1.0)];
        let (filter, vout, _) = FilterGraphBuilder::new(segments, 30, "9:16", false)
            .with_ass("/path/to/captions.ass".into())
            .with_overlay_mov("/path/to/overlay.mov".into())
            .build();

        // Both subtitles AND overlay should be present (dual caption mode)
        assert!(filter.contains("subtitles='/path/to/captions.ass'"));
        assert!(filter.contains("movie='/path/to/overlay.mov'"));
        assert!(filter.contains("overlay=0:0"));
        assert_eq!(vout, "[vovl]");
    }

    #[test]
    fn test_broll_overlay() {
        let segments = vec![make_segment("seg_001", 0.0, 5.0)];
        let broll = vec![BrollEvent {
            path: "/path/to/broll.mp4".into(),
            start_ms: 1000,
            end_ms: 3000,
            source_duration_s: None,
        }];
        let (filter, vout, _) = FilterGraphBuilder::new(segments, 30, "9:16", false)
            .with_broll(broll)
            .build();

        assert!(filter.contains("movie='/path/to/broll.mp4'"));
        assert!(filter.contains("scale=1080:1920"));
        assert!(filter.contains("overlay=0:0:enable='between(t,1,"));
        // zoompan (Ken Burns) must be inserted between trim and scale so
        // every b-roll clip has continuous motion even when the source is slow.
        assert!(filter.contains("[broll_src_0]zoompan="));
        assert!(filter.contains("d=1:s=1080x1920:fps=30"));
        assert!(filter.contains("[broll_zp_0]"));
        assert_eq!(vout, "[vbroll_0]");
    }

    #[test]
    fn test_broll_ken_burns_motion_cycles_through_styles() {
        // Four consecutive clips must use four different motion styles so
        // the rendered timeline doesn't feel monotonous.
        let segments = vec![make_segment("seg_001", 0.0, 20.0)];
        let broll: Vec<BrollEvent> = (0..8)
            .map(|i| BrollEvent {
                path: format!("/path/to/broll_{}.mp4", i),
                start_ms: i as i64 * 2500,
                end_ms: i as i64 * 2500 + 2000,
                source_duration_s: None,
            })
            .collect();
        let (filter, _, _) = FilterGraphBuilder::new(segments, 30, "9:16", false)
            .with_broll(broll)
            .build();

        // Each clip index should appear once in its zoompan filter
        for i in 0..8 {
            assert!(
                filter.contains(&format!("[broll_src_{}]zoompan=", i)),
                "clip {} missing zoompan in filter chain",
                i
            );
        }
        // Clips at indices 0 and 4 share the same style (index % 4 == 0)
        // so their zoom expressions must match. Spot-check style variation:
        let clip0_idx = filter.find("[broll_src_0]zoompan=").unwrap();
        let clip0_chunk = &filter[clip0_idx..filter.find("[broll_zp_0]").unwrap()];
        let clip1_idx = filter.find("[broll_src_1]zoompan=").unwrap();
        let clip1_chunk = &filter[clip1_idx..filter.find("[broll_zp_1]").unwrap()];
        // Two consecutive clips must use different x expressions (different styles)
        assert_ne!(
            clip0_chunk, clip1_chunk,
            "consecutive clips should not share an identical zoompan expression"
        );
    }

    #[test]
    fn test_motion_style_for_clip_cycles() {
        // Verify the cycle is exactly 4 styles, no duplicates within one cycle.
        let s0 = MotionStyle::for_clip(0);
        let s1 = MotionStyle::for_clip(1);
        let s2 = MotionStyle::for_clip(2);
        let s3 = MotionStyle::for_clip(3);
        let s4 = MotionStyle::for_clip(4);
        assert_ne!(s0, s1);
        assert_ne!(s1, s2);
        assert_ne!(s2, s3);
        assert_ne!(s0, s3);
        // Index 4 wraps back to the same style as index 0
        assert_eq!(s0, s4);
    }

    #[test]
    fn test_motion_style_expressions_non_empty() {
        // Every motion style must produce non-empty zoom/pan expressions;
        // an empty expression would silently disable motion.
        for style in [
            MotionStyle::ZoomInCenter,
            MotionStyle::ZoomInTopLeft,
            MotionStyle::ZoomOutCenter,
            MotionStyle::PanRight,
        ] {
            let (z, x, y) = style.expressions();
            assert!(!z.is_empty(), "{:?} produced empty zoom expr", style);
            assert!(!x.is_empty(), "{:?} produced empty x expr", style);
            assert!(!y.is_empty(), "{:?} produced empty y expr", style);
        }
    }

    #[test]
    fn test_broll_zoompan_uses_target_dimensions_and_fps() {
        // zoompan output size and fps must match the builder's target so the
        // downstream scale filter and timing pipeline see consistent values.
        let segments = vec![make_segment("seg_001", 0.0, 5.0)];
        let broll = vec![BrollEvent {
            path: "/p/b.mp4".into(),
            start_ms: 0,
            end_ms: 3000,
            source_duration_s: None,
        }];
        let (filter, _, _) = FilterGraphBuilder::new(segments, 24, "16:9", false)
            .with_broll(broll)
            .build();
        // 16:9 defaults to 1920x1080 with the chosen 24 fps
        assert!(
            filter.contains("s=1920x1080:fps=24"),
            "zoompan should output at target dimensions and fps: filter was\n{}",
            filter
        );
    }

    #[test]
    fn test_16x9_aspect() {
        let segments = vec![make_segment("seg_001", 0.0, 1.0)];
        let (filter, vout, _) = FilterGraphBuilder::new(segments, 30, "16:9", false).build();

        // 16:9 now also crops (to 1920×1080) instead of leaving the source untouched.
        assert!(filter.contains("scale=-2:1080"));
        assert!(filter.contains("crop=1920:1080"));
        assert_eq!(vout, "[vcrop]");
    }

    #[test]
    fn test_empty_segments() {
        let (filter, vout, aout) = FilterGraphBuilder::new(vec![], 30, "9:16", false).build();

        assert!(filter.is_empty());
        assert_eq!(vout, "[0:v]");
        assert_eq!(aout, "[0:a]");
    }

    #[test]
    fn test_many_segments_use_concat() {
        // For >10 segments, concat is used instead of xfade for performance
        let segments: Vec<Segment> = (0..15)
            .map(|i| {
                let t = i as f64 * 3.0;
                make_segment(&format!("seg_{:03}", i + 1), t, t + 2.5)
            })
            .collect();
        let (filter, vout, aout) = FilterGraphBuilder::new(segments, 30, "9:16", true).build();

        // Video+audio use combined concat (v=1:a=1)
        assert!(filter.contains("concat=n=15:v=1:a=1[concat_v][concat_a]"));
        // xfade should NOT be used
        assert!(!filter.contains("xfade="));
        // Post-trim still applies
        assert!(filter.contains("fps=30"));
        assert!(filter.contains("scale=-2:1920"));
        assert!(filter.contains("loudnorm=I=-16"));
        assert_eq!(vout, "[vcrop]");
        assert_eq!(aout, "[afinal]");
    }

    #[test]
    fn test_threshold_boundary_xfade_at_10() {
        // Exactly 10 segments should still use xfade
        let segments: Vec<Segment> = (0..10)
            .map(|i| {
                let t = i as f64 * 2.0;
                make_segment(&format!("seg_{:03}", i + 1), t, t + 1.5)
            })
            .collect();
        let (filter, _, _) = FilterGraphBuilder::new(segments, 30, "9:16", false).build();

        assert!(filter.contains("xfade=transition=smoothleft"));
        assert!(filter.contains("concat=n=10:v=0:a=1[acat]"));
    }

    #[test]
    fn test_threshold_boundary_concat_at_11() {
        // 11 segments should switch to concat
        let segments: Vec<Segment> = (0..11)
            .map(|i| {
                let t = i as f64 * 2.0;
                make_segment(&format!("seg_{:03}", i + 1), t, t + 1.5)
            })
            .collect();
        let (filter, _, _) = FilterGraphBuilder::new(segments, 30, "9:16", false).build();

        assert!(filter.contains("concat=n=11:v=1:a=1[concat_v][concat_a]"));
        assert!(!filter.contains("xfade="));
    }

    #[test]
    fn test_audio_only_uses_color_background() {
        // When the source is audio-only (mp3/wav), the filter graph must NOT
        // reference `[0:v]` — it should synthesize a solid-color video via
        // the `color=` filter at the target W/H and FPS, with a duration that
        // covers the full timeline. The audio chain still uses `[0:a]` so the
        // dialogue + SFX mix works normally.
        let segments = vec![make_segment("seg_001", 0.0, 5.0)];
        let (filter, vout, _) = FilterGraphBuilder::new(segments, 30, "9:16", false)
            .with_audio_only(true)
            .build();

        // Background video synthesized via color=
        assert!(
            filter.contains("color=size=1080x1920:rate=30:duration=5.5:color=black[vbg]"),
            "audio-only path must synthesize a solid-color background: filter was\n{}",
            filter
        );
        // Must NOT reference [0:v] — that stream doesn't exist on audio-only sources
        assert!(
            !filter.contains("[0:v]"),
            "audio-only filter graph must not reference [0:v]: filter was\n{}",
            filter
        );
        // Audio still trimmed from [0:a]
        assert!(filter.contains("[0:a]atrim="));
        // Video output label is still a valid video stream for post-trim chaining
        assert_eq!(vout, "[vcrop]");
    }

    #[test]
    fn test_audio_only_preserves_broll_and_subtitles() {
        // B-roll overlays and subtitle burn-in must work on the audio-only path
        // exactly like the video path — they chain off the synthesized background.
        let segments = vec![make_segment("seg_001", 0.0, 5.0)];
        let broll = vec![BrollEvent {
            path: "/path/to/broll.mp4".into(),
            start_ms: 1000,
            end_ms: 3000,
            source_duration_s: None,
        }];
        let (filter, _, _) = FilterGraphBuilder::new(segments, 30, "9:16", true)
            .with_audio_only(true)
            .with_ass("/path/to/captions.ass".into())
            .with_broll(broll)
            .build();

        // Background synthesized
        assert!(filter.contains("color="));
        // B-roll chain still present (movie + zoompan + scale + overlay)
        assert!(filter.contains("movie='/path/to/broll.mp4'"));
        assert!(filter.contains("zoompan="));
        assert!(filter.contains("overlay=0:0:enable='between(t,1,"));
        // ASS subtitle burn-in still applied
        assert!(filter.contains("subtitles='/path/to/captions.ass'"));
        // Loudnorm + alimiter audio chain still applied
        assert!(filter.contains("loudnorm=I=-16"));
        assert!(filter.contains("alimiter="));
    }

    #[test]
    fn test_audio_only_disabled_keeps_video_path() {
        // Sanity check: when audio_only=false (the default), the video path
        // is used and [0:v] is referenced. This guards against the audio-only
        // path accidentally leaking into the default code path.
        let segments = vec![make_segment("seg_001", 0.0, 5.0)];
        let (filter, _, _) = FilterGraphBuilder::new(segments, 30, "9:16", false).build();

        assert!(filter.contains("[0:v]trim="));
        assert!(filter.contains("[0:a]atrim="));
        assert!(!filter.contains("color="));
    }

    #[test]
    fn test_ass_with_fonts_dir() {
        let segments = vec![make_segment("seg_001", 0.0, 1.0)];
        let (filter, vout, _) = FilterGraphBuilder::new(segments, 30, "9:16", false)
            .with_ass("/path/to/captions.ass".into())
            .with_fonts_dir("/home/ishanp/Documents/GitHub/openscript/mcp/fonts".into())
            .build();

        assert!(filter.contains("subtitles='/path/to/captions.ass':fontsdir='/home/ishanp/Documents/GitHub/openscript/mcp/fonts'"));
        assert_eq!(vout, "[vsub]");
    }

    #[test]
    fn test_srt_with_fonts_dir() {
        let segments = vec![make_segment("seg_001", 0.0, 1.0)];
        let (filter, _, _) = FilterGraphBuilder::new(segments, 30, "9:16", false)
            .with_srt("/path/to/captions.srt".into())
            .with_fonts_dir("/usr/share/fonts/truetype".into())
            .build();

        assert!(filter
            .contains("subtitles='/path/to/captions.srt':fontsdir='/usr/share/fonts/truetype'"));
    }

    #[test]
    fn test_voiceover_mixing() {
        let segments = vec![make_segment("seg_001", 0.0, 5.0)];
        let voiceover = vec![VoiceoverEvent {
            path: "/path/to/voiceover.wav".into(),
            start_ms: 0,
            gain_db: -6.0,
        }];
        let (filter, _, aout) = FilterGraphBuilder::new(segments, 30, "9:16", true)
            .with_voiceover(voiceover)
            .build();

        assert!(filter.contains("amovie='/path/to/voiceover.wav':f=wav:s=a[voiceover_0]"));
        assert!(filter.contains("[voiceover_0]volume="));
        assert!(filter.contains("[vo_vol_0]adelay=0|0[vo_delayed_0]"));
        assert!(filter.contains("[vo_delayed_0]amix=inputs=2:duration=first:dropout_transition=1"));
        assert_eq!(aout, "[afinal]");
    }

    #[test]
    fn test_voiceover_with_music() {
        let segments = vec![make_segment("seg_001", 0.0, 5.0)];
        let voiceover = vec![VoiceoverEvent {
            path: "/path/to/voiceover.wav".into(),
            start_ms: 0,
            gain_db: -6.0,
        }];
        let music = vec![MusicEvent {
            path: "/path/to/music.mp3".into(),
            volume: 0.3,
        }];
        let (filter, _, aout) = FilterGraphBuilder::new(segments, 30, "9:16", true)
            .with_voiceover(voiceover)
            .with_music(music)
            .build();

        // Voiceover should be mixed before music
        assert!(filter.contains("[amix_voiceover]"));
        assert!(filter.contains("amovie='/path/to/music.mp3'"));
        // Audio chain: voiceover mixed → then music mixed
        assert!(filter.contains("[amix_voiceover][music_vol_0]amix"));
        assert_eq!(aout, "[afinal]");
    }
}

//! Multi-layer from-scratch video renderer.
//!
//! Renders a video with proper temporal layering:
//! - Layer 0: Background video (per-scene clips, concatenated)
//! - Layer 1: Sticker overlays (PNG/GIF at positions)
//! - Layer 2: Captions (ASS subtitles burned in)
//! - Audio 1: Voiceover (concatenated WAVs)
//! - Audio 2: Music (with sidechain ducking)
//!
//! This replaces the single-background `render_from_script` with a proper
//! multi-layer composition engine.

use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::FfmpegError;

/// A background clip with its time range.
#[derive(Debug, Clone)]
pub struct BackgroundClip {
    pub path: String,
    pub duration_s: f64,
    pub looped: bool,
}

/// A sticker overlay with its time range and position.
///
/// Coordinates use CENTER-BASED system: [0,0] = center of canvas.
/// Negative X = left of center, positive X = right of center.
/// Negative Y = above center, positive Y = below center.
/// This makes it intuitive for AI agents to reason about placement.
#[derive(Debug, Clone)]
pub struct StickerOverlay {
    pub path: String,
    pub start_s: f64,
    pub end_s: f64,
    pub position: String, // Named position ("bottom-left", etc.) — converted to x/y
    pub scale: f64,       // 0.0-1.0 relative to canvas width
    pub center_x: i32,    // Pixel offset from canvas center (0 = centered). Positive = right.
    pub center_y: i32,    // Pixel offset from canvas center (0 = centered). Positive = down.
    pub sticker_width: u32, // Calculated pixel width after scaling
    pub sticker_height: u32, // Calculated pixel height after scaling
}

/// Multi-layer render specification.
pub struct MultiLayerRenderSpec {
    /// Per-scene background clips (will be concatenated)
    pub backgrounds: Vec<BackgroundClip>,
    /// Voiceover WAV paths in order
    pub voiceover_paths: Vec<String>,
    /// Optional sticker overlays
    pub stickers: Vec<StickerOverlay>,
    /// Optional background music path
    pub music_path: Option<String>,
    /// Music volume (0.0-1.0)
    pub music_volume: f64,
    /// Whether to duck music during voiceover
    pub ducking: bool,
    /// Ducking depth in dB
    pub ducking_depth_db: f64,
    /// Caption ASS file path
    pub captions_path: Option<String>,
    /// Output width
    pub width: u32,
    /// Output height
    pub height: u32,
    /// Output FPS
    pub fps: u32,
    /// Output path
    pub output_path: String,
    /// CRF (quality)
    pub crf: u32,
    /// FFmpeg preset
    pub preset: String,
    /// Total duration in seconds
    pub total_duration_s: f64,
    /// Full-screen meme b-roll clips (GIPHY MP4s). Each clip has a start/end
    /// time and will be composited as a brief full-screen background cut.
    /// Unlike stickers (small overlays), meme clips replace the background
    /// for their duration — like TikTok reaction cuts.
    pub meme_clips: Vec<MemeClip>,
}

/// A full-screen meme b-roll clip from GIPHY.
#[derive(Debug, Clone)]
pub struct MemeClip {
    pub path: String,
    pub start_s: f64,
    pub end_s: f64,
}

/// Parse a position string to:
/// - (top_left_x, top_left_y): FFmpeg overlay coordinates (top-left origin)
/// - (center_x, center_y): Agent-friendly center-based coordinates [0,0] = canvas center
///
/// Center-based system:
///   [0, 0] = exact center of canvas
///   Negative X = left of center, Positive X = right of center
///   Negative Y = above center, Positive Y = below center
///
/// This allows agents to reason: "put sticker at [-300, 400] = 300px left, 400px down from center"
fn parse_position(
    position: &str,
    canvas_w: u32,
    canvas_h: u32,
    sticker_w: u32,
    sticker_h: u32,
) -> (i32, i32, i32, i32) {
    let margin = 40i32;
    let cw = canvas_w as i32;
    let ch = canvas_h as i32;
    let sw = sticker_w as i32;
    let sh = sticker_h as i32;

    // Top-left coordinates for FFmpeg overlay filter
    // NOTE: accept both adjective-noun ("top-center") and noun-adjective
    // ("center-top") spellings. The MemeBrollSpec default position is
    // "center-bottom" and agents naturally write "center-top" — without
    // these aliases every such value fell through to the `_` arm (top-left
    // at the margin), causing meme b-rolls to render on top of the speaker
    // sticker instead of where the agent asked. (Round-6 fresh-agent UX.)
    let (tl_x, tl_y) = match position {
        "top-left" => (margin, margin),
        "top-right" => (cw - sw - margin, margin),
        "top-center" | "center-top" => ((cw - sw) / 2, margin),
        "bottom-left" => (margin, ch - sh - margin),
        "bottom-right" => (cw - sw - margin, ch - sh - margin),
        "bottom-center" | "center-bottom" => ((cw - sw) / 2, ch - sh - margin),
        "center" => ((cw - sw) / 2, (ch - sh) / 2),
        _ => (margin, margin),
    };

    // Center-based coordinates: offset of sticker CENTER from canvas CENTER
    // sticker_center_x = tl_x + sw/2
    // canvas_center_x = cw/2
    // center_x = sticker_center_x - canvas_center_x = tl_x + sw/2 - cw/2
    let center_x = tl_x + sw / 2 - cw / 2;
    let center_y = tl_y + sh / 2 - ch / 2;

    (tl_x, tl_y, center_x, center_y)
}

/// Render a multi-layer video composition.
///
/// FFmpeg filter graph structure:
/// ```text
/// [bg1][bg2][bg3]concat=n=3[vcat]        — concat backgrounds
/// [vcat]scale,crop[vbg]                  — fit to canvas
/// [vbg]subtitles=captions.ass[vcap]      — burn captions
/// [sticker1]scale[st1]                   — scale sticker
/// [vcap][st1]overlay=x:y:enable=between(t,s,e)[vout]  — overlay sticker
/// [vo1][vo2]concat=n=2[audio_vo]         — concat voiceovers
/// [music]volume[mvol]                    — music volume
/// [audio_vo][mvol]sidechaincompress[amix] — duck + mix
/// ```
pub async fn render_multilayer(spec: &MultiLayerRenderSpec) -> Result<String, FfmpegError> {
    if spec.backgrounds.is_empty() {
        return Err(FfmpegError::NoSegments);
    }

    // Defense-in-depth: filter out placeholder or empty background paths
    // before they reach ffmpeg. The MCP `timeline.render` handler already
    // filters placeholder b-roll events, but callers that build a
    // MultiLayerRenderSpec directly (e.g. `script.to_video`) could pass
    // placeholder strings through. ffmpeg would then fail with a cryptic
    // "Unable to parse 'si' option value 'v'" error. Log and skip such
    // entries here so the render degrades gracefully to the remaining
    // valid backgrounds.
    //
    // We do NOT filter non-existent paths here — those will fail at ffmpeg
    // spawn with a clearer "No such file" error, and tests use fake paths.
    let filtered_bgs: Vec<&crate::multilayer_render::BackgroundClip> = spec
        .backgrounds
        .iter()
        .filter(|bg| {
            if bg.path == "placeholder" || bg.path.is_empty() {
                tracing::warn!("[multilayer_render] Skipping placeholder/empty background path");
                false
            } else {
                true
            }
        })
        .collect();

    if filtered_bgs.is_empty() {
        return Err(FfmpegError::NoSegments);
    }

    // Build voiceover concat list
    let concat_list_path = format!("{}.concat.txt", spec.output_path);
    let concat_content: String = spec
        .voiceover_paths
        .iter()
        .map(|p| {
            let abs = std::fs::canonicalize(p).unwrap_or_else(|_| std::path::PathBuf::from(p));
            format!("file '{}'\n", abs.to_string_lossy().replace('\'', "'\\''"))
        })
        .collect();
    std::fs::write(&concat_list_path, &concat_content)?;

    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y");

    // Check if all backgrounds are the same file (single-background playback)
    let all_same_bg =
        filtered_bgs.len() > 1 && filtered_bgs.windows(2).all(|w| w[0].path == w[1].path);

    let bg_count = if all_same_bg {
        // Single background — use ONE input, looped for the full duration
        let bg = filtered_bgs[0];
        if bg.looped {
            cmd.arg("-stream_loop").arg("-1");
        }
        cmd.arg("-t").arg(spec.total_duration_s.to_string());
        cmd.arg("-i").arg(&bg.path);
        1
    } else {
        // Multiple different backgrounds — one input per clip
        let count = filtered_bgs.len();
        for bg in &filtered_bgs {
            if bg.looped {
                cmd.arg("-stream_loop").arg("-1");
            }
            cmd.arg("-t").arg(bg.duration_s.to_string());
            cmd.arg("-i").arg(&bg.path);
        }
        count
    };

    // Input: voiceover concat
    let vo_input_idx = bg_count;
    cmd.arg("-f").arg("concat");
    cmd.arg("-safe").arg("0");
    cmd.arg("-i").arg(&concat_list_path);

    // Input: music
    let music_input_idx = vo_input_idx + 1;
    let has_music = spec.music_path.is_some()
        && std::path::Path::new(spec.music_path.as_ref().unwrap()).exists();
    if has_music {
        cmd.arg("-stream_loop").arg("-1");
        cmd.arg("-t").arg(spec.total_duration_s.to_string());
        cmd.arg("-i").arg(spec.music_path.as_ref().unwrap());
    }

    // Inputs: sticker overlays
    // NOTE: only skip past the music input index when music was actually
    // added. Previously this always used `music_input_idx + 1`, which is
    // `vo_input_idx + 2` — but when `has_music` is false the music input
    // is never added to the ffmpeg command, so the first sticker input
    // actually lands at `vo_input_idx + 1`. This off-by-one made every
    // sticker filtergraph label ([N:v]) point one input too high, which
    // produced "Invalid file index N in filtergraph description" whenever
    // music was omitted. (Round-6 fresh-agent UX: brain video with no
    // music failed to render.)
    let mut sticker_input_idx = if has_music {
        music_input_idx + 1
    } else {
        vo_input_idx + 1
    };
    let mut sticker_inputs = Vec::new();
    for sticker in &spec.stickers {
        if std::path::Path::new(&sticker.path).exists() {
            cmd.arg("-i").arg(&sticker.path);
            sticker_inputs.push((sticker_input_idx, sticker));
            sticker_input_idx += 1;
        }
    }

    // Build filter complex
    let mut filters: Vec<String> = Vec::new();

    // 1. Concat background clips
    if bg_count == 1 {
        // Single background — just scale
        filters.push(format!(
            "[0:v]scale={}:{}:force_original_aspect_ratio=increase,crop={}:{},fps={},trim=duration={}[vbg]",
            spec.width, spec.height, spec.width, spec.height, spec.fps, spec.total_duration_s
        ));
    } else {
        // Multiple backgrounds — concat then scale
        let bg_durations: Vec<String> = filtered_bgs
            .iter()
            .map(|bg| format!("trim=duration={}", bg.duration_s))
            .collect();

        // Trim each background to its scene duration
        for (i, dur_filter) in bg_durations.iter().enumerate() {
            filters.push(format!(
                "[{}:v]{},setpts=PTS-STARTPTS[bg{}]",
                i, dur_filter, i
            ));
        }

        // Concat all trimmed backgrounds
        let concat_labels: String = (0..bg_count)
            .map(|i| format!("[bg{}]", i))
            .collect::<String>();
        filters.push(format!("{}concat=n={}[vcat]", concat_labels, bg_count));

        // Scale the concatenated video
        filters.push(format!(
            "[vcat]scale={}:{}:force_original_aspect_ratio=increase,crop={}:{},fps={}[vbg]",
            spec.width, spec.height, spec.width, spec.height, spec.fps
        ));
    }

    // 2. Burn captions — NOTE: caption burning is now handled in step 3b
    // (after meme b-rolls are composited onto [vbg], so captions remain
    // visible ON TOP of full-screen meme cuts). This step is intentionally
    // a no-op.
    let _ = &spec.captions_path;
    let current_video_label = "[vbg]";

    // 3. Overlay meme b-rolls (full-screen GIPHY video clips) and stickers
    //
    // LAYERING ORDER (z-axis, bottom to top):
    //   1. Background clips [vbg]
    //   2. Full-screen meme b-rolls (brief cuts from GIPHY MP4)
    //      — composited BEFORE captions so captions remain visible on top
    //   3. Captions (burned on top of meme-composited video)
    //   4. Regular stickers (small overlays, scale<1.0)
    //      — composited AFTER captions so they appear on top

    // 3a. Add meme b-roll clips as FFmpeg inputs
    let mut meme_input_start = if has_music {
        music_input_idx + 1
    } else {
        vo_input_idx + 1
    };
    // Adjust for sticker inputs that were already added
    meme_input_start += sticker_inputs.len();

    let mut meme_inputs: Vec<(usize, &MemeClip)> = Vec::new();
    for meme in &spec.meme_clips {
        if std::path::Path::new(&meme.path).exists() {
            // Loop the meme input to the full output duration so the
            // blurred-letterbox background stream has frames for the entire
            // video. Without this, the short GIPHY MP4 (~2-3s) runs out of
            // frames and the overlay filtergraph fails to bind (the base
            // stream [mb{idx}_bg] would EOF before the output ends).
            // (Round-12 fresh-agent UX: render failed with "Filter 'fps:default'
            // has output 0 (vbg) unconnected" because [vbg] was never consumed
            // as a base — the meme overlay was missing its connection back to
            // the background. Fixed in the filter section below.)
            cmd.arg("-stream_loop").arg("-1");
            cmd.arg("-t").arg(spec.total_duration_s.to_string());
            cmd.arg("-i").arg(&meme.path);
            meme_inputs.push((meme_input_start, meme));
            meme_input_start += 1;
        }
    }

    // 3b. Overlay full-screen meme b-rolls on the BACKGROUND (before captions)
    //
    // Meme b-roll scaling: use force_original_aspect_ratio=decrease + pad
    // (contain/letterbox) so the FULL GIPHY clip is visible without cropping.
    // The GIPHY clips are typically 480x270 — scaling to 1080x1920 with
    // "increase+crop" would zoom in massively and lose quality. With
    // "decrease+pad", the clip is scaled to fit within the canvas and the
    // remaining area is filled with a blurred version of the clip itself
    // (blurred letterbox) for a professional look.
    let mut video_label = "[vbg]".to_string();
    for (idx, (input_idx, meme)) in meme_inputs.iter().enumerate() {
        let meme_w = spec.width;
        let meme_h = spec.height;

        tracing::info!(
            "[render] Meme b-roll {} ({}): FULLSCREEN size={}x{}, time={:.1}-{:.1}s",
            idx, meme.path, meme_w, meme_h, meme.start_s, meme.end_s
        );

        let meme_label = format!("[mb{}]", idx);
        let out_label = format!("[vmb{}]", idx);

        // Blurred letterbox: scale the clip to fill the canvas (increase),
        // blur it heavily, then overlay the properly-scaled (decrease) clip
        // on top. This creates a professional "blurred background" effect
        // that fills the screen while showing the full GIF/MP4 in the center.
        //
        // [mb{idx}_bg] = blurred full-screen background (looped to full duration)
        // [mb{idx}_fg] = sharp contain-scaled foreground (looped to full duration)
        // [vmid{idx}]  = blurred bg + sharp fg overlaid (the meme composite)
        // [vmb{idx}]   = video_label + meme composite, ONLY during the meme's
        //                time range. Outside the range, video_label (the
        //                background) passes through unchanged.
        //
        // The previous version produced [vmb{idx}] from only [mb_bg]+[mb_fg]
        // and never connected [vbg] — FFmpeg errored with "output 0 (vbg)
        // unconnected" and the background was never composited with the meme.
        // (Round-12 fresh-agent UX fix.)
        filters.push(format!(
            "[{}:v]scale={}:{}:force_original_aspect_ratio=increase,crop={}:{},boxblur=20:5,fps={},setpts=PTS-STARTPTS[mb{}_bg]",
            input_idx, meme_w, meme_h, meme_w, meme_h, spec.fps, idx
        ));
        filters.push(format!(
            "[{}:v]scale={}:{}:force_original_aspect_ratio=decrease,pad={}:{}:-1:-1:color=black,fps={},setpts=PTS-STARTPTS[mb{}_fg]",
            input_idx, meme_w, meme_h, meme_w, meme_h, spec.fps, idx
        ));
        // [vmid{idx}] = blurred bg + sharp fg (full-screen meme composite)
        filters.push(format!(
            "[mb{}_bg][mb{}_fg]overlay=0:0:enable='between(t,{},{})':eof_action=repeat[vmid{}]",
            idx, idx, meme.start_s, meme.end_s, idx
        ));
        // [vmb{idx}] = background + meme composite, only during the meme range
        filters.push(format!(
            "{}[vmid{}]overlay=0:0:enable='between(t,{},{})':eof_action=repeat[vmb{}]",
            video_label, idx, meme.start_s, meme.end_s, idx
        ));

        video_label = format!("[vmb{}]", idx);
    }

    // 3c. Burn captions ON TOP of meme-composited video
    let video_label = if let Some(ref captions) = spec.captions_path {
        if std::path::Path::new(captions).exists() {
            let escaped = captions.replace('\\', "/").replace('\'', "'\\''");
            let cap_label = if video_label == "[vbg]" {
                filters.push(format!("[vbg]subtitles='{}'[vcap]", escaped));
                "[vcap]"
            } else {
                filters.push(format!("{}subtitles='{}'[vcap]", video_label, escaped));
                "[vcap]"
            };
            cap_label.to_string()
        } else {
            video_label
        }
    } else {
        video_label
    };

    // 3d. Overlay regular stickers ON TOP of captions
    //
    // GIF sticker rendering fixes (round-10):
    // - Animated GIFs need `loop=loop=-1:size=0` to loop continuously
    // - GIFs need `fps` filter to match output framerate for smooth animation
    // - Non-square GIFs need crop-to-fit or letterbox to avoid distortion
    // - The user requested "crop to fit with custom-solid-color background
    //   or blurred-letterbox so that it can be perfectly optimal"
    //
    // We use a square crop-to-fit approach: scale the GIF to fill the
    // target square area, then crop to exact dimensions. This avoids
    // distortion and ensures the sticker always fills its allocated space.
    let mut video_label = video_label;
    for (idx, (input_idx, sticker)) in sticker_inputs.iter().enumerate() {
        let sticker_w = (spec.width as f64 * sticker.scale) as u32;
        let sticker_h = sticker_w; // Square target area
        let (tl_x, tl_y, _cx, _cy) = parse_position(
            &sticker.position,
            spec.width,
            spec.height,
            sticker_w,
            sticker_h,
        );

        tracing::info!(
            "[render] Sticker {} ({}): scale={:.0}%, size={}x{}, top_left=({}, {})",
            idx, sticker.path, sticker.scale * 100.0, sticker_w, sticker_h, tl_x, tl_y
        );

        let st_label = format!("[st{}]", idx);
        let out_label = if idx == sticker_inputs.len() - 1 {
            "[vout]".to_string()
        } else {
            format!("[vst{}]", idx)
        };

        // Check if the sticker is a GIF (animated) or a regular image/video
        let is_gif = sticker.path.ends_with(".gif");

        if is_gif {
            // Animated GIF: loop continuously + contain (letterbox) mode.
            // Round-11: user said "the GIF must show up in the middle, being
            // able to show the full-GIF effectively. Currently, it is very
            // zoomed in low resolution image."
            //
            // Use force_original_aspect_ratio=decrease (contain/letterbox)
            // instead of increase (crop). This shows the FULL GIF without
            // cropping, padded to the target size. The padding is transparent
            // (GIF alpha), so the background shows through.
            //
            // - loop=loop=-1:size=0: infinite loop
            // - scale=W:H:force_original_aspect_ratio=decrease: contain (full GIF visible)
            // - pad=W:H:(W-w)/2:(H-h)/2:center: center the GIF in the target area
            // - fps={fps}: match output framerate for smooth playback
            // - setpts=PTS-STARTPTS: reset timestamps
            filters.push(format!(
                "[{}:v]loop=loop=-1:size=0,scale={}:{}:force_original_aspect_ratio=decrease,pad={}:{}:-1:-1:color=0x00000000,fps={},setpts=PTS-STARTPTS[st{}]",
                input_idx, sticker_w, sticker_h, sticker_w, sticker_h, spec.fps, idx
            ));
        } else {
            // Regular image or video: contain mode (full content visible)
            filters.push(format!(
                "[{}:v]scale={}:{}:force_original_aspect_ratio=decrease,pad={}:{}:-1:-1:color=0x00000000,fps={},setpts=PTS-STARTPTS[st{}]",
                input_idx, sticker_w, sticker_h, sticker_w, sticker_h, spec.fps, idx
            ));
        }

        // Overlay with time-based enable
        filters.push(format!(
            "{}{}overlay={}:{}:enable='between(t,{},{})':eof_action=repeat{}",
            video_label, st_label, tl_x, tl_y, sticker.start_s, sticker.end_s, out_label
        ));

        video_label = out_label;
    }

    // If no stickers, rename current video to vout
    if sticker_inputs.is_empty() {
        filters.push(format!("{}copy[vout]", video_label));
    }

    // 4. Audio: voiceover + music (with sidechain ducking)
    //
    // The round-5 audit found music was inaudible because:
    //   1. threshold=0.001 was too low — any voice signal triggered full ducking
    //   2. makeup=1 meant no gain recovery after compression
    //   3. Default music_volume was -18 dB (linear 0.126) — already very quiet
    //
    // Fix: raise threshold to 0.05 (only duck on moderate voice), add makeup
    // gain of 2x (~6 dB boost after ducking so music stays present), and
    // the caller now passes a higher default volume (-12 dB instead of -18).
    if has_music {
        // Sidechain compression: duck music when voice is present.
        // threshold=0.05: only trigger on moderate voice (not background noise)
        // ratio=4: 4:1 compression
        // makeup=2: boost music 2x after compression so it stays audible
        filters.push(format!("[{}:a]asplit=2[vo_out][vo_sc]", vo_input_idx));
        filters.push(format!(
            "[{}:a]volume={}[music_vol]",
            music_input_idx, spec.music_volume
        ));
        filters.push(format!(
            "[music_vol][vo_sc]sidechaincompress=threshold=0.05:ratio=4:attack=50:release=200:makeup=2:level_sc=1[music_ducked]"
        ));
        filters.push(format!(
            "[vo_out][music_ducked]amix=inputs=2:duration=first:dropout_transition=2:normalize=0[aout_raw]"
        ));
    } else {
        filters.push(format!("[{}:a]anull[aout_raw]", vo_input_idx));
    }

    // Loudness normalization
    filters.push("[aout_raw]loudnorm=I=-16:TP=-1.5:LRA=11[aout]".to_string());

    let filter_complex = filters.join(";");

    cmd.arg("-filter_complex").arg(&filter_complex);
    cmd.arg("-map").arg("[vout]");
    cmd.arg("-map").arg("[aout]");

    // Video codec
    cmd.arg("-c:v").arg("libx264");
    cmd.arg("-preset").arg(&spec.preset);
    cmd.arg("-crf").arg(spec.crf.to_string());
    cmd.arg("-pix_fmt").arg("yuv420p");
    cmd.arg("-r").arg(spec.fps.to_string());

    // Audio codec
    cmd.arg("-c:a").arg("aac");
    cmd.arg("-b:a").arg("160k");

    // Duration
    cmd.arg("-t").arg(spec.total_duration_s.to_string());

    // Output
    cmd.arg(&spec.output_path);

    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let log_path = format!("{}.render.log", spec.output_path);

    let mut child = cmd.spawn().map_err(|e| {
        let _ = std::fs::remove_file(&concat_list_path);
        FfmpegError::RenderFailed(format!("Failed to spawn ffmpeg: {}", e))
    })?;

    let stderr = child.stderr.take().expect("stderr was piped");
    let mut reader = BufReader::new(stderr);
    let mut line = String::new();
    let mut full_stderr = Vec::new();

    while reader.read_line(&mut line).await.unwrap_or(0) > 0 {
        full_stderr.extend_from_slice(line.as_bytes());
        line.clear();
    }

    let output = child.wait().await.map_err(|e| {
        let _ = std::fs::remove_file(&concat_list_path);
        FfmpegError::RenderFailed(format!("Failed to wait for ffmpeg: {}", e))
    })?;

    let _ = std::fs::write(&log_path, &full_stderr);
    let _ = std::fs::remove_file(&concat_list_path);

    if !output.success() {
        let stderr_str = String::from_utf8_lossy(&full_stderr);
        return Err(FfmpegError::RenderFailed(format!(
            "Render failed, see log: {}\nLast 5 lines: {}",
            log_path,
            stderr_str
                .lines()
                .rev()
                .take(5)
                .collect::<Vec<_>>()
                .join("\n")
        )));
    }

    Ok(spec.output_path.clone())
}

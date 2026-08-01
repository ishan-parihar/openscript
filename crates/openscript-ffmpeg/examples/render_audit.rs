use openscript_core::timeline::Timeline;
use openscript_ffmpeg::render::render_from_timeline_with_cancel;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let timeline_path = &args[1];
    let source_path = &args[2];
    let output_path = &args[3];
    let skip_audio = args.get(4).map(|s| s == "skip_audio").unwrap_or(false);

    eprintln!("[render_audit] loading timeline: {}", timeline_path);
    let tl = Timeline::load(timeline_path)?;
    eprintln!("[render_audit] {} segments, source: {}", tl.segments.len(), source_path);

    // For b-roll verification, we can mutate the timeline to remove SFX
    // events (which currently break the audio mix chain independently of
    // the b-roll fix).
    let mut tl_for_render = tl.clone();
    if skip_audio {
        // Clear SFX and music tracks to isolate the b-roll video render
        tl_for_render.tracks.remove(&openscript_core::types::TrackType::Music);
    }

    eprintln!("[render_audit] rendering to {}", output_path);
    let result = render_from_timeline_with_cancel(
        &tl_for_render,
        source_path,
        Some(output_path),
        Some(20),
        None,
    )
    .await?;
    eprintln!("[render_audit] rendered: {}", result);
    Ok(())
}

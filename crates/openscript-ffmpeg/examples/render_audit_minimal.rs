use openscript_core::timeline::Timeline;
use openscript_ffmpeg::render::render_from_timeline_with_cancel;
use openscript_core::types::TrackType;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let timeline_path = &args[1];
    let source_path = &args[2];
    let output_path = &args[3];

    eprintln!("[render_minimal] loading timeline: {}", timeline_path);
    let mut tl = Timeline::load(timeline_path)?;
    eprintln!("[render_minimal] {} segments, source: {}", tl.segments.len(), source_path);

    // Strip everything to bare minimum: just dialogue audio + b-rolls
    tl.tracks.remove(&TrackType::Sfx);
    tl.tracks.remove(&TrackType::Music);
    tl.tracks.remove(&TrackType::Voiceover);
    tl.tracks.remove(&TrackType::Dialogue);
    tl.tracks.remove(&TrackType::Music);
    tl.tracks.remove(&TrackType::Captions);
    eprintln!("[render_minimal] stripped to video-only b-roll chain");

    eprintln!("[render_minimal] rendering to {}", output_path);
    let start = std::time::Instant::now();
    let result = render_from_timeline_with_cancel(
        &tl,
        source_path,
        Some(output_path),
        Some(20),
        None,
    )
    .await?;
    eprintln!("[render_minimal] rendered: {} in {:?}", result, start.elapsed());
    Ok(())
}

use openscript_core::timeline::Timeline;
use openscript_ffmpeg::filter_graph::FilterGraphBuilder;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let timeline_path = &args[1];
    let output_path = &args[2];

    let tl = Timeline::load(timeline_path)?;
    eprintln!("[filter_dump] timeline: {}", timeline_path);
    eprintln!("[filter_dump] segments: {}", tl.segments.len());

    let mut builder = FilterGraphBuilder::from_timeline(&tl);
    let (filter, vout, aout) = builder.build();
    std::fs::write(output_path, &filter)?;
    eprintln!("[filter_dump] wrote filter graph to {}", output_path);
    eprintln!("[filter_dump] vout: {} aout: {}", vout, aout);
    eprintln!("[filter_dump] filter length: {} chars", filter.len());
    eprintln!("[filter_dump] amix count: {}", filter.matches("amix=inputs=").count());
    eprintln!("[filter_dump] movie= count: {}", filter.matches("movie=").count());
    eprintln!("[filter_dump] amovie count: {}", filter.matches("amovie=").count());

    Ok(())
}

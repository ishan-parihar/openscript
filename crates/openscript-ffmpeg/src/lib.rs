pub mod filter_graph;
pub mod gpu;
pub mod multilayer_render;
pub mod probe;
pub mod render;
pub mod script_render;
pub mod subtitles;

#[derive(Debug, thiserror::Error)]
pub enum FfmpegError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("No segments in EDL")]
    NoSegments,
    #[error("Render failed, see log: {0}")]
    RenderFailed(String),
}

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ToolError {
    #[error("Missing required argument: {0}")]
    MissingArg(String),
    #[error("Unknown tool: {0}")]
    UnknownTool(String),
    #[error("Method not found: {0}")]
    MethodNotFound(String),
    #[error("Timeline error: {0}")]
    Timeline(String),
    #[error("SRT error: {0}")]
    Srt(String),
    #[error("FFmpeg error: {0}")]
    Ffmpeg(String),
    #[error("Asset error: {0}")]
    Asset(String),
    #[error("TTS error: {0}")]
    Tts(String),
    #[error("Transcription error: {0}")]
    Transcribe(String),
    #[error("HyperFrames error: {0}")]
    Hf(String),
    #[error("File not found: {0}")]
    NotFound(String),
    #[error("Invalid argument: {0}")]
    InvalidArg(String),
    #[error("Permission denied: {0}")]
    Permission(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

impl ToolError {
    /// Map error to JSON-RPC error code.
    pub fn json_rpc_code(&self) -> i64 {
        match self {
            ToolError::MethodNotFound(_) => -32601,
            ToolError::MissingArg(_) => -32602,
            ToolError::InvalidArg(_) => -32602,
            ToolError::UnknownTool(_) => -32601,
            _ => -32000,
        }
    }
}

impl From<openscript_core::timeline::TimelineError> for ToolError {
    fn from(e: openscript_core::timeline::TimelineError) -> Self {
        ToolError::Timeline(e.to_string())
    }
}

impl From<openscript_core::srt::SrtError> for ToolError {
    fn from(e: openscript_core::srt::SrtError) -> Self {
        ToolError::Srt(e.to_string())
    }
}

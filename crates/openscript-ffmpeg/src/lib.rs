pub mod filter_graph;
pub mod gpu;
pub mod multilayer_render;
pub mod probe;
pub mod render;
pub mod script_render;
pub mod subtitles;

/// Resolve a registry record for an asset referenced by a timeline event,
/// accepting BOTH registry conventions so callers never silently miss assets:
///   (a) `asset_id == registry key` (broll.fetch-style) → direct hit
///   (b) `asset_id == file path`, registry keyed by event id
///       (broll.assign / overlay.assign-style) → scan for a record whose
///       `path` field equals the asset_id.
///
/// Used by `FilterGraphBuilder::from_timeline` (b-roll + sticker lanes) and by
/// the `render_from_timeline` probe loop. Keeping it in ONE place prevents the
/// two sites from drifting (the V2V fixture audit caught a silent b-roll drop
/// caused by exactly that divergence).
pub(crate) fn find_registry_record<'a>(
    registry: &'a std::collections::HashMap<String, serde_json::Value>,
    asset_id: &str,
) -> Option<&'a serde_json::Value> {
    registry.get(asset_id).or_else(|| {
        registry.values().find(|v| {
            v.get("path")
                .and_then(|p| p.as_str())
                .map(|p| p == asset_id)
                .unwrap_or(false)
        })
    })
}

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

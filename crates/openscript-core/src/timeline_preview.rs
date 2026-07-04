//! Layered timeline preview system for AI agents.
//!
//! Provides a token-efficient representation of the video composition
//! as temporal layers, allowing agents to:
//! - Preview the entire composition at a glance
//! - Drill into specific layers for detailed restructuring
//! - Identify layering issues before rendering

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A single layer in the timeline (video, audio, captions, stickers).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineLayer {
    /// Layer type: "background", "broll", "voiceover", "music", "sfx", "captions", "stickers"
    pub layer_type: String,
    /// Layer name for display
    pub name: String,
    /// Events on this layer, sorted by start time
    pub events: Vec<LayerEvent>,
}

/// A single event on a layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerEvent {
    /// Start time in milliseconds
    pub start_ms: i64,
    /// End time in milliseconds
    pub end_ms: i64,
    /// Asset path or description
    pub asset: String,
    /// Optional metadata (speaker, text, position, etc.)
    #[serde(default)]
    pub metadata: serde_json::Value,
}

/// The complete layered timeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayeredTimeline {
    pub total_duration_ms: i64,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub layers: Vec<TimelineLayer>,
}

impl LayeredTimeline {
    /// Generate a compact text preview for AI agents (token-efficient).
    ///
    /// Example output:
    /// ```
    /// Timeline (25.1s, 1080x1920, 30fps)
    /// ├── BACKGROUND
    /// │   ├── [0.0s - 5.0s] minecraft_01.mp4
    /// │   ├── [5.0s - 10.0s] subway_01.mp4
    /// │   └── [10.0s - 25.1s] minecraft_01.mp4 (looped)
    /// ├── VOICEOVER
    /// │   ├── [0.0s - 3.9s] alice: "Welcome to the show..."
    /// │   ├── [3.9s - 7.7s] bob: "Today we're diving..."
    /// │   └── [7.7s - 12.0s] alice: "Let's start with..."
    /// ├── MUSIC
    /// │   └── [0.0s - 25.1s] lofi.mp3 (ducked -15dB)
    /// ├── CAPTIONS
    /// │   ├── [0.0s - 0.5s] "Welcome" (highlighted)
    /// │   ├── [0.5s - 0.8s] "to" (highlighted)
    /// │   └── [0.8s - 1.1s] "the" (highlighted)
    /// └── STICKERS
    ///     ├── [0.0s - 3.9s] alice.png (bottom-left, 20%)
    ///     └── [3.9s - 7.7s] bob.png (bottom-right, 20%)
    /// ```
    pub fn preview(&self) -> String {
        let duration_s = self.total_duration_ms as f64 / 1000.0;
        let mut out = format!(
            "Timeline ({:.1}s, {}x{}, {}fps)\n",
            duration_s, self.width, self.height, self.fps
        );

        for (i, layer) in self.layers.iter().enumerate() {
            let is_last = i == self.layers.len() - 1;
            let prefix = if is_last { "└── " } else { "├── " };
            let cont = if is_last { "    " } else { "│   " };

            out.push_str(&format!("{}{}\n", prefix, layer.name.to_uppercase()));

            for (j, event) in layer.events.iter().enumerate() {
                let is_last_event = j == layer.events.len() - 1;
                let event_prefix = if is_last_event { "└── " } else { "├── " };

                let start_s = event.start_ms as f64 / 1000.0;
                let end_s = event.end_ms as f64 / 1000.0;

                let desc = self.format_event_desc(layer, event);
                out.push_str(&format!(
                    "{}{}[{:.1}s - {:.1}s] {}\n",
                    cont, event_prefix, start_s, end_s, desc
                ));
            }
        }

        out
    }

    /// Format an event description based on its layer type.
    fn format_event_desc(&self, layer: &TimelineLayer, event: &LayerEvent) -> String {
        match layer.layer_type.as_str() {
            "voiceover" => {
                let speaker = event.metadata.get("speaker")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                let text = event.metadata.get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let truncated = if text.len() > 40 {
                    format!("{}...", &text[..37])
                } else {
                    text.to_string()
                };
                format!("{}: \"{}\"", speaker, truncated)
            }
            "captions" => {
                let word_count = event.metadata.get("word_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let style = event.metadata.get("style")
                    .and_then(|v| v.as_str())
                    .unwrap_or("word_highlight");
                let filename = std::path::Path::new(&event.asset)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(&event.asset);
                format!("{} ({} words, {} style)", filename, word_count, style)
            }
            "stickers" => {
                let position = event.metadata.get("position")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                let scale = event.metadata.get("scale")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.25);
                format!("{} ({}, {:.0}%)", event.asset, position, scale * 100.0)
            }
            "music" => {
                let ducked = event.metadata.get("ducked")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if ducked {
                    let depth = event.metadata.get("ducking_depth_db")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(12.0);
                    format!("{} (ducked -{:.0}dB)", event.asset, depth)
                } else {
                    event.asset.clone()
                }
            }
            "background" => {
                let looped = event.metadata.get("looped")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if looped {
                    format!("{} (looped)", event.asset)
                } else {
                    event.asset.clone()
                }
            }
            _ => event.asset.clone(),
        }
    }

    /// Get a compact JSON summary (for programmatic inspection).
    pub fn summary(&self) -> serde_json::Value {
        serde_json::json!({
            "total_duration_ms": self.total_duration_ms,
            "duration_s": self.total_duration_ms as f64 / 1000.0,
            "width": self.width,
            "height": self.height,
            "fps": self.fps,
            "layer_count": self.layers.len(),
            "layers": self.layers.iter().map(|l| {
                serde_json::json!({
                    "type": l.layer_type,
                    "name": l.name,
                    "event_count": l.events.len(),
                    "first_start_ms": l.events.first().map(|e| e.start_ms),
                    "last_end_ms": l.events.last().map(|e| e.end_ms),
                })
            }).collect::<Vec<_>>(),
        })
    }

    /// Inspect a specific layer in detail.
    pub fn inspect_layer(&self, layer_type: &str) -> Option<&TimelineLayer> {
        self.layers.iter().find(|l| l.layer_type == layer_type)
    }

    /// Validate the timeline for common issues.
    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();

        for layer in &self.layers {
            // Check for gaps in voiceover layer
            if layer.layer_type == "voiceover" {
                for i in 1..layer.events.len() {
                    let prev_end = layer.events[i - 1].end_ms;
                    let curr_start = layer.events[i].start_ms;
                    if curr_start > prev_end + 100 {
                        issues.push(format!(
                            "Voiceover gap: {:.1}s - {:.1}s ({}ms gap)",
                            prev_end as f64 / 1000.0,
                            curr_start as f64 / 1000.0,
                            curr_start - prev_end
                        ));
                    }
                }
            }

            // Check for overlapping events on same layer
            for i in 1..layer.events.len() {
                let prev_end = layer.events[i - 1].end_ms;
                let curr_start = layer.events[i].start_ms;
                if curr_start < prev_end {
                    issues.push(format!(
                        "Overlap in {}: {:.1}s - {:.1}s",
                        layer.layer_type,
                        curr_start as f64 / 1000.0,
                        prev_end as f64 / 1000.0
                    ));
                }
            }

            // Check for events extending beyond total duration
            for event in &layer.events {
                if event.end_ms > self.total_duration_ms + 100 {
                    issues.push(format!(
                        "{} event extends beyond timeline: ends at {:.1}s (timeline: {:.1}s)",
                        layer.layer_type,
                        event.end_ms as f64 / 1000.0,
                        self.total_duration_ms as f64 / 1000.0
                    ));
                }
            }
        }

        // Check for empty layers
        for layer in &self.layers {
            if layer.events.is_empty() {
                issues.push(format!("Layer '{}' has no events", layer.layer_type));
            }
        }

        issues
    }
}

/// Build a LayeredTimeline from a voiceover manifest, background assignments,
/// and script spec.
pub fn build_layered_timeline(
    manifest: &serde_json::Value,
    background_clips: &[BackgroundClipAssignment],
    music_path: Option<&str>,
    music_ducking: bool,
    sticker_assignments: &[StickerAssignment],
    captions_path: Option<&str>,
    width: u32,
    height: u32,
    fps: u32,
) -> LayeredTimeline {
    let mut layers = Vec::new();
    let mut total_duration_ms = 0i64;

    // Extract segments from manifest
    let segments = manifest.get("segments")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // Layer 1: Background
    let mut bg_events = Vec::new();
    for (i, clip) in background_clips.iter().enumerate() {
        bg_events.push(LayerEvent {
            start_ms: clip.start_ms,
            end_ms: clip.end_ms,
            asset: clip.path.clone(),
            metadata: serde_json::json!({
                "looped": clip.looped,
                "scene_index": i,
            }),
        });
        total_duration_ms = total_duration_ms.max(clip.end_ms);
    }
    layers.push(TimelineLayer {
        layer_type: "background".into(),
        name: "Background".into(),
        events: bg_events,
    });

    // Layer 2: Voiceover
    let mut vo_events = Vec::new();
    for seg in &segments {
        let start_ms = seg.get("start_ms").and_then(|v| v.as_i64()).unwrap_or(0);
        let end_ms = seg.get("end_ms").and_then(|v| v.as_i64()).unwrap_or(0);
        let speaker = seg.get("speaker").and_then(|v| v.as_str()).unwrap_or("");
        let text = seg.get("text").and_then(|v| v.as_str()).unwrap_or("");
        let wav = seg.get("wav_path").and_then(|v| v.as_str()).unwrap_or("");

        vo_events.push(LayerEvent {
            start_ms,
            end_ms,
            asset: wav.to_string(),
            metadata: serde_json::json!({
                "speaker": speaker,
                "text": text,
            }),
        });
        total_duration_ms = total_duration_ms.max(end_ms);
    }
    layers.push(TimelineLayer {
        layer_type: "voiceover".into(),
        name: "Voiceover".into(),
        events: vo_events,
    });

    // Layer 3: Music
    if let Some(path) = music_path {
        layers.push(TimelineLayer {
            layer_type: "music".into(),
            name: "Music".into(),
            events: vec![LayerEvent {
                start_ms: 0,
                end_ms: total_duration_ms,
                asset: path.to_string(),
                metadata: serde_json::json!({
                    "ducked": music_ducking,
                    "ducking_depth_db": 12.0,
                }),
            }],
        });
    }

    // Layer 4: Stickers
    let mut sticker_events = Vec::new();
    for sticker in sticker_assignments {
        sticker_events.push(LayerEvent {
            start_ms: sticker.start_ms,
            end_ms: sticker.end_ms,
            asset: sticker.path.clone(),
            metadata: serde_json::json!({
                "position": sticker.position,
                "scale": sticker.scale,
                "speaker": sticker.speaker,
            }),
        });
    }
    if !sticker_events.is_empty() {
        layers.push(TimelineLayer {
            layer_type: "stickers".into(),
            name: "Stickers".into(),
            events: sticker_events,
        });
    }

    // Layer 5: Captions (summary)
    if let Some(caps) = captions_path {
        let mut caption_count = 0;
        for seg in &segments {
            if let Some(words) = seg.get("words").and_then(|v| v.as_array()) {
                caption_count += words.len();
            }
        }
        layers.push(TimelineLayer {
            layer_type: "captions".into(),
            name: "Captions".into(),
            events: vec![LayerEvent {
                start_ms: 0,
                end_ms: total_duration_ms,
                asset: caps.to_string(),
                metadata: serde_json::json!({
                    "word_count": caption_count,
                    "style": "word_highlight",
                }),
            }],
        });
    }

    LayeredTimeline {
        total_duration_ms,
        width,
        height,
        fps,
        layers,
    }
}

/// Background clip assignment for a scene.
#[derive(Debug, Clone)]
pub struct BackgroundClipAssignment {
    pub start_ms: i64,
    pub end_ms: i64,
    pub path: String,
    pub looped: bool,
}

/// Sticker assignment for a scene.
#[derive(Debug, Clone)]
pub struct StickerAssignment {
    pub start_ms: i64,
    pub end_ms: i64,
    pub path: String,
    pub position: String,
    pub scale: f64,
    pub speaker: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_timeline_preview() {
        let tl = LayeredTimeline {
            total_duration_ms: 10000,
            width: 1080,
            height: 1920,
            fps: 30,
            layers: vec![
                TimelineLayer {
                    layer_type: "background".into(),
                    name: "Background".into(),
                    events: vec![
                        LayerEvent {
                            start_ms: 0,
                            end_ms: 5000,
                            asset: "minecraft.mp4".into(),
                            metadata: json!({"looped": true}),
                        },
                        LayerEvent {
                            start_ms: 5000,
                            end_ms: 10000,
                            asset: "subway.mp4".into(),
                            metadata: json!({"looped": false}),
                        },
                    ],
                },
                TimelineLayer {
                    layer_type: "voiceover".into(),
                    name: "Voiceover".into(),
                    events: vec![LayerEvent {
                        start_ms: 0,
                        end_ms: 5000,
                        asset: "vo_001.wav".into(),
                        metadata: json!({"speaker": "alice", "text": "Welcome to the show everyone!"}),
                    }],
                },
            ],
        };

        let preview = tl.preview();
        assert!(preview.contains("10.0s"));
        assert!(preview.contains("BACKGROUND"));
        assert!(preview.contains("minecraft.mp4"));
        assert!(preview.contains("VOICEOVER"));
        assert!(preview.contains("alice"));
    }

    #[test]
    fn test_timeline_validate_no_issues() {
        let tl = LayeredTimeline {
            total_duration_ms: 10000,
            width: 1080,
            height: 1920,
            fps: 30,
            layers: vec![
                TimelineLayer {
                    layer_type: "background".into(),
                    name: "Background".into(),
                    events: vec![LayerEvent {
                        start_ms: 0, end_ms: 10000, asset: "bg.mp4".into(),
                        metadata: json!({}),
                    }],
                },
            ],
        };

        let issues = tl.validate();
        assert!(issues.is_empty(), "Expected no issues, got: {:?}", issues);
    }

    #[test]
    fn test_timeline_validate_overlap() {
        let tl = LayeredTimeline {
            total_duration_ms: 10000,
            width: 1080, height: 1920, fps: 30,
            layers: vec![TimelineLayer {
                layer_type: "voiceover".into(),
                name: "Voiceover".into(),
                events: vec![
                    LayerEvent { start_ms: 0, end_ms: 5000, asset: "a.wav".into(), metadata: json!({}) },
                    LayerEvent { start_ms: 4000, end_ms: 8000, asset: "b.wav".into(), metadata: json!({}) },
                ],
            }],
        };

        let issues = tl.validate();
        assert!(issues.iter().any(|i| i.contains("Overlap")));
    }

    #[test]
    fn test_timeline_summary() {
        let tl = LayeredTimeline {
            total_duration_ms: 5000,
            width: 1080, height: 1920, fps: 30,
            layers: vec![TimelineLayer {
                layer_type: "voiceover".into(),
                name: "Voiceover".into(),
                events: vec![LayerEvent {
                    start_ms: 0, end_ms: 5000, asset: "a.wav".into(),
                    metadata: json!({"speaker": "alice", "text": "Hello"}),
                }],
            }],
        };

        let summary = tl.summary();
        assert_eq!(summary["duration_s"], 5.0);
        assert_eq!(summary["layer_count"], 1);
    }
}

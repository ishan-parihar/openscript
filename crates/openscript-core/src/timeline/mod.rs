mod schema;

use crate::types::TrackType;
pub use schema::*;
use std::fs;
use std::path::{Path, PathBuf};

impl Timeline {
    /// Create a new empty timeline for a source video.
    pub fn new(source: PathBuf, aspect: &str, fps: u32, max_duration: Option<u32>) -> Self {
        Self {
            version: "2.0".to_string(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            source,
            target: RenderTarget {
                aspect: aspect.into(),
                fps,
                max_duration,
                width: None,
                height: None,
            },
            segments: Vec::new(),
            tracks: std::collections::HashMap::from([
                (TrackType::Dialogue, vec![]),
                (TrackType::Voiceover, vec![]),
                (TrackType::Captions, vec![]),
                (TrackType::Broll, vec![]),
                (TrackType::Music, vec![]),
                (TrackType::Sfx, vec![]),
            ]),
            directives: Directives {
                ducking: vec![],
                transitions: vec![],
                mix: MixConfig {
                    master_gain_db: 0.0,
                    limiter_threshold_db: -1.0,
                    normalize_to_lufs: -14.0,
                },
                render_backend: "auto".into(),
            },
            assets: AssetRegistry {
                voices: std::collections::HashMap::new(),
                broll: std::collections::HashMap::new(),
                music: std::collections::HashMap::new(),
                sfx: std::collections::HashMap::new(),
                captions: std::collections::HashMap::new(),
            },
            effects: Effects {
                burn_captions: true,
                audio: AudioEffects { loudnorm: true },
                caption_style: None,
            },
        }
    }

    /// Load timeline from JSON file.
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, TimelineError> {
        let data = fs::read_to_string(&path)?;
        let timeline: Self = serde_json::from_str(&data)?;
        Ok(timeline)
    }

    /// Save timeline to JSON file atomically (write .tmp, then rename).
    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<(), TimelineError> {
        let path = path.as_ref();
        let tmp_path = path.with_extension("json.tmp");
        let data = serde_json::to_string_pretty(self)?;
        fs::write(&tmp_path, data)?;
        fs::rename(&tmp_path, path)?;
        Ok(())
    }

    /// Add a segment and return its ID.
    pub fn add_segment(
        &mut self,
        start: f64,
        end: f64,
        caption: &str,
        crossfade_ms: u32,
        semantic_role: Option<&str>,
    ) -> String {
        let id = format!("seg_{:03}", self.segments.len() + 1);
        let role = semantic_role.and_then(|r| serde_json::from_str(&format!("\"{}\"", r)).ok());
        self.segments.push(Segment {
            id: id.clone(),
            start,
            end,
            caption: caption.into(),
            crossfade_ms,
            semantic_role: role,
        });
        self.updated_at = chrono::Utc::now();
        id
    }

    /// Add an event to a specific track.
    pub fn add_track_event(&mut self, track_type: TrackType, event: TimelineEvent) {
        self.tracks.entry(track_type).or_default().push(event);
        self.updated_at = chrono::Utc::now();
    }

    /// Add an asset to the registry.
    pub fn add_asset(&mut self, asset_type: &str, id: String, asset: serde_json::Value) {
        let registry = match asset_type {
            "voices" => &mut self.assets.voices,
            "broll" => &mut self.assets.broll,
            "music" => &mut self.assets.music,
            "sfx" => &mut self.assets.sfx,
            "captions" => &mut self.assets.captions,
            _ => return,
        };
        registry.insert(id, asset);
        self.updated_at = chrono::Utc::now();
    }

    /// Get total output duration in milliseconds (from last segment end).
    pub fn total_duration_ms(&self) -> i64 {
        if self.segments.is_empty() {
            return 0;
        }
        (self.segments.last().expect("non-empty segments").end * 1000.0).round() as i64
    }

    /// Returns expected rendered duration accounting for crossfade overlaps
    /// between consecutive segments. Crossfades reduce total duration because
    /// overlapping segments share time rather than being additive.
    ///
    /// NOTE: This assumes segments are contiguous (no gaps between them).
    /// If gaps exist, the actual rendered duration will be longer than this calculation.
    pub fn rendered_duration_ms(&self) -> i64 {
        if self.segments.is_empty() {
            return 0;
        }
        let raw_ms =
            (self.segments.last().expect("non-empty segments").end * 1000.0).round() as i64;
        let crossfade_overlap_ms: i64 = self
            .segments
            .iter()
            .skip(1)
            .map(|s| s.crossfade_ms as i64)
            .sum();
        raw_ms.saturating_sub(crossfade_overlap_ms)
    }

    /// Get all events for a specific track type (Python-compatible API).
    pub fn get_track_events(&self, track_type: &str) -> Vec<&TimelineEvent> {
        if let Ok(tt) = track_type.parse::<TrackType>() {
            self.tracks
                .get(&tt)
                .map(|v| v.iter().collect::<Vec<_>>())
                .unwrap_or_default()
        } else {
            Vec::new()
        }
    }

    /// Serialize timeline to a Python-compatible dict.
    /// Python's EDL_v2 expects tracks as `{"dialogue": [dict, ...], ...}`.
    pub fn to_dict(&self) -> serde_json::Value {
        let tracks_map: serde_json::Map<String, serde_json::Value> = self
            .tracks
            .iter()
            .map(|(tt, events)| {
                (
                    tt.to_string(),
                    serde_json::to_value(events).unwrap_or_default(),
                )
            })
            .collect();

        let segments_json: Vec<serde_json::Value> = self
            .segments
            .iter()
            .map(|s| {
                serde_json::json!({
                    "id": s.id,
                    "start": s.start,
                    "end": s.end,
                    "caption": s.caption,
                    "crossfade_ms": s.crossfade_ms,
                    "semantic_role": s.semantic_role.as_ref().map(|r| r.to_string()),
                })
            })
            .collect();

        serde_json::json!({
            "version": self.version,
            "created_at": self.created_at.to_rfc3339(),
            "updated_at": self.updated_at.to_rfc3339(),
            "source": self.source.to_string_lossy(),
            "target": {
                "aspect": self.target.aspect,
                "fps": self.target.fps,
                "max_duration": self.target.max_duration,
            },
            "segments": segments_json,
            "tracks": tracks_map,
            "directives": serde_json::to_value(&self.directives).unwrap_or_default(),
            "assets": serde_json::to_value(&self.assets).unwrap_or_default(),
            "effects": serde_json::to_value(&self.effects).unwrap_or_default(),
        })
    }

    /// Validate the timeline. Returns list of error messages.
    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();
        if self.source.as_os_str().is_empty() {
            errors.push("Source video path is required".into());
        }
        for (i, seg) in self.segments.iter().enumerate() {
            if seg.start >= seg.end {
                errors.push(format!(
                    "Segment {}: start ({}) must be before end ({})",
                    seg.id, seg.start, seg.end
                ));
            }
            if i > 0 && seg.start < self.segments[i - 1].end {
                errors.push(format!(
                    "Segment {}: overlaps with previous segment",
                    seg.id
                ));
            }
        }
        let total_ms = self.total_duration_ms();
        for (track_name, events) in &self.tracks {
            for event in events {
                // Voiceover and Music events are allowed to extend beyond the
                // last segment end — they are additive (outro narration, music
                // tail). Only flag Dialogue/Broll/Caption/Sfx events.
                let is_additive = matches!(
                    event.kind,
                    EventKind::Voiceover { .. } | EventKind::Music { .. }
                );
                if !is_additive && event.end_ms > total_ms {
                    errors.push(format!(
                        "Track {:?} event {}: extends beyond timeline duration",
                        track_name, event.id
                    ));
                }
            }
        }
        errors
    }

    /// Add a ducking directive.
    pub fn add_ducking_directive(
        &mut self,
        when: &str,
        target_track: &str,
        reduction_db: f64,
        attack_ms: u32,
        release_ms: u32,
    ) {
        self.directives.ducking.push(DuckingDirective {
            when: when.into(),
            target_track: target_track.into(),
            reduction_db,
            attack_ms,
            release_ms,
        });
        self.updated_at = chrono::Utc::now();
    }

    /// Upgrade an EDL v1 dict to a Timeline.
    pub fn from_edl_v1(v1: &serde_json::Value) -> Result<Self, TimelineError> {
        let source = v1
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .into();
        let target = v1
            .get("target")
            .cloned()
            .unwrap_or(serde_json::json!({"aspect": "9:16", "fps": 30}));
        let target: RenderTarget = serde_json::from_value(target)?;
        let segments: Vec<Segment> = v1
            .get("segments")
            .and_then(|arr| arr.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|s| {
                        serde_json::from_value(s.clone()).unwrap_or(Segment {
                            id: format!("seg_{:03}", 0),
                            start: s.get("start").and_then(|v| v.as_f64()).unwrap_or(0.0),
                            end: s.get("end").and_then(|v| v.as_f64()).unwrap_or(0.0),
                            caption: s
                                .get("caption")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .into(),
                            crossfade_ms: s
                                .get("crossfade_ms")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(80) as u32,
                            semantic_role: None,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let effects = v1
            .get("effects")
            .cloned()
            .unwrap_or(serde_json::json!({"burn_captions": true, "audio": {"loudnorm": true}}));
        let effects: Effects = serde_json::from_value(effects)?;
        Ok(Self {
            version: "2.0".into(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            source,
            target,
            segments,
            tracks: std::collections::HashMap::from([
                (TrackType::Dialogue, vec![]),
                (TrackType::Voiceover, vec![]),
                (TrackType::Captions, vec![]),
                (TrackType::Broll, vec![]),
                (TrackType::Music, vec![]),
                (TrackType::Sfx, vec![]),
            ]),
            directives: Directives {
                ducking: vec![],
                transitions: vec![],
                mix: MixConfig {
                    master_gain_db: 0.0,
                    limiter_threshold_db: -1.0,
                    normalize_to_lufs: -14.0,
                },
                render_backend: "auto".into(),
            },
            assets: AssetRegistry {
                voices: std::collections::HashMap::new(),
                broll: std::collections::HashMap::new(),
                music: std::collections::HashMap::new(),
                sfx: std::collections::HashMap::new(),
                captions: std::collections::HashMap::new(),
            },
            effects,
        })
    }

    pub fn generate_broll_from_script(&mut self, cadence_ms: i64, max_slots: usize) -> usize {
        let mut slots_created = 0;
        let segments = self.segments.clone();
        let initial_broll_count = self
            .tracks
            .get(&TrackType::Broll)
            .map(|v| v.len())
            .unwrap_or(0);
        let mut event_counter = 0;

        for segment in &segments {
            if slots_created >= max_slots {
                break;
            }

            let concept = segment
                .caption
                .split_whitespace()
                .next()
                .unwrap_or("general")
                .to_string();
            let seg_start_ms = (segment.start * 1000.0).round() as i64;
            let seg_duration_ms = ((segment.end - segment.start) * 1000.0).round() as i64;

            let mut offset_ms = 0i64;
            while offset_ms < seg_duration_ms && slots_created < max_slots {
                let slot_duration = cadence_ms.min(seg_duration_ms - offset_ms);
                if slot_duration <= 0 {
                    break;
                }

                event_counter += 1;
                let event_id = format!("broll_{:03}", initial_broll_count + event_counter);
                let event = TimelineEvent {
                    id: event_id.clone(),
                    asset_id: "placeholder".into(),
                    start_ms: seg_start_ms + offset_ms,
                    end_ms: seg_start_ms + offset_ms + slot_duration,
                    offset_ms: 0,
                    gain_db: 0.0,
                    fade_in_ms: 0,
                    fade_out_ms: 0,
                    tags: vec![concept.clone()],
                    provenance: Some(Provenance {
                        tool: "broll.director".into(),
                        editorial_role: None,
                        concept: Some(concept.clone()),
                    }),
                    kind: EventKind::Broll {
                        concept: concept.clone(),
                        source_provider: "placeholder".into(),
                        transition_style: "cut".into(),
                        crop_mode: "center".into(),
                        orientation: "9:16".into(),
                        motion_intensity: "medium".into(),
                    },
                };

                self.add_track_event(TrackType::Broll, event);
                slots_created += 1;
                offset_ms += cadence_ms;
            }
        }

        slots_created
    }

    /// Populate segments and caption events from a grouped SRT file.
    ///
    /// Parses the SRT file (format: index\ntimestamp --> timestamp\ntext\n\n),
    /// creates a Segment for each entry, and adds corresponding Caption events
    /// to the Captions track.
    ///
    /// Returns the number of segments created.
    pub fn populate_segments_from_srt(
        &mut self,
        srt_path: &str,
        crossfade_ms: u32,
    ) -> Result<usize, String> {
        // Use the canonical srt::parse_srt rather than a duplicate inline
        // parser. Prior versions re-implemented SRT parsing + timestamp
        // parsing here (~85 LoC of duplicate logic). The canonical parser
        // also handles \r\n line endings and word-per-line formats that the
        // inline parser did not.
        let entries = crate::srt::parse_srt(srt_path)
            .map_err(|e| format!("Failed to parse SRT file '{}': {}", srt_path, e))?;

        let mut count = 0;
        let initial_caption_count = self
            .tracks
            .get(&TrackType::Captions)
            .map(|v| v.len())
            .unwrap_or(0);

        for entry in entries {
            if entry.text.trim().is_empty() {
                continue;
            }

            let id = format!("seg_{:03}", self.segments.len() + 1);
            self.segments.push(Segment {
                id: id.clone(),
                start: entry.start,
                end: entry.end,
                caption: entry.text.clone(),
                crossfade_ms,
                semantic_role: None,
            });

            let start_ms = (entry.start * 1000.0).round() as i64;
            let end_ms = (entry.end * 1000.0).round() as i64;
            let caption_event = TimelineEvent {
                id: format!("caption_{:03}", initial_caption_count + count + 1),
                asset_id: String::new(),
                start_ms,
                end_ms,
                offset_ms: 0,
                gain_db: 0.0,
                fade_in_ms: 0,
                fade_out_ms: 0,
                tags: vec![],
                provenance: Some(Provenance {
                    tool: "reelize.timeline".into(),
                    editorial_role: None,
                    concept: None,
                }),
                kind: EventKind::Caption {
                    text: entry.text,
                    style: "default".into(),
                    word_timings: vec![],
                },
            };
            self.add_track_event(TrackType::Captions, caption_event);
            count += 1;
        }

        if count > 0 {
            self.updated_at = chrono::Utc::now();
            self.assets.captions.insert(
                "srt".into(),
                serde_json::json!({"path": std::fs::canonicalize(srt_path).unwrap_or_else(|_| srt_path.into())}),
            );
        }

        Ok(count)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TimelineError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Validation failed: {0}")]
    Validation(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicU32, Ordering};

    static TEST_COUNTER: AtomicU32 = AtomicU32::new(0);

    fn write_temp_srt(content: &str) -> String {
        let dir = std::env::temp_dir();
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = dir.join(format!(
            "openscript_test_srt_{}_{}.srt",
            std::process::id(),
            n
        ));
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(content.as_bytes()).unwrap();
        path.to_string_lossy().to_string()
    }

    fn cleanup(path: &str) {
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_populate_segments_from_srt_basic() {
        let srt = "1\n00:00:00,000 --> 00:00:02,500\nHello world\n\n2\n00:00:03,000 --> 00:00:05,500\nSecond line\n\n";
        let path = write_temp_srt(srt);
        let mut timeline = Timeline::new("test.mp4".into(), "9:16", 30, None);
        let count = timeline.populate_segments_from_srt(&path, 500).unwrap();
        cleanup(&path);

        assert_eq!(count, 2);
        assert_eq!(timeline.segments.len(), 2);
        assert_eq!(timeline.segments[0].id, "seg_001");
        assert_eq!(timeline.segments[0].start, 0.0);
        assert_eq!(timeline.segments[0].end, 2.5);
        assert_eq!(timeline.segments[0].caption, "Hello world");
        assert_eq!(timeline.segments[1].id, "seg_002");
        assert_eq!(timeline.segments[1].start, 3.0);
        assert_eq!(timeline.segments[1].end, 5.5);
        assert_eq!(timeline.segments[1].caption, "Second line");

        let caption_events = timeline.get_track_events("captions");
        assert_eq!(caption_events.len(), 2);
    }

    #[test]
    fn test_populate_segments_from_srt_missing_file() {
        let mut timeline = Timeline::new("test.mp4".into(), "9:16", 30, None);
        let result = timeline.populate_segments_from_srt("/nonexistent/file.srt", 500);
        // The canonical srt::parse_srt returns an IO error for missing files;
        // populate_segments_from_srt wraps it with the file path.
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("/nonexistent/file.srt") || err.contains("Failed to parse SRT"),
            "Error should mention the file path or the parse failure, got: {}",
            err
        );
    }

    #[test]
    fn test_populate_segments_from_srt_empty_file() {
        let path = write_temp_srt("");
        let mut timeline = Timeline::new("test.mp4".into(), "9:16", 30, None);
        let count = timeline.populate_segments_from_srt(&path, 500).unwrap();
        cleanup(&path);
        assert_eq!(count, 0);
        assert_eq!(timeline.segments.len(), 0);
    }

    #[test]
    fn test_populate_segments_from_srt_multiline_caption() {
        let srt = "1\n00:00:00,000 --> 00:00:03,000\nLine one\nLine two\n\n";
        let path = write_temp_srt(srt);
        let mut timeline = Timeline::new("test.mp4".into(), "9:16", 30, None);
        let count = timeline.populate_segments_from_srt(&path, 500).unwrap();
        cleanup(&path);

        assert_eq!(count, 1);
        // The canonical srt::parse_srt joins multiline captions with a space
        // (not a newline, as the prior inline parser did). This is the
        // standard SRT behavior — the prior inline parser was the outlier.
        assert_eq!(timeline.segments[0].caption, "Line one Line two");
    }

    #[test]
    fn test_populate_segments_from_srt_crossfade_applied() {
        let srt = "1\n00:00:01,000 --> 00:00:04,000\nTest\n\n";
        let path = write_temp_srt(srt);
        let mut timeline = Timeline::new("test.mp4".into(), "9:16", 30, None);
        let count = timeline.populate_segments_from_srt(&path, 800).unwrap();
        cleanup(&path);

        assert_eq!(count, 1);
        assert_eq!(timeline.segments[0].crossfade_ms, 800);
    }
}

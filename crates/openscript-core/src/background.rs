//! Background video utilities for from-scratch video creation.
//!
//! Handles background video sourcing, cropping, and assignment:
//! - Gameplay: YouTube auto-download via yt-dlp + random clip extraction
//! - Procedural: FFmpeg-generated motion backgrounds (gradient waves, particles)
//! - Static: Single image with ken-burns zoom

use serde::{Deserialize, Serialize};

/// Background clip assignment for a scene.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackgroundClip {
    /// Scene ID this clip is assigned to.
    pub scene_id: String,
    /// Path to the background video file.
    pub path: String,
    /// Start time in the source video (seconds) for clip extraction.
    pub source_start_s: f64,
    /// Duration of the clip (seconds).
    pub duration_s: f64,
    /// Whether to loop this clip if scene is longer.
    pub loop_: bool,
}

/// Assign background clips to scenes based on the change_cadence.
///
/// Cadence options:
/// - "scene": each scene gets a different background (cycles through pool)
/// - "speaker": background changes when speaker changes
/// - "fixed": one background for all scenes
pub fn assign_backgrounds(
    scene_ids: &[String],
    scene_speakers: &[String],
    pool: &[String],
    cadence: &str,
    scene_durations_s: &[f64],
) -> Vec<BackgroundClip> {
    if pool.is_empty() || scene_ids.is_empty() {
        return Vec::new();
    }

    let mut clips = Vec::new();
    let mut pool_idx = 0usize;
    let mut last_speaker = String::new();

    for (i, scene_id) in scene_ids.iter().enumerate() {
        let speaker = scene_speakers.get(i).cloned().unwrap_or_default();
        let duration = scene_durations_s.get(i).copied().unwrap_or(10.0);

        let should_change = match cadence {
            "speaker" => speaker != last_speaker,
            "fixed" => i == 0, // only first scene picks, rest reuse
            _ => true,         // "scene" — always change
        };

        if should_change || clips.is_empty() {
            if !clips.is_empty() {
                // Advance to next pool item on change (not first scene)
                pool_idx = (pool_idx + 1) % pool.len();
            }
            last_speaker = speaker.clone();
        }

        // For fixed cadence, all scenes use the first clip
        let path = if cadence == "fixed" {
            pool[0].clone()
        } else {
            pool[pool_idx].clone()
        };

        clips.push(BackgroundClip {
            scene_id: scene_id.clone(),
            path,
            source_start_s: 0.0, // will be randomized by fetcher
            duration_s: duration,
            loop_: true,
        });
    }

    clips
}

/// Generate an FFmpeg filter graph for a procedural motion background.
///
/// Produces a gradient wave background with subtle motion — suitable for
/// "procedural" background type when no gameplay footage is available.
pub fn procedural_filter(width: u32, height: u32, duration_s: f64) -> Vec<String> {
    let w = width.to_string();
    let h = height.to_string();
    let d = duration_s.to_string();

    vec![
        format!(
            "color=c=0x0a0a1a:s={}x{}:d={}:r=30[bg]",
            w, h, d
        ),
        format!(
            "color=c=0x1a1a3a:s={}x{}:d={}:r=30[bg2]",
            w, h, d
        ),
        format!(
            "[bg][bg2]blend=all_mode=overlay:all_opacity=0.5[bg3]",
        ),
        format!(
            "[bg3]geq=r='128+80*sin(2*PI*X/W+0.1*T)':g='128+80*sin(2*PI*Y/H+0.15*T)':b='128+80*sin(2*PI*(X+Y)/(W+H)+0.2*T)'[v]",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_assign_scene_cadence() {
        let scenes = vec!["s1".to_string(), "s2".to_string(), "s3".to_string()];
        let speakers = vec!["alice".to_string(), "bob".to_string(), "alice".to_string()];
        let pool = vec!["bg1.mp4".to_string(), "bg2.mp4".to_string()];
        let durations = vec![5.0, 3.0, 4.0];

        let clips = assign_backgrounds(&scenes, &speakers, &pool, "scene", &durations);

        assert_eq!(clips.len(), 3);
        // Scene cadence: each scene advances to next pool item
        assert_eq!(clips[0].path, "bg1.mp4"); // first scene, pool_idx=0
        assert_eq!(clips[1].path, "bg2.mp4"); // advance, pool_idx=1
        assert_eq!(clips[2].path, "bg1.mp4"); // advance, wraps to 0
    }

    #[test]
    fn test_assign_speaker_cadence() {
        let scenes = vec!["s1".to_string(), "s2".to_string(), "s3".to_string()];
        let speakers = vec!["alice".to_string(), "alice".to_string(), "bob".to_string()];
        let pool = vec!["bg1.mp4".to_string(), "bg2.mp4".to_string()];
        let durations = vec![5.0, 3.0, 4.0];

        let clips = assign_backgrounds(&scenes, &speakers, &pool, "speaker", &durations);

        assert_eq!(clips.len(), 3);
        // Speaker cadence: change only when speaker changes
        assert_eq!(clips[0].path, "bg1.mp4"); // alice
        assert_eq!(clips[1].path, "bg1.mp4"); // alice (no change)
        assert_eq!(clips[2].path, "bg2.mp4"); // bob (change)
    }

    #[test]
    fn test_assign_fixed_cadence() {
        let scenes = vec!["s1".to_string(), "s2".to_string(), "s3".to_string()];
        let speakers = vec!["alice".to_string(), "bob".to_string(), "alice".to_string()];
        let pool = vec!["bg1.mp4".to_string(), "bg2.mp4".to_string()];
        let durations = vec![5.0, 3.0, 4.0];

        let clips = assign_backgrounds(&scenes, &speakers, &pool, "fixed", &durations);

        assert_eq!(clips.len(), 3);
        // Fixed: all scenes use the first clip
        assert_eq!(clips[0].path, "bg1.mp4");
        assert_eq!(clips[1].path, "bg1.mp4");
        assert_eq!(clips[2].path, "bg1.mp4");
    }

    #[test]
    fn test_assign_empty_pool() {
        let scenes = vec!["s1".to_string()];
        let speakers = vec!["alice".to_string()];
        let pool: Vec<String> = Vec::new();
        let durations = vec![5.0];

        let clips = assign_backgrounds(&scenes, &speakers, &pool, "scene", &durations);
        assert!(clips.is_empty());
    }

    #[test]
    fn test_assign_empty_scenes() {
        let scenes: Vec<String> = Vec::new();
        let speakers: Vec<String> = Vec::new();
        let pool = vec!["bg1.mp4".to_string()];
        let durations: Vec<f64> = Vec::new();

        let clips = assign_backgrounds(&scenes, &speakers, &pool, "scene", &durations);
        assert!(clips.is_empty());
    }

    #[test]
    fn test_assign_durations_preserved() {
        let scenes = vec!["s1".to_string(), "s2".to_string()];
        let speakers = vec!["alice".to_string(), "bob".to_string()];
        let pool = vec!["bg1.mp4".to_string()];
        let durations = vec![5.5, 3.2];

        let clips = assign_backgrounds(&scenes, &speakers, &pool, "scene", &durations);
        assert_eq!(clips[0].duration_s, 5.5);
        assert_eq!(clips[1].duration_s, 3.2);
    }

    #[test]
    fn test_procedural_filter() {
        let filters = procedural_filter(1080, 1920, 10.0);
        assert!(!filters.is_empty());
        assert!(filters[0].contains("1080"));
        assert!(filters[0].contains("1920"));
        assert!(filters[0].contains("10"));
    }
}

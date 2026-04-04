use openscript_core::timeline::EventKind;
use openscript_core::timeline::{Timeline, TimelineEvent};
use openscript_core::types::TrackType;
use std::path::PathBuf;

#[test]
fn test_timeline_create_save_load_roundtrip() {
    let dir = std::env::temp_dir();
    let path = dir.join("test_timeline_roundtrip.timeline.json");

    let mut timeline = Timeline::new(PathBuf::from("/tmp/test_video.mp4"), "9:16", 30, Some(60));

    timeline.add_segment(0.0, 2.5, "Hello world", 80, Some("hook"));
    timeline.add_segment(2.5, 5.0, "This is a test", 80, Some("setup"));
    timeline.add_segment(5.0, 8.0, "Final message here", 80, Some("cta"));

    timeline.save(&path).unwrap();
    assert!(path.exists());

    let loaded = Timeline::load(&path).unwrap();

    assert_eq!(loaded.version, "2.0");
    assert_eq!(loaded.segments.len(), 3);
    assert_eq!(loaded.segments[0].caption, "Hello world");
    assert_eq!(loaded.segments[1].caption, "This is a test");
    assert_eq!(loaded.segments[2].caption, "Final message here");
    assert_eq!(loaded.target.aspect, "9:16");
    assert_eq!(loaded.target.fps, 30);
    assert_eq!(loaded.target.max_duration, Some(60));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_timeline_add_delete_segments() {
    let dir = std::env::temp_dir();
    let path = dir.join("test_timeline_add_delete.timeline.json");

    let mut timeline = Timeline::new(PathBuf::from("/tmp/test.mp4"), "16:9", 30, None);

    for i in 0..5 {
        timeline.add_segment(
            i as f64 * 2.0,
            (i as f64 + 1.0) * 2.0,
            &format!("Segment {}", i + 1),
            80,
            None,
        );
    }

    assert_eq!(timeline.segments.len(), 5);
    assert_eq!(timeline.segments[0].id, "seg_001");
    assert_eq!(timeline.segments[4].id, "seg_005");

    timeline.save(&path).unwrap();
    let mut loaded = Timeline::load(&path).unwrap();

    // Remove middle segment (index 2)
    loaded.segments.remove(2);

    // Re-index IDs
    for (i, seg) in loaded.segments.iter_mut().enumerate() {
        seg.id = format!("seg_{:03}", i + 1);
    }

    assert_eq!(loaded.segments.len(), 4);
    assert_eq!(loaded.segments[0].id, "seg_001");
    assert_eq!(loaded.segments[3].id, "seg_004");

    loaded.save(&path).unwrap();
    let reloaded = Timeline::load(&path).unwrap();
    assert_eq!(reloaded.segments.len(), 4);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_timeline_validation() {
    let mut timeline = Timeline::new(PathBuf::from("/tmp/test.mp4"), "9:16", 30, None);

    timeline.add_segment(0.0, 2.0, "Good segment", 80, None);
    timeline.add_segment(2.0, 4.0, "Another good one", 80, None);

    let errors = timeline.validate();
    assert!(errors.is_empty(), "Valid timeline should have no errors");

    // Add invalid segment (start >= end)
    timeline.add_segment(5.0, 3.0, "Bad segment", 80, None);
    let errors = timeline.validate();
    assert!(!errors.is_empty(), "Invalid segment should produce errors");
    assert!(
        errors
            .iter()
            .any(|e| e.contains("start") && e.contains("end")),
        "Error should mention start and end: {:?}",
        errors
    );
}

#[test]
fn test_timeline_validation_overlap() {
    let mut timeline = Timeline::new(PathBuf::from("/tmp/test.mp4"), "9:16", 30, None);

    timeline.add_segment(0.0, 5.0, "First", 80, None);
    timeline.add_segment(3.0, 7.0, "Overlapping", 80, None);

    let errors = timeline.validate();
    assert!(!errors.is_empty());
    assert!(
        errors.iter().any(|e| e.contains("overlaps")),
        "Should detect overlapping segments: {:?}",
        errors
    );
}

#[test]
fn test_timeline_total_duration() {
    let mut timeline = Timeline::new(PathBuf::from("/tmp/test.mp4"), "9:16", 30, None);

    assert_eq!(timeline.total_duration_ms(), 0);

    timeline.add_segment(0.0, 2.5, "First", 80, None);
    assert_eq!(timeline.total_duration_ms(), 2500);

    timeline.add_segment(2.5, 5.0, "Second", 80, None);
    assert_eq!(timeline.total_duration_ms(), 5000);
}

#[test]
fn test_timeline_track_events() {
    let mut timeline = Timeline::new(PathBuf::from("/tmp/test.mp4"), "9:16", 30, None);

    timeline.add_segment(0.0, 5.0, "Test", 80, None);

    let music_event = TimelineEvent {
        id: "evt_music_001".to_string(),
        asset_id: "music_001".to_string(),
        start_ms: 0,
        end_ms: 5000,
        offset_ms: 0,
        gain_db: -12.0,
        fade_in_ms: 500,
        fade_out_ms: 500,
        tags: vec!["background".to_string()],
        provenance: None,
        kind: EventKind::Music {
            mood: "neutral".to_string(),
            energy: "medium".to_string(),
            bpm: None,
            loopability: true,
            intro_friendly: true,
            cta_friendly: false,
            loudness_target_lufs: -14.0,
            loop_mode: "loop".to_string(),
            ducking_policy: "auto".to_string(),
        },
    };

    timeline.add_track_event(TrackType::Music, music_event);

    assert_eq!(
        timeline
            .tracks
            .get(&TrackType::Music)
            .map(|e| e.len())
            .unwrap_or(0),
        1
    );
    assert_eq!(
        timeline
            .tracks
            .get(&TrackType::Dialogue)
            .map(|e| e.len())
            .unwrap_or(0),
        0
    );
}

#[test]
fn test_timeline_edl_v1_upgrade() {
    let v1_data = serde_json::json!({
        "source": "/tmp/test.mp4",
        "target": {"aspect": "9:16", "fps": 30},
        "segments": [
            {"id": "seg_001", "start": 0.0, "end": 2.5, "caption": "Hello", "crossfade_ms": 80},
            {"id": "seg_002", "start": 2.5, "end": 5.0, "caption": "World", "crossfade_ms": 80}
        ],
        "effects": {"burn_captions": true, "audio": {"loudnorm": true}}
    });

    let timeline = Timeline::from_edl_v1(&v1_data).unwrap();

    assert_eq!(timeline.version, "2.0");
    assert_eq!(timeline.segments.len(), 2);
    assert_eq!(timeline.segments[0].caption, "Hello");
    assert_eq!(timeline.segments[1].caption, "World");
    assert_eq!(timeline.target.aspect, "9:16");
    assert_eq!(timeline.target.fps, 30);
}

#[test]
fn test_timeline_all_tracks_initialized() {
    let timeline = Timeline::new(PathBuf::from("/tmp/test.mp4"), "9:16", 30, None);

    // All 6 track types should be initialized
    assert_eq!(timeline.tracks.len(), 6);
    assert!(timeline.tracks.contains_key(&TrackType::Dialogue));
    assert!(timeline.tracks.contains_key(&TrackType::Voiceover));
    assert!(timeline.tracks.contains_key(&TrackType::Captions));
    assert!(timeline.tracks.contains_key(&TrackType::Broll));
    assert!(timeline.tracks.contains_key(&TrackType::Music));
    assert!(timeline.tracks.contains_key(&TrackType::Sfx));

    // All should be empty
    for (track, events) in &timeline.tracks {
        assert!(
            events.is_empty(),
            "Track {:?} should be empty initially",
            track
        );
    }
}

#[test]
fn test_timeline_asset_registry() {
    let mut timeline = Timeline::new(PathBuf::from("/tmp/test.mp4"), "9:16", 30, None);

    timeline.add_asset(
        "music",
        "music_001".to_string(),
        serde_json::json!({
            "title": "Background Track",
            "duration_ms": 30000
        }),
    );

    assert_eq!(timeline.assets.music.len(), 1);
    assert!(timeline.assets.music.contains_key("music_001"));

    // Adding to unknown asset type should be a no-op
    timeline.add_asset("unknown_type", "x".to_string(), serde_json::json!({}));
    assert_eq!(timeline.assets.music.len(), 1); // unchanged
}

#[test]
fn test_timeline_ducking_directives() {
    let mut timeline = Timeline::new(PathBuf::from("/tmp/test.mp4"), "9:16", 30, None);

    timeline.add_ducking_directive("during_dialogue", "music", -10.0, 50, 200);

    assert_eq!(timeline.directives.ducking.len(), 1);
    assert_eq!(timeline.directives.ducking[0].target_track, "music");
    assert!((timeline.directives.ducking[0].reduction_db - (-10.0)).abs() < f64::EPSILON);
}

#[test]
fn test_srt_parse_and_group() {
    use openscript_core::srt::{group_entries, parse_srt, write_srt};

    let dir = std::env::temp_dir();
    let srt_path = dir.join("test.srt");

    let srt_content = "1\n00:00:00,000 --> 00:00:00,500\nHello\n\n2\n00:00:00,500 --> 00:00:01,000\nWorld\n\n3\n00:00:01,500 --> 00:00:02,000\nTest\n";
    std::fs::write(&srt_path, srt_content).unwrap();

    let entries = parse_srt(&srt_path).unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].text, "Hello");
    assert!((entries[0].start - 0.0).abs() < 0.001);
    assert!((entries[0].end - 0.5).abs() < 0.001);

    // Group with max_words=2, max_chars=20, max_gap=0.5
    let groups = group_entries(&entries, 2, 20, 0.5);
    assert_eq!(groups.len(), 2); // ["Hello World", "Test"]
    assert_eq!(groups[0].0, "Hello World");
    assert_eq!(groups[1].0, "Test");

    // Write back
    let out_path = dir.join("test_grouped.srt");
    write_srt(&groups, &out_path).unwrap();
    let content = std::fs::read_to_string(&out_path).unwrap();
    assert!(content.contains("Hello World"));
    assert!(content.contains("Test"));

    let _ = std::fs::remove_file(&srt_path);
    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn test_srt_parse_commas_and_periods() {
    use openscript_core::srt::parse_srt;

    let dir = std::env::temp_dir();
    let srt_path = dir.join("test_commas.srt");

    let srt_content = "1\n00:00:00,123 --> 00:00:01,456\nFirst entry\n\n";
    std::fs::write(&srt_path, srt_content).unwrap();

    let entries = parse_srt(&srt_path).unwrap();
    assert_eq!(entries.len(), 1);
    assert!((entries[0].start - 0.123).abs() < 0.001);
    assert!((entries[0].end - 1.456).abs() < 0.001);

    let _ = std::fs::remove_file(&srt_path);
}

#[test]
fn test_srt_write_and_reparse_roundtrip() {
    use openscript_core::srt::{parse_srt, write_srt};

    let dir = std::env::temp_dir();
    let original_path = dir.join("roundtrip_orig.srt");
    let roundtrip_path = dir.join("roundtrip_out.srt");

    let original = "1\n00:00:00,000 --> 00:00:02,000\nHello world\n\n2\n00:00:02,500 --> 00:00:05,000\nSecond line\n\n";
    std::fs::write(&original_path, original).unwrap();

    let entries = parse_srt(&original_path).unwrap();
    let groups: Vec<(String, f64, f64)> = entries
        .iter()
        .map(|e| (e.text.clone(), e.start, e.end))
        .collect();

    write_srt(&groups, &roundtrip_path).unwrap();
    let reparsed = parse_srt(&roundtrip_path).unwrap();

    assert_eq!(reparsed.len(), groups.len());
    for (i, (text, start, end)) in groups.iter().enumerate() {
        assert_eq!(reparsed[i].text, *text);
        assert!((reparsed[i].start - start).abs() < 0.001);
        assert!((reparsed[i].end - end).abs() < 0.001);
    }

    let _ = std::fs::remove_file(&original_path);
    let _ = std::fs::remove_file(&roundtrip_path);
}

#[test]
fn test_srt_retime() {
    use openscript_core::srt::retime_srt;

    // Original SRT entries with start, end, text
    let srt_entries = vec![
        (0.0, 1.0, "Hello".to_string()),
        (1.5, 2.5, "World".to_string()),
    ];

    // EDL segments (start, end in source video)
    let edl_segments = vec![(0.0, 2.5), (3.0, 5.5)];

    let retimed = retime_srt(&srt_entries, &edl_segments, 0.25);

    // Both entries should be included (both fall within EDL segment ranges)
    assert!(!retimed.is_empty());
    // Text should be preserved
    let texts: Vec<&str> = retimed.iter().map(|(_, _, t)| t.as_str()).collect();
    assert!(texts.contains(&"Hello"));
    assert!(texts.contains(&"World"));
}

#[test]
fn test_srt_empty_file() {
    use openscript_core::srt::parse_srt;

    let dir = std::env::temp_dir();
    let srt_path = dir.join("empty.srt");
    std::fs::write(&srt_path, "").unwrap();

    let entries = parse_srt(&srt_path).unwrap();
    assert!(entries.is_empty());

    let _ = std::fs::remove_file(&srt_path);
}

#[test]
fn test_srt_analysis() {
    use openscript_core::srt::analyze_srt;

    let entries = vec![
        ("um uh hello ai world".to_string(), 0.0, 3.0),
        ("great machine learning tutorial".to_string(), 3.0, 6.0),
    ];

    let results = analyze_srt(&entries);
    assert_eq!(results.len(), 2);

    // First entry has filler words
    assert!(results[0].filler_count > 0);
    // Second entry has keywords
    assert!(results[1].keywords_score > 0);
}

#[test]
fn test_segment_id_auto_increment() {
    let mut timeline = Timeline::new(PathBuf::from("/tmp/test.mp4"), "9:16", 30, None);

    let id1 = timeline.add_segment(0.0, 1.0, "First", 80, None);
    let id2 = timeline.add_segment(1.0, 2.0, "Second", 80, None);
    let id3 = timeline.add_segment(2.0, 3.0, "Third", 80, None);

    assert_eq!(id1, "seg_001");
    assert_eq!(id2, "seg_002");
    assert_eq!(id3, "seg_003");
}

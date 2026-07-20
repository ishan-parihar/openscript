use openscript_core::timeline::Segment;
use openscript_ffmpeg::filter_graph::{DuckingEvent, FilterGraphBuilder, MusicEvent};

fn make_segment(id: &str, start: f64, end: f64) -> Segment {
    Segment {
        id: id.into(),
        start,
        end,
        caption: "test".into(),
        crossfade_ms: 80,
        semantic_role: None,
    }
}

#[test]
fn test_filter_graph_single_segment() {
    let segments = vec![make_segment("seg_001", 0.0, 3.0)];

    let builder = FilterGraphBuilder::new(segments, 30, "9:16", true);
    let (filter, vout, aout) = builder.build();

    // Single segment: trim, fps, scale/crop, loudnorm (no concat, no xfade)
    assert!(filter.contains("trim=start=0:end=3"));
    assert!(filter.contains("atrim=start=0:end=3"));
    assert!(filter.contains("fps=30"));
    assert!(filter.contains("scale=-2:1920"));
    assert!(filter.contains("loudnorm=I=-16:TP=-2.5:LRA=11"));
    assert_eq!(vout, "[vcrop]");
    assert_eq!(aout, "[afinal]");
}

#[test]
fn test_filter_graph_multiple_segments() {
    let segments = vec![
        make_segment("seg_001", 0.0, 2.0),
        make_segment("seg_002", 2.5, 5.0),
        make_segment("seg_003", 5.0, 8.0),
    ];

    let builder = FilterGraphBuilder::new(segments, 30, "9:16", true);
    let (filter, vout, aout) = builder.build();

    // Should have 3 trims
    assert!(filter.contains("trim=start=0:end=2"));
    assert!(filter.contains("atrim=start=0:end=2"));
    assert!(filter.contains("trim=start=2.5:end=5"));
    assert!(filter.contains("atrim=start=2.5:end=5"));
    assert!(filter.contains("trim=start=5:end=8"));
    assert!(filter.contains("atrim=start=5:end=8"));

    // Should use xfade for video (not concat), concat for audio
    assert!(filter.contains("xfade=transition=smoothleft"));
    // Audio uses concat for reliability with PTS-reset segments
    assert!(filter.contains("concat=n=3:v=0:a=1[acat]"));
    // Video concat should NOT be used (xfade replaces it)
    assert!(!filter.contains("concat=n=3:v=1:a=1"));
    assert_eq!(vout, "[vcrop]");
    assert_eq!(aout, "[afinal]");
}

#[test]
fn test_filter_graph_16x9_aspect() {
    let segments = vec![make_segment("seg_001", 0.0, 3.0)];

    let builder = FilterGraphBuilder::new(segments, 30, "16:9", true);
    let (filter, vout, aout) = builder.build();

    // 16:9 now also crops (to 1920×1080) instead of leaving the source untouched.
    // Prior versions skipped the crop for non-9:16 aspects, which meant 16:9
    // renders used the source resolution verbatim. Now all three standard
    // aspects (9:16, 16:9, 1:1) produce a deterministic crop.
    assert!(filter.contains("scale=-2:1080"));
    assert!(filter.contains("crop=1920:1080"));
    assert_eq!(vout, "[vcrop]");
    assert_eq!(aout, "[afinal]");
}

#[test]
fn test_filter_graph_with_ass() {
    let segments = vec![make_segment("seg_001", 0.0, 3.0)];

    let builder =
        FilterGraphBuilder::new(segments, 30, "9:16", true).with_ass("/tmp/test.ass".to_string());
    let (filter, vout, aout) = builder.build();

    assert!(filter.contains("subtitles='/tmp/test.ass'"));
    assert_eq!(vout, "[vsub]");
    assert_eq!(aout, "[afinal]");
}

#[test]
fn test_filter_graph_no_loudnorm() {
    let segments = vec![make_segment("seg_001", 0.0, 3.0)];

    let builder = FilterGraphBuilder::new(segments, 30, "9:16", false);
    let (filter, _, aout) = builder.build();

    assert!(!filter.contains("loudnorm"));
    // Single segment: audio trim label passes through as a0
    assert_eq!(aout, "[a0]");
}

#[test]
fn test_filter_graph_negative_times_clamped() {
    // Segments with negative start should be clamped to 0
    let segments = vec![make_segment("seg_001", -1.0, 3.0)];

    let builder = FilterGraphBuilder::new(segments, 30, "9:16", false);
    let (filter, _, _) = builder.build();

    // Negative start should be clamped to 0
    assert!(filter.contains("trim=start=0:end=3"));
}

#[test]
fn test_filter_graph_end_before_start_clamped() {
    // Segment where end <= start should be clamped to start + epsilon
    let segments = vec![make_segment("seg_001", 3.0, 1.0)];

    let builder = FilterGraphBuilder::new(segments, 30, "9:16", false);
    let (filter, _, _) = builder.build();

    // Should have trim=start=3:end=3.001 (end clamped to start + 0.001)
    assert!(filter.contains("trim=start=3:end=3.001"));
}

#[test]
fn test_filter_graph_ass_path_escaping() {
    let segments = vec![make_segment("seg_001", 0.0, 1.0)];

    // Path with backslashes (Windows-style)
    let builder = FilterGraphBuilder::new(segments, 30, "9:16", false)
        .with_ass("C:\\path\\to\\captions.ass".to_string());
    let (filter, _, _) = builder.build();

    // Backslashes should be converted to forward slashes
    assert!(filter.contains("subtitles='C:/path/to/captions.ass'"));
}

#[test]
fn test_filter_graph_different_fps() {
    let segments = vec![make_segment("seg_001", 0.0, 2.0)];

    let builder = FilterGraphBuilder::new(segments, 60, "9:16", false);
    let (filter, _, _) = builder.build();

    assert!(filter.contains("fps=60"));
    assert!(!filter.contains("fps=30"));
}

#[test]
fn test_filter_graph_empty_segments() {
    let segments: Vec<Segment> = vec![];

    let builder = FilterGraphBuilder::new(segments, 30, "9:16", true);
    let (filter, vout, aout) = builder.build();

    // Empty segments: no filters, passthrough
    assert!(filter.is_empty());
    assert_eq!(vout, "[0:v]");
    assert_eq!(aout, "[0:a]");
}

#[test]
fn test_filter_graph_4k_aspect_not_scaled() {
    // Unknown aspect ratios should NOT get scaled/cropped
    let segments = vec![make_segment("seg_001", 0.0, 2.0)];

    let builder = FilterGraphBuilder::new(segments, 30, "4:3", false);
    let (filter, vout, _) = builder.build();

    assert!(!filter.contains("scale=-2:1920"));
    assert!(!filter.contains("crop=1080:1920"));
    assert_eq!(vout, "[vfps]");
}

#[test]
fn test_filter_graph_builder_chaining() {
    let segments = vec![make_segment("seg_001", 0.0, 2.0)];

    // Test that builder methods return Self for chaining
    let builder = FilterGraphBuilder::new(segments.clone(), 30, "9:16", true)
        .with_ass("/tmp/test.ass".to_string());
    let (filter1, vout1, _) = builder.build();

    assert!(filter1.contains("subtitles='/tmp/test.ass'"));
    assert_eq!(vout1, "[vsub]");
}

#[test]
fn test_filter_graph_srt_burn() {
    let segments = vec![make_segment("seg_001", 0.0, 3.0)];

    let builder =
        FilterGraphBuilder::new(segments, 30, "9:16", true).with_srt("/tmp/test.srt".to_string());
    let (filter, vout, _) = builder.build();

    assert!(filter.contains("subtitles='/tmp/test.srt'"));
    assert_eq!(vout, "[vsub]");
}

#[test]
fn test_filter_graph_overlay_mov() {
    let segments = vec![make_segment("seg_001", 0.0, 3.0)];

    let builder = FilterGraphBuilder::new(segments, 30, "9:16", true)
        .with_overlay_mov("/tmp/overlay.mov".to_string());
    let (filter, vout, _) = builder.build();

    assert!(filter.contains("movie='/tmp/overlay.mov'"));
    assert!(filter.contains("overlay=0:0"));
    assert_eq!(vout, "[vovl]");
}

#[test]
fn test_filter_graph_overlay_with_subtitles() {
    let segments = vec![make_segment("seg_001", 0.0, 3.0)];

    // Dual caption mode: both subtitles AND overlay MOV
    let builder = FilterGraphBuilder::new(segments, 30, "9:16", true)
        .with_ass("/tmp/test.ass".to_string())
        .with_overlay_mov("/tmp/overlay.mov".to_string());
    let (filter, vout, _) = builder.build();

    assert!(filter.contains("subtitles='/tmp/test.ass'"));
    assert!(filter.contains("movie='/tmp/overlay.mov'"));
    assert_eq!(vout, "[vovl]");
}

#[test]
fn test_filter_graph_xfade_offset_calculation() {
    let segments = vec![
        make_segment("seg_001", 0.0, 2.0),
        make_segment("seg_002", 2.5, 5.0),
    ];

    let builder = FilterGraphBuilder::new(segments, 30, "16:9", false);
    let (filter, _, _) = builder.build();

    // First xfade offset = duration of seg1 - overlap = 2.0 - 0.08 = 1.92
    assert!(filter.contains("offset=1.92"));
}

#[test]
fn test_filter_graph_crossfade_duration_from_segment() {
    let segments = vec![
        Segment {
            id: "seg_001".into(),
            start: 0.0,
            end: 2.0,
            caption: "test".into(),
            crossfade_ms: 200,
            semantic_role: None,
        },
        Segment {
            id: "seg_002".into(),
            start: 2.5,
            end: 5.0,
            caption: "test2".into(),
            crossfade_ms: 200,
            semantic_role: None,
        },
    ];

    let builder = FilterGraphBuilder::new(segments, 30, "16:9", false);
    let (filter, _, _) = builder.build();

    // Should use 200ms crossfade (0.2s) for video xfade
    assert!(filter.contains("xfade=transition=smoothleft:duration=0.2"));
    // Audio uses concat for reliability
    assert!(filter.contains("concat=n=2:v=0:a=1[acat]"));
}

#[test]
fn test_filter_graph_ducking_single_event() {
    let segments = vec![make_segment("seg_001", 0.0, 5.0)];
    let music = vec![MusicEvent {
        path: "/path/to/music.mp3".into(),
        volume: 0.3,
    }];
    let ducking = vec![DuckingEvent {
        start_ms: 0,
        end_ms: 5000,
        reduction_db: -10.0,
        attack_ms: 50,
        release_ms: 200,
    }];

    let builder = FilterGraphBuilder::new(segments, 30, "9:16", true)
        .with_music(music)
        .with_ducking(ducking);
    let (filter, _, aout) = builder.build();

    assert!(filter.contains("asplit=2[aloud_out][sidechain_src]"));
    assert!(filter.contains("[sidechain_src]sidechaincompress="));
    assert!(filter.contains("threshold=0.001"));
    assert!(filter.contains("ratio=4"));
    assert!(filter.contains("attack=50"));
    assert!(filter.contains("release=200"));
    assert!(filter.contains("makeup=1"));
    assert!(filter.contains("level_sc=1"));
    assert_eq!(aout, "[afinal]");
}

#[test]
fn test_filter_graph_ducking_multiple_events() {
    let segments = vec![make_segment("seg_001", 0.0, 10.0)];
    let music = vec![
        MusicEvent {
            path: "/path/to/music1.mp3".into(),
            volume: 0.3,
        },
        MusicEvent {
            path: "/path/to/music2.mp3".into(),
            volume: 0.5,
        },
    ];
    let ducking = vec![
        DuckingEvent {
            start_ms: 0,
            end_ms: 3000,
            reduction_db: -12.0,
            attack_ms: 30,
            release_ms: 150,
        },
        DuckingEvent {
            start_ms: 5000,
            end_ms: 8000,
            reduction_db: -8.0,
            attack_ms: 100,
            release_ms: 300,
        },
    ];

    let builder = FilterGraphBuilder::new(segments, 30, "9:16", true)
        .with_music(music)
        .with_ducking(ducking);
    let (filter, _, aout) = builder.build();

    assert!(filter.contains("asplit=2[aloud_out][sidechain_src]"));
    assert!(filter.contains("[music_vol_0][sidechain_src]sidechaincompress="));
    assert!(filter.contains("[music_vol_1][sidechain_src]sidechaincompress="));
    assert!(filter.contains("attack=30"));
    assert!(filter.contains("release=150"));
    assert!(filter.contains("music_ducked_0"));
    assert!(filter.contains("music_ducked_1"));
    assert_eq!(aout, "[afinal]");
}

#[test]
fn test_filter_graph_no_ducking_preserves_behavior() {
    let segments = vec![make_segment("seg_001", 0.0, 5.0)];
    let music = vec![MusicEvent {
        path: "/path/to/music.mp3".into(),
        volume: 0.3,
    }];

    let builder = FilterGraphBuilder::new(segments, 30, "9:16", true).with_music(music);
    let (filter, _, aout) = builder.build();

    assert!(!filter.contains("asplit=2"));
    assert!(!filter.contains("sidechaincompress"));
    assert!(filter.contains("[music_0]volume=0.3[music_vol_0]"));
    assert!(filter.contains("[aloud][music_vol_0]amix="));
    assert_eq!(aout, "[afinal]");
}

#[test]
fn test_filter_graph_ducking_with_music_no_events() {
    let segments = vec![make_segment("seg_001", 0.0, 3.0)];
    let music = vec![MusicEvent {
        path: "/path/to/music.mp3".into(),
        volume: 0.2,
    }];

    let builder = FilterGraphBuilder::new(segments, 30, "9:16", true)
        .with_music(music)
        .with_ducking(vec![]);
    let (filter, _, _) = builder.build();

    assert!(!filter.contains("asplit=2"));
    assert!(!filter.contains("sidechaincompress"));
    assert!(filter.contains("volume=0.2"));
}

// Quick test harness to audit MCP tools by calling handlers directly
use openscript_mcp::tools::*;
use serde_json::json;
use std::path::Path;

#[tokio::main]
async fn main() {
    let video = "/home/ishanp/Downloads/VID_20260402_011234328.mp4";
    
    if !Path::new(video).exists() {
        eprintln!("ERROR: Video not found: {}", video);
        std::process::exit(1);
    }

    println!("=== MCP TOOL AUDIT ===\n");
    println!("Source video: {}\n", video);

    // ============================================================
    // TEST 1: reelize.brief
    // ============================================================
    println!("[1/12] reelize.brief...");
    match route_tool("reelize.brief", json!({"video_path": video})).await {
        Ok(result) => {
            let segs = result.get("total_segments").and_then(|v| v.as_u64()).unwrap_or(0);
            let dur = result.get("source_duration_s").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let dial = result.get("total_dialogue_s").and_then(|v| v.as_f64()).unwrap_or(0.0);
            println!("  PASS: {} segments, {:.1}s source, {:.1}s dialogue", segs, dur, dial);
            // Print topic summary
            if let Some(topics) = result.get("topic_summary").and_then(|v| v.as_array()) {
                let mut sorted: Vec<_> = topics.iter().collect();
                sorted.sort_by(|a, b| {
                    let da = a.get("total_s").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let db = b.get("total_s").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    db.partial_cmp(&da).unwrap()
                });
                println!("  Top topics:");
                for t in sorted.iter().take(10) {
                    let topic = t.get("topic").and_then(|v| v.as_str()).unwrap_or("?");
                    let count = t.get("segment_count").and_then(|v| v.as_u64()).unwrap_or(0);
                    let total_s = t.get("total_s").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    println!("    - {} ({} segs, {:.1}s)", topic, count, total_s);
                }
            }
        }
        Err(e) => println!("  FAIL: {}", e),
    }

    // ============================================================
    // TEST 2: sfx.index
    // ============================================================
    println!("\n[2/12] sfx.index...");
    match route_tool("sfx.index", json!({})).await {
        Ok(result) => {
            let count = result.get("total").and_then(|v| v.as_u64()).unwrap_or(0);
            let cats = result.get("categories").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
            println!("  PASS: {} SFX, {} categories", count, cats);
        }
        Err(e) => println!("  FAIL: {}", e),
    }

    // ============================================================
    // TEST 3: sfx.search
    // ============================================================
    println!("\n[3/12] sfx.search...");
    match route_tool("sfx.search", json!({"query": "whoosh"})).await {
        Ok(result) => {
            let count = result.get("results").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
            println!("  PASS: {} results for 'whoosh'", count);
        }
        Err(e) => println!("  FAIL: {}", e),
    }

    // ============================================================
    // TEST 4: music.index
    // ============================================================
    println!("\n[4/12] music.index...");
    match route_tool("music.index", json!({})).await {
        Ok(result) => {
            let count = result.get("total").and_then(|v| v.as_u64()).unwrap_or(0);
            println!("  PASS: {} music tracks", count);
        }
        Err(e) => println!("  FAIL: {}", e),
    }

    // ============================================================
    // TEST 5: music.search
    // ============================================================
    println!("\n[5/12] music.search...");
    match route_tool("music.search", json!({"mood": "upbeat", "energy": "high"})).await {
        Ok(result) => {
            let count = result.get("results").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
            println!("  PASS: {} results for upbeat/high energy", count);
        }
        Err(e) => println!("  FAIL: {}", e),
    }

    // ============================================================
    // TEST 6: broll.suggest
    // ============================================================
    println!("\n[6/12] broll.suggest...");
    match route_tool("broll.suggest", json!({"topic_keywords": ["technology", "business"], "segment_count": 5})).await {
        Ok(result) => {
            let count = result.get("concepts").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
            println!("  PASS: {} b-roll concepts suggested", count);
        }
        Err(e) => println!("  FAIL: {}", e),
    }

    // ============================================================
    // TEST 7: broll.fetch (expect graceful degradation without PEXELS_API_KEY)
    // ============================================================
    println!("\n[7/12] broll.fetch...");
    match route_tool("broll.fetch", json!({"concepts": ["technology"], "orientation": "9:16", "quality": "sd", "download": true})).await {
        Ok(result) => {
            let status = result.get("status").and_then(|v| v.as_str()).unwrap_or("unknown");
            let cached = result.get("cached").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
            println!("  PASS: status={}, {} cached", status, cached);
        }
        Err(e) => println!("  FAIL: {}", e),
    }

    // ============================================================
    // TEST 8: timeline.build + timeline.validate
    // ============================================================
    println!("\n[8/12] timeline.build + timeline.validate...");
    match route_tool("timeline.build", json!({
        "video_path": video,
        "segments": [
            {"start": 0.0, "end": 5.0, "caption": "Test segment"}
        ],
        "aspect": "9:16",
        "fps": 30,
        "crossfade_ms": 300
    })).await {
        Ok(result) => {
            let path = result.get("timeline_path").and_then(|v| v.as_str()).unwrap_or("?");
            println!("  PASS: timeline built at {}", path);
            
            // Now validate it
            match route_tool("timeline.validate", json!({"timeline_path": path})).await {
                Ok(vr) => {
                    let valid = vr.get("valid").and_then(|v| v.as_bool()).unwrap_or(false);
                    let errs = vr.get("errors").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
                    println!("  validate: valid={}, {} errors", valid, errs);
                }
                Err(e) => println!("  validate FAIL: {}", e),
            }
        }
        Err(e) => println!("  FAIL: {}", e),
    }

    // ============================================================
    // TEST 9: voice.profile.list + voice.profile.add
    // ============================================================
    println!("\n[9/12] voice.profile.list...");
    match route_tool("voice.profile.list", json!({})).await {
        Ok(result) => {
            let count = result.get("profiles").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
            println!("  PASS: {} voice profiles", count);
        }
        Err(e) => println!("  FAIL: {}", e),
    }

    // ============================================================
    // TEST 10: tts.estimate_duration
    // ============================================================
    println!("\n[10/12] tts.estimate_duration...");
    match route_tool("tts.estimate_duration", json!({"text": "Hello world, this is a test of the TTS duration estimator."})).await {
        Ok(result) => {
            let dur = result.get("estimated_duration_s").and_then(|v| v.as_f64()).unwrap_or(0.0);
            println!("  PASS: estimated {:.2}s for test text", dur);
        }
        Err(e) => println!("  FAIL: {}", e),
    }

    // ============================================================
    // TEST 11: verify.captions
    // ============================================================
    println!("\n[11/12] verify.captions (with test ASS)...");
    // First create a test ASS file
    let test_ass = "/tmp/test_captions.ass";
    std::fs::write(test_ass, r#"[Script Info]
Title: Test
ScriptType: v4.00+
PlayResX: 1080
PlayResY: 1920

[V4+ Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
Style: Standard,Arial,48,&H00FFFFFF,&H000000FF,&H00000000,&H80000000,0,0,0,0,100,100,0,0,1,2,0,2,10,10,10,1

[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
Dialogue: 0,0:00:00.00,0:00:05.00,Standard,,0,0,0,,Test caption
"#).unwrap();
    
    match route_tool("verify.captions", json!({
        "ass_path": test_ass,
        "video_width": 1080,
        "video_height": 1920
    })).await {
        Ok(result) => {
            let valid = result.get("valid").and_then(|v| v.as_bool()).unwrap_or(false);
            let issues = result.get("issues").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
            println!("  PASS: valid={}, {} issues", valid, issues);
        }
        Err(e) => println!("  FAIL: {}", e),
    }

    // ============================================================
    // TEST 12: music.ducking.plan
    // ============================================================
    println!("\n[12/12] music.ducking.plan...");
    match route_tool("music.ducking.plan", json!({
        "timeline_path": "VID_20260402_011234328.timeline.json",
        "music_gain_db": -12.0,
        "duck_amount_db": -6.0
    })).await {
        Ok(result) => {
            let plan = result.get("plan").and_then(|v| v.as_str()).unwrap_or("");
            println!("  PASS: ducking plan generated ({} chars)", plan.len());
        }
        Err(e) => println!("  FAIL: {}", e),
    }

    println!("\n=== AUDIT COMPLETE ===");
}

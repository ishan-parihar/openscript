use openscript_mcp::tools::*;
use serde_json::json;
use std::path::Path;

#[tokio::main]
async fn main() {
    let video = "/home/ishanp/Downloads/VID_20260402_011234328.mp4";
    let output = "/home/ishanp/Downloads/instagram_reel_v6.mp4";

    if !Path::new(video).exists() {
        eprintln!("ERROR: Video not found: {}", video);
        std::process::exit(1);
    }

    println!("=== GENERATING INSTAGRAM REEL ===\n");
    println!("Source: {}", video);
    println!("Output: {}\n", output);

    match route_tool("reelize.direct", json!({
        "video_path": video,
        "segments": [
            {"start": 23.059, "end": 26.539, "id": "seg_007", "caption": "nepaal mein dekha kya haal hua?", "crossfade_ms": 200},
            {"start": 26.539, "end": 29.699, "id": "seg_008", "caption": "kyonki bhai sab logon ne Instagram par social media par post", "crossfade_ms": 200},
            {"start": 29.699, "end": 33.079, "id": "seg_009", "caption": "karna start kar diya tha and saare jainji ne awareness manaani", "crossfade_ms": 200},
            {"start": 33.079, "end": 36.06, "id": "seg_010", "caption": "start. Ki kitni corrupt sarkaar hai ki kitne fed up hai bhai", "crossfade_ms": 200},
            {"start": 108.959, "end": 111.62, "id": "seg_034", "caption": "Inke oopar se saare cases hat jaate hain chaahe vah rape ke", "crossfade_ms": 200},
            {"start": 111.62, "end": 115.06, "id": "seg_035", "caption": "ho murder ke ho criminal cases ho sab hat jaate hain", "crossfade_ms": 200},
            {"start": 115.06, "end": 117.42, "id": "seg_036", "caption": "ki zindagi ji rahe hain, ash ki zindagi ji rahe hain", "crossfade_ms": 200},
            {"start": 117.42, "end": 119.719, "id": "seg_037", "caption": "na paani mil rahi", "crossfade_ms": 200},
            {"start": 134.28, "end": 135.34, "id": "seg_044", "caption": "to bhai bhaad mein ja bhai", "crossfade_ms": 200},
            {"start": 136.12, "end": 141.18, "id": "seg_045", "caption": "Apna kaam banta hai, bhaad mein ja jaanta", "crossfade_ms": 200},
            {"start": 141.18, "end": 145.879, "id": "seg_046", "caption": "bhai, mere hisaab se to bhai, inka laabh zindaabaad", "crossfade_ms": 200},
            {"start": 145.879, "end": 149.319, "id": "seg_047", "caption": "aana chaahie bhai aur phir chizen sahi honi chaahie", "crossfade_ms": 200},
            {"start": 205.379, "end": 209.74, "id": "seg_058", "caption": "Bhai agar janta bachaani apna apna desh bachaana hai", "crossfade_ms": 200},
            {"start": 209.74, "end": 213.18, "id": "seg_059", "caption": "aapka ko bachaana hai apni bhai quality of life", "crossfade_ms": 200},
            {"start": 213.18, "end": 216.02, "id": "seg_060", "caption": "bhi kadar hai na, to bhai jara sa aware ho lo", "crossfade_ms": 200},
            {"start": 216.02, "end": 219.599, "id": "seg_061", "caption": "kya chal raha hai, mazaak se hat ke", "crossfade_ms": 200},
            {"start": 219.599, "end": 223.52, "id": "seg_062", "caption": "duniya ki aur phir jo phishiega na aam aadmi phishiega", "crossfade_ms": 200}
        ],
        "aspect": "9:16",
        "fps": 30,
        "crf": 20,
        "captions": {
            "enabled": true,
            "style": "kinetic"
        },
        "broll": [
            {"concept": "nepal protest", "overlay_at_s": 0, "duration_s": 3.5},
            {"concept": "social media instagram", "overlay_at_s": 3.5, "duration_s": 3.2},
            {"concept": "awareness crowd", "overlay_at_s": 6.7, "duration_s": 3.4},
            {"concept": "corruption government", "overlay_at_s": 10.1, "duration_s": 3},
            {"concept": "court justice", "overlay_at_s": 16.5, "duration_s": 2.7},
            {"concept": "crime police", "overlay_at_s": 19.2, "duration_s": 3.4},
            {"concept": "luxury lifestyle", "overlay_at_s": 22.6, "duration_s": 2.4},
            {"concept": "poverty water", "overlay_at_s": 25, "duration_s": 2.3},
            {"concept": "revolution protest", "overlay_at_s": 28.5, "duration_s": 1.1},
            {"concept": "angry crowd", "overlay_at_s": 29.6, "duration_s": 5.1},
            {"concept": "revolution flag", "overlay_at_s": 34.7, "duration_s": 4.7},
            {"concept": "hope sunrise", "overlay_at_s": 39.4, "duration_s": 3.4},
            {"concept": "india flag patriot", "overlay_at_s": 46.2, "duration_s": 4.4},
            {"concept": "quality life city", "overlay_at_s": 50.6, "duration_s": 3.4},
            {"concept": "awareness education", "overlay_at_s": 54, "duration_s": 2.8},
            {"concept": "serious news", "overlay_at_s": 56.8, "duration_s": 3.6},
            {"concept": "common people crowd", "overlay_at_s": 60.4, "duration_s": 3.9}
        ],
        "sfx": [
            {"role": "impact", "at_s": 0},
            {"role": "whoosh", "at_s": 3.5},
            {"role": "whoosh", "at_s": 10.1},
            {"role": "impact", "at_s": 16.5},
            {"role": "whoosh", "at_s": 22.6},
            {"role": "impact", "at_s": 28.5},
            {"role": "whoosh", "at_s": 34.7},
            {"role": "impact", "at_s": 46.2},
            {"role": "whoosh", "at_s": 54},
            {"role": "impact", "at_s": 60.4}
        ],
        "music": {
            "mood": "serious",
            "energy": "high",
            "gain_db": -14,
            "duck_under_dialogue": true
        },
        "output_path": output
    })).await {
        Ok(result) => {
            println!("\n=== REEL GENERATED SUCCESSFULLY ===\n");
            let status = result.get("status").and_then(|v| v.as_str()).unwrap_or("?");
            let out = result.get("output_path").and_then(|v| v.as_str()).unwrap_or("?");
            let dur = result.get("duration_s").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let segs = result.get("segments_count").and_then(|v| v.as_u64()).unwrap_or(0);
            let broll = result.get("broll_count").and_then(|v| v.as_u64()).unwrap_or(0);
            let sfx = result.get("sfx_count").and_then(|v| v.as_u64()).unwrap_or(0);
            let music = result.get("music_count").and_then(|v| v.as_u64()).unwrap_or(0);
            let vo = result.get("voiceover_count").and_then(|v| v.as_u64()).unwrap_or(0);
            let tl = result.get("timeline_path").and_then(|v| v.as_str()).unwrap_or("?");

            println!("Status: {}", status);
            println!("Output: {}", out);
            println!("Duration: {:.1}s", dur);
            println!("Segments: {}", segs);
            println!("B-roll: {}", broll);
            println!("SFX: {}", sfx);
            println!("Music: {}", music);
            println!("Voiceover: {}", vo);
            println!("Timeline: {}", tl);

            if let Some(warnings) = result.get("warnings") {
                if !warnings.is_null() {
                    if let Some(arr) = warnings.as_array() {
                        println!("\nWarnings ({}):", arr.len());
                        for w in arr {
                            println!("  - {}", w.as_str().unwrap_or("?"));
                        }
                    }
                }
            }
        }
        Err(e) => {
            println!("\n=== FAILED ===");
            println!("Error: {}", e);
            std::process::exit(1);
        }
    }
}

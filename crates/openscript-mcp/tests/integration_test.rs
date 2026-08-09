use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::process::{Command, Stdio};

/// Helper: start MCP server subprocess, return (stdin writer, stdout reader, child process)
fn start_mcp_server() -> (
    BufWriter<std::process::ChildStdin>,
    BufReader<std::process::ChildStdout>,
    std::process::Child,
) {
    // Use pre-built binary if available, fall back to cargo run
    let workspace_root = std::env::current_dir()
        .ok()
        .and_then(|d| d.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let release_bin = workspace_root.join("target/release/openscript");

    let mut child = if release_bin.exists() {
        Command::new(&release_bin)
            .arg("run-mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Failed to start MCP server binary")
    } else {
        Command::new("cargo")
            .args([
                "run",
                "--package",
                "openscript-cli",
                "--quiet",
                "--",
                "run-mcp",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Failed to start MCP server via cargo")
    };

    let stdin = BufWriter::new(child.stdin.take().unwrap());
    let stdout = BufReader::new(child.stdout.take().unwrap());

    (stdin, stdout, child)
}

/// Helper: send a JSON-RPC request via Content-Length framing and parse the response.
fn send_request(
    stdin: &mut BufWriter<std::process::ChildStdin>,
    stdout: &mut BufReader<std::process::ChildStdout>,
    method: &str,
    params: serde_json::Value,
    id: u64,
) -> serde_json::Value {
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    });
    let json_str = serde_json::to_string(&request).unwrap();

    // Send with Content-Length framing (matches server's read_message)
    let framing = format!("Content-Length: {}\r\n\r\n{}", json_str.len(), json_str);
    stdin.write_all(framing.as_bytes()).unwrap();
    stdin.flush().unwrap();

    // Read response: server uses Content-Length framing
    let mut header = String::new();
    stdout.read_line(&mut header).unwrap();

    let content_len: usize = header
        .strip_prefix("Content-Length: ")
        .unwrap_or_else(|| panic!("Expected Content-Length header, got: {}", header))
        .trim()
        .parse()
        .unwrap();

    // Read empty line separator
    let mut empty = String::new();
    stdout.read_line(&mut empty).unwrap();

    // Read exactly content_len bytes for the body
    let mut body = vec![0u8; content_len];
    Read::read_exact(stdout, &mut body).unwrap();

    let response: serde_json::Value = serde_json::from_slice(&body).unwrap();
    response
}

/// Helper: extract the parsed JSON payload from an MCP result response.
/// Tool-call results are wrapped as: {"content": [{"type": "text", "text": "<json>"}]}
/// Protocol results (initialize, tools/list) return raw objects directly.
fn extract_result_payload(response: &serde_json::Value) -> serde_json::Value {
    let result = response.get("result").expect("Response should have result");
    // Direct object (initialize, tools/list, etc.)
    if result.get("content").is_none() {
        return result.clone();
    }
    // Tool-call wrapper format
    let content = result.get("content").unwrap().as_array().unwrap();
    let text = content[0].get("text").unwrap().as_str().unwrap();
    serde_json::from_str(text).expect("Result text should be valid JSON")
}

/// Gracefully kill the subprocess and wait for it.
fn cleanup(mut child: std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

// ---------------------------------------------------------------------------
// Integration Tests
// ---------------------------------------------------------------------------

#[test]
fn test_mcp_initialize() {
    let (mut stdin, mut stdout, child) = start_mcp_server();

    let response = send_request(
        &mut stdin,
        &mut stdout,
        "initialize",
        serde_json::json!({}),
        1,
    );

    let payload = extract_result_payload(&response);
    assert!(
        payload.get("protocolVersion").is_some(),
        "initialize result should include protocolVersion"
    );
    assert!(
        payload.get("serverInfo").is_some(),
        "initialize result should include serverInfo"
    );

    cleanup(child);
}

#[test]
fn test_mcp_ping() {
    let (mut stdin, mut stdout, child) = start_mcp_server();

    let response = send_request(&mut stdin, &mut stdout, "ping", serde_json::json!({}), 1);

    assert!(
        response.get("result").is_some(),
        "ping should return a result"
    );

    cleanup(child);
}

#[test]
fn test_mcp_tools_list() {
    let (mut stdin, mut stdout, child) = start_mcp_server();

    send_request(
        &mut stdin,
        &mut stdout,
        "initialize",
        serde_json::json!({}),
        1,
    );

    let response = send_request(
        &mut stdin,
        &mut stdout,
        "tools/list",
        serde_json::json!({}),
        2,
    );

    let payload = extract_result_payload(&response);
    let tools = payload.get("tools").unwrap().as_array().unwrap();

    // 109 tools: 99 + asset.library.status + asset.ingest + asset.probe + asset.rate + asset.import + asset.search (asset-development pipeline) + voice.design + character.create + character.design_emotion + character.list + character.remove
    assert_eq!(
        tools.len(),
        109,
        "Expected 109 MCP tools, got {}",
        tools.len()
    );

    let tool_names: Vec<&str> = tools
        .iter()
        .map(|t| t.get("name").unwrap().as_str().unwrap())
        .collect();
    assert!(tool_names.contains(&"verify.production"));
    assert!(tool_names.contains(&"llm.complete"));
    assert!(tool_names.contains(&"vision.analyze_clip"));
    assert!(tool_names.contains(&"vision.score_clip"));
    assert!(tool_names.contains(&"broll.keywords"));
    assert!(tool_names.contains(&"broll.validate_keywords"));
    assert!(tool_names.contains(&"broll.repair"));
    assert!(tool_names.contains(&"broll.auto"));
    // Asset-development pipeline (user-curated footage library)
    assert!(tool_names.contains(&"asset.library.status"));
    assert!(tool_names.contains(&"asset.ingest"));
    assert!(tool_names.contains(&"asset.probe"));
    assert!(tool_names.contains(&"asset.rate"));
    assert!(tool_names.contains(&"asset.import"));
    assert!(tool_names.contains(&"asset.search"));
    assert!(tool_names.contains(&"sticker.keywords"));
    assert!(tool_names.contains(&"sticker.validate_keywords"));
    assert!(tool_names.contains(&"sticker.auto"));
    assert!(tool_names.contains(&"system.config.get"));
    assert!(tool_names.contains(&"system.config.set"));
    assert!(tool_names.contains(&"director.run"));
    assert!(tool_names.contains(&"system.doctor"));

    // Original tools (8)
    assert!(tool_names.contains(&"transcribe"));
    assert!(tool_names.contains(&"srt.read"));
    assert!(tool_names.contains(&"srt.prepare"));
    assert!(tool_names.contains(&"srt.apply_edit"));
    assert!(tool_names.contains(&"edl.build"));
    assert!(tool_names.contains(&"render"));
    assert!(tool_names.contains(&"reelize"));
    assert!(tool_names.contains(&"overlay.generate"));

    // Timeline V2 tools
    assert!(tool_names.contains(&"timeline.build"));
    assert!(tool_names.contains(&"timeline.load"));
    assert!(tool_names.contains(&"timeline.validate"));
    assert!(tool_names.contains(&"timeline.upgrade"));
    assert!(tool_names.contains(&"timeline.add_segment"));
    assert!(tool_names.contains(&"timeline.add_track_event"));
    assert!(tool_names.contains(&"timeline.diff"));
    assert!(tool_names.contains(&"timeline.preview"));
    assert!(tool_names.contains(&"timeline.autofill_broll"));

    // Voice/TTS tools
    assert!(tool_names.contains(&"voice.profile.add"));
    assert!(tool_names.contains(&"voice.profile.list"));
    assert!(tool_names.contains(&"voice.profile.remove"));
    assert!(tool_names.contains(&"tts.generate"));
    assert!(tool_names.contains(&"tts.estimate_duration"));
    // Character-first workflow (two-part voice-design)
    assert!(tool_names.contains(&"character.create"));
    assert!(tool_names.contains(&"character.design_emotion"));
    assert!(tool_names.contains(&"character.list"));
    assert!(tool_names.contains(&"character.remove"));
    assert!(tool_names.contains(&"tts.preview"));

    // Asset tools
    assert!(tool_names.contains(&"sfx.index"));
    assert!(tool_names.contains(&"sfx.search"));
    assert!(tool_names.contains(&"sfx.assign"));
    assert!(tool_names.contains(&"music.index"));
    // music.search removed (YAGNI) — agents use library.search instead
    assert!(tool_names.contains(&"music.assign"));
    assert!(tool_names.contains(&"music.ducking.plan"));

    // B-roll tools
    assert!(tool_names.contains(&"broll.suggest"));
    assert!(tool_names.contains(&"broll.fetch"));
    assert!(tool_names.contains(&"broll.assign"));

    // Voiceover tools
    assert!(tool_names.contains(&"voiceover.generate"));
    assert!(tool_names.contains(&"tts.commentary"));

    // Orchestration
    assert!(tool_names.contains(&"reelize"));

    // Verification tools
    assert!(tool_names.contains(&"verify.audio"));
    assert!(tool_names.contains(&"verify.captions"));
    assert!(tool_names.contains(&"verify.render"));

    cleanup(child);
}

#[test]
fn test_mcp_unknown_tool_returns_error() {
    let (mut stdin, mut stdout, child) = start_mcp_server();

    let response = send_request(
        &mut stdin,
        &mut stdout,
        "tools/call",
        serde_json::json!({"name": "nonexistent.tool", "arguments": {}}),
        1,
    );

    assert!(
        response.get("error").is_some(),
        "Unknown tool should return an error response"
    );

    cleanup(child);
}

#[test]
fn test_mcp_unknown_method_returns_error() {
    let (mut stdin, mut stdout, child) = start_mcp_server();

    let response = send_request(
        &mut stdin,
        &mut stdout,
        "nonexistent/method",
        serde_json::json!({}),
        1,
    );

    assert!(
        response.get("error").is_some(),
        "Unknown method should return an error response"
    );
    let error = response.get("error").unwrap();
    assert_eq!(
        error.get("code").and_then(|v| v.as_i64()).unwrap(),
        -32601,
        "Should return method not found error code"
    );

    cleanup(child);
}

#[test]
fn test_mcp_timeline_build_missing_video_returns_error() {
    let (mut stdin, mut stdout, child) = start_mcp_server();

    send_request(
        &mut stdin,
        &mut stdout,
        "initialize",
        serde_json::json!({}),
        1,
    );

    let response = send_request(
        &mut stdin,
        &mut stdout,
        "tools/call",
        serde_json::json!({
            "name": "timeline.build",
            "arguments": {
                "source_video": "/nonexistent/path/video.mp4",
                "aspect": "9:16",
                "fps": 30
            }
        }),
        2,
    );

    // File-not-found is an execution error → result.isError:true per MCP spec
    let has_error = response.get("error").is_some()
        || response
            .get("result")
            .and_then(|r| r.get("isError"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
    assert!(
        has_error,
        "timeline.build with missing video should return an error (JSON-RPC error or isError:true result)"
    );

    cleanup(child);
}

#[test]
fn test_mcp_voice_profile_list() {
    let (mut stdin, mut stdout, child) = start_mcp_server();

    send_request(
        &mut stdin,
        &mut stdout,
        "initialize",
        serde_json::json!({}),
        1,
    );

    let response = send_request(
        &mut stdin,
        &mut stdout,
        "tools/call",
        serde_json::json!({
            "name": "voice.profile.list",
            "arguments": {}
        }),
        2,
    );

    assert!(
        response.get("result").is_some(),
        "voice.profile.list should return a result"
    );
    let payload = extract_result_payload(&response);
    assert!(
        payload.get("profiles").is_some() || payload.get("status").is_some(),
        "voice.profile.list response should have profiles or status"
    );

    cleanup(child);
}

#[test]
fn test_mcp_srt_read_missing_file_returns_error() {
    let (mut stdin, mut stdout, child) = start_mcp_server();

    send_request(
        &mut stdin,
        &mut stdout,
        "initialize",
        serde_json::json!({}),
        1,
    );

    let response = send_request(
        &mut stdin,
        &mut stdout,
        "tools/call",
        serde_json::json!({
            "name": "srt.read",
            "arguments": {
                "srt_path": "/nonexistent/file.srt"
            }
        }),
        2,
    );

    // File-not-found is an execution error → result.isError:true per MCP spec
    let has_error = response.get("error").is_some()
        || response
            .get("result")
            .and_then(|r| r.get("isError"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
    assert!(
        has_error,
        "srt.read with missing file should return an error (JSON-RPC error or isError:true result)"
    );

    cleanup(child);
}

#[test]
fn test_mcp_tts_estimate_duration() {
    let (mut stdin, mut stdout, child) = start_mcp_server();

    send_request(
        &mut stdin,
        &mut stdout,
        "initialize",
        serde_json::json!({}),
        1,
    );

    let response = send_request(
        &mut stdin,
        &mut stdout,
        "tools/call",
        serde_json::json!({
            "name": "tts.estimate_duration",
            "arguments": {
                "text": "Hello world, this is a test"
            }
        }),
        2,
    );

    assert!(
        response.get("result").is_some(),
        "tts.estimate_duration should return a result"
    );
    let payload = extract_result_payload(&response);
    assert!(
        payload.get("estimated_duration_ms").is_some(),
        "TTS duration estimate should include estimated_duration_ms, got: {:?}",
        payload
    );

    cleanup(child);
}

#[test]
fn test_mcp_sfx_search() {
    let (mut stdin, mut stdout, child) = start_mcp_server();

    send_request(
        &mut stdin,
        &mut stdout,
        "initialize",
        serde_json::json!({}),
        1,
    );

    let response = send_request(
        &mut stdin,
        &mut stdout,
        "tools/call",
        serde_json::json!({
            "name": "sfx.search",
            "arguments": {
                "query": "boom",
                "limit": 5
            }
        }),
        2,
    );

    // SFX search should either return results or a graceful error if no library is configured.
    // The important thing is the server doesn't crash.
    if response.get("result").is_some() {
        let result = response.get("result").unwrap();
        // Check if it's an error result (isError: true) per MCP spec
        if result
            .get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            // Execution error returned as result.isError:true — acceptable
            let content = result.get("content").unwrap().as_array().unwrap();
            let text = content[0].get("text").unwrap().as_str().unwrap();
            assert!(
                text.contains("No such file")
                    || text.contains("not found")
                    || text.contains("Asset error")
                    || text.contains("error"),
                "Unexpected error content: {}",
                text
            );
        } else {
            // Success result — check payload
            let payload = extract_result_payload(&response);
            assert!(payload.get("results").is_some() || payload.get("status").is_some());
        }
    } else if response.get("error").is_some() {
        // Protocol-level error (JSON-RPC error) — acceptable for missing arg etc.
        let error = response.get("error").unwrap();
        let msg = error.get("message").unwrap().as_str().unwrap();
        assert!(
            msg.contains("No such file")
                || msg.contains("not found")
                || msg.contains("Asset error"),
            "Unexpected error message: {}",
            msg
        );
    } else {
        panic!("Expected either result or error response");
    }

    cleanup(child);
}

#[test]
fn test_mcp_srt_read_valid_file() {
    let (mut stdin, mut stdout, child) = start_mcp_server();

    send_request(
        &mut stdin,
        &mut stdout,
        "initialize",
        serde_json::json!({}),
        1,
    );

    // Create a temporary SRT file
    let dir = std::env::temp_dir();
    let srt_path = dir.join("test_mcp_srt_read.srt");
    let srt_content = "1\n00:00:00,000 --> 00:00:01,000\nHello world\n\n";
    std::fs::write(&srt_path, srt_content).unwrap();

    let response = send_request(
        &mut stdin,
        &mut stdout,
        "tools/call",
        serde_json::json!({
            "name": "srt.read",
            "arguments": {
                "srt_path": srt_path.to_string_lossy()
            }
        }),
        2,
    );

    assert!(
        response.get("result").is_some(),
        "srt.read should return a result for valid file"
    );
    let payload = extract_result_payload(&response);
    assert_eq!(payload.get("status").unwrap().as_str().unwrap(), "success");
    assert_eq!(payload.get("count").unwrap().as_u64().unwrap(), 1);

    let _ = std::fs::remove_file(&srt_path);
    cleanup(child);
}

#[test]
fn test_mcp_missing_required_arg_returns_error() {
    let (mut stdin, mut stdout, child) = start_mcp_server();

    send_request(
        &mut stdin,
        &mut stdout,
        "initialize",
        serde_json::json!({}),
        1,
    );

    // srt.read requires srt_path argument
    let response = send_request(
        &mut stdin,
        &mut stdout,
        "tools/call",
        serde_json::json!({
            "name": "srt.read",
            "arguments": {}
        }),
        2,
    );

    assert!(
        response.get("error").is_some(),
        "Missing required arg should return an error"
    );

    cleanup(child);
}

/// RAII guard: restore an env var on drop — even when the test panics — so
/// a mid-test failure can't leak mutated env to later tests in the same
/// binary (the repo flags env-mutating tests as flaky, AGENTS.md §16).
struct EnvGuard<'a> {
    key: &'a str,
    old: Option<String>,
}

impl<'a> EnvGuard<'a> {
    fn set(key: &'a str, value: std::path::PathBuf) -> Self {
        let old = std::env::var(key).ok();
        std::env::set_var(key, value);
        EnvGuard { key, old }
    }
}

impl Drop for EnvGuard<'_> {
    fn drop(&mut self) {
        match &self.old {
            Some(v) => std::env::set_var(self.key, v),
            None => std::env::remove_var(self.key),
        }
    }
}

/// Character registry round-trip WITHOUT invoking VoiceDesign (the slow model):
/// character.create with an existing voice profile → character.list sees it →
/// character.remove deletes it. Exercises the .openscript/characters.json
/// persistence path end-to-end through the MCP protocol.
#[test]
fn test_character_registry_roundtrip_with_existing_voice() {
    // The spawned server inherits CWD = crate dir, so .openscript/* relative
    // paths wouldn't resolve. Point the registries at the repo root (two
    // levels up) via env overrides; the guards restore them on drop (panic-safe).
    let cwd = std::env::current_dir().unwrap();
    let repo_root = cwd
        .ancestors()
        .nth(2)
        .unwrap_or(&cwd)
        .to_path_buf();
    let _vp_guard = EnvGuard::set(
        "OPENSCRIPT_VOICE_PROFILES_PATH",
        repo_root.join(".openscript/voice_profiles.json"),
    );
    let _cp_guard = EnvGuard::set(
        "OPENSCRIPT_CHARACTERS_PATH",
        repo_root.join(".openscript/characters.json"),
    );

    let (mut stdin, mut stdout, child) = start_mcp_server();

    send_request(
        &mut stdin,
        &mut stdout,
        "initialize",
        serde_json::json!({}),
        1,
    );

    let character_id = format!("itest_char_{}", std::process::id());
    let personality = "integration-test persona, calm measured tone";

    // create (no VoiceDesign — reuse the kokoro_af_heart profile as base voice)
    let resp = send_request(
        &mut stdin,
        &mut stdout,
        "tools/call",
        serde_json::json!({
            "name": "character.create",
            "arguments": {
                "character_id": character_id,
                "name": "Integration Tester",
                "role": "protagonist",
                "personality": personality,
                "voice": "kokoro_af_heart"
            }
        }),
        2,
    );
    let payload = extract_result_payload(&resp);
    assert_eq!(
        payload.get("status").unwrap().as_str().unwrap(),
        "character_created",
        "character.create failed: {}",
        payload
    );
    assert_eq!(
        payload.get("voice_profile_id").unwrap().as_str().unwrap(),
        "kokoro_af_heart",
        "existing voice must be reused verbatim"
    );

    // list sees it
    let resp = send_request(
        &mut stdin,
        &mut stdout,
        "tools/call",
        serde_json::json!({
            "name": "character.list",
            "arguments": {}
        }),
        3,
    );
    let payload = extract_result_payload(&resp);
    let chars = payload.get("characters").unwrap().as_array().unwrap();
    assert!(
        chars.iter().any(|c| c.get("character_id").and_then(|v| v.as_str()) == Some(character_id.as_str())),
        "created character missing from character.list"
    );

    // design_emotion requires a base voice profile; kokoro_af_heart is a kokoro
    // preset, so the emotions-map attach would be wrong — skip it here and just
    // verify remove.
    let resp = send_request(
        &mut stdin,
        &mut stdout,
        "tools/call",
        serde_json::json!({
            "name": "character.remove",
            "arguments": {"character_id": character_id}
        }),
        4,
    );
    let payload = extract_result_payload(&resp);
    assert_eq!(
        payload.get("status").unwrap().as_str().unwrap(),
        "character_removed",
        "character.remove failed: {}",
        payload
    );

    cleanup(child);
}

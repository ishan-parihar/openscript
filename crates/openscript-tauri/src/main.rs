#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod state;

use state::AppState;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[cfg(target_os = "linux")]
fn check_gstreamer_availability() {
    let required_elements = ["autoaudiosink", "playbin", "decodebin"];
    let mut missing = Vec::new();

    for element in &required_elements {
        let output = std::process::Command::new("gst-inspect-1.0")
            .arg(element)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();

        match output {
            Ok(status) if status.success() => {}
            _ => missing.push(*element),
        }
    }

    if !missing.is_empty() {
        tracing::warn!(
            "Missing GStreamer elements: {:?}. Video playback will fail (grey screen).",
            missing
        );
        tracing::warn!("Install: sudo pacman -S gst-plugins-base gst-plugins-good gst-libav");
    } else {
        tracing::info!("GStreamer media elements verified: all required elements available");
    }
}

fn start_media_server() -> std::thread::JoinHandle<()> {
    use std::io::{BufRead, BufReader, Read, Seek, Write};
    use std::net::TcpListener;

    fn send_response<W: Write>(writer: &mut W, status: u16, reason: &str, headers: &[(String, String)], body: &[u8]) {
        let _ = write!(writer, "HTTP/1.1 {} {}\r\n", status, reason);
        for (k, v) in headers {
            let _ = write!(writer, "{}: {}\r\n", k, v);
        }
        let _ = write!(writer, "Connection: close\r\n");
        let _ = write!(writer, "\r\n");
        let _ = writer.write_all(body);
        let _ = writer.flush();
    }

    std::thread::spawn(|| {
        let listener = match TcpListener::bind("127.0.0.1:1421") {
            Ok(l) => l,
            Err(e) => {
                tracing::error!("FAILED to bind media server on 127.0.0.1:1421: {}", e);
                return;
            }
        };
        tracing::info!("Media server bound and listening on http://127.0.0.1:1421");

        for stream in listener.incoming() {
            let Ok(mut stream) = stream else {
                tracing::warn!("Media server: failed to accept connection");
                continue;
            };

            let reader = BufReader::new(stream.try_clone().unwrap());
            let mut lines = reader.lines();

            // Parse request line
            let request_line = match lines.next() {
                Some(Ok(line)) => line,
                _ => continue,
            };
            let parts: Vec<&str> = request_line.split_whitespace().collect();
            if parts.len() < 2 {
                continue;
            }
            let _method = parts[0];
            let url = parts[1];

            // Parse headers
            let mut range: Option<String> = None;
            for line_result in lines.by_ref() {
                let line = match line_result {
                    Ok(l) => l,
                    Err(_) => break,
                };
                if line.is_empty() {
                    break; // End of headers
                }
                if let Some(val) = line.strip_prefix("Range:") {
                    range = Some(val.trim().to_string());
                }
            }

            let path = url.trim_start_matches("/file/");
            let decoded = urlencoding::decode(path).unwrap_or(std::borrow::Cow::Borrowed(path));
            let file_path = std::path::PathBuf::from(decoded.as_ref());

            if !file_path.exists() {
                send_response(&mut stream, 404, "Not Found", &[], b"File not found");
                tracing::warn!("Media server: 404 {:?}", file_path);
                continue;
            }

            let Ok(mut file) = std::fs::File::open(&file_path) else {
                send_response(&mut stream, 500, "Internal Server Error", &[], b"Cannot open file");
                continue;
            };

            let mime = infer::get_from_path(&file_path)
                .ok()
                .flatten()
                .map(|m| m.mime_type())
                .unwrap_or("video/mp4")
                .to_string();
            let total_size = file.metadata().map(|m| m.len()).unwrap_or(0);

            tracing::debug!("Media server: serving {:?} ({} bytes, {})", file_path, total_size, mime);

            if let Some(range_str) = range {
                if let Some(bytes_str) = range_str.strip_prefix("bytes=") {
                    if let Some((start_str, end_str)) = bytes_str.split_once('-') {
                        let start: u64 = start_str.parse().unwrap_or(0);
                        let end = if end_str.is_empty() {
                            total_size.saturating_sub(1)
                        } else {
                            end_str.parse().unwrap_or(total_size.saturating_sub(1))
                        };
                        let end = end.min(total_size.saturating_sub(1));
                        if start > end {
                            send_response(&mut stream, 416, "Range Not Satisfiable", &[], b"");
                            continue;
                        }
                        let chunk_size = (end - start + 1) as usize;
                        let mut buffer = vec![0u8; chunk_size];
                        if file.seek(std::io::SeekFrom::Start(start)).is_ok()
                            && file.read_exact(&mut buffer).is_ok()
                        {
                            let headers = vec![
                                ("Content-Type".to_string(), mime.clone()),
                                ("Accept-Ranges".to_string(), "bytes".to_string()),
                                ("Content-Range".to_string(), format!("bytes {}-{}/{}", start, end, total_size)),
                                ("Content-Length".to_string(), chunk_size.to_string()),
                            ];
                            send_response(&mut stream, 206, "Partial Content", &headers, &buffer);
                            tracing::debug!("Media server: 206 bytes {}-{}/{}", start, end, total_size);
                        } else {
                            send_response(&mut stream, 500, "Internal Server Error", &[], b"Read failed");
                        }
                        continue;
                    }
                }
                send_response(&mut stream, 400, "Bad Request", &[], b"Invalid Range header");
                continue;
            }

            // No Range header — serve entire file
            let mut buffer = Vec::new();
            if file.read_to_end(&mut buffer).is_ok() {
                let headers = vec![
                    ("Content-Type".to_string(), mime.clone()),
                    ("Accept-Ranges".to_string(), "bytes".to_string()),
                    ("Content-Length".to_string(), total_size.to_string()),
                ];
                send_response(&mut stream, 200, "OK", &headers, &buffer);
                tracing::debug!("Media server: 200 {} bytes", buffer.len());
            } else {
                send_response(&mut stream, 500, "Internal Server Error", &[], b"Read failed");
            }
        }
    })
}

fn main() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "openscript_tauri=debug,tauri=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    #[cfg(target_os = "linux")]
    check_gstreamer_availability();

    let _media_server = start_media_server();

    let app_state = AppState::new();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_shell::init())
        .manage(app_state)
        .setup(|_app| {
            tracing::info!("OpenScript Tauri app initialized");
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // Generic MCP dispatch (the "desktop as MCP client" pass-through)
            commands::invoke_tool::invoke_tool,
            commands::invoke_tool::list_mcp_tools,
            commands::invoke_tool::get_mcp_tool,
            // Legacy stateful commands (kept for backward compat + AppState management)
            // Note: system_capabilities, split_segment, validate_timeline,
            // timeline_preview, voice_profile_add/remove, verify_audio/captions
            // were deleted — they are superseded by invoke_tool(name, args)
            // which dispatches to the MCP route_tool() directly.
            // Project
            commands::project::create_project,
            commands::project::load_project,
            commands::project::list_projects,
            commands::project::save_project,
            // Timeline
            commands::timeline::add_segment,
            commands::timeline::get_timeline,
            commands::timeline::undo,
            commands::timeline::redo,
            commands::timeline::remove_segment,
            commands::timeline::update_segment,
            // Transcript
            commands::transcript::transcribe_video,
            commands::transcript::read_transcript,
            commands::transcript::prepare_transcript,
            commands::transcript::analyze_transcript,
            commands::transcript::remove_filler_words_from_text,
            commands::transcript::apply_transcript_edit,
            // Assets
            commands::assets::broll_fetch,
            commands::assets::broll_assign,
            commands::assets::music_search,
            commands::assets::music_assign,
            commands::assets::sfx_search,
            commands::assets::sfx_assign,
            // Render
            commands::render::reelize_timeline,
            commands::render::render_timeline,
            commands::render::get_render_progress,
            commands::render::cancel_render,
            // TTS
            commands::tts::voice_profile_list,
            commands::tts::tts_generate,
            commands::tts::tts_estimate_duration,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

use clap::{Parser, Subcommand};
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use openscript_core::timeline::Timeline;
use openscript_ui::app::App;
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io::stdout;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Parser)]
#[command(name = "openscript")]
#[command(about = "OpenScript Rust Video Editor - MCP Server & TUI")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the MCP server on stdio (default — also used as 'serve' alias)
    #[command(alias = "serve")]
    RunMcp,
    /// Start the TUI with a timeline file
    RunTui {
        /// Path to timeline JSON file
        #[arg(short, long)]
        timeline: Option<String>,
        /// Path to source video (creates new timeline)
        #[arg(short, long)]
        source: Option<String>,
    },
    /// Parse and validate a from-scratch video creation script (mirrors script.parse MCP tool)
    ScriptParse {
        /// Path to script JSON file
        #[arg(short, long)]
        script: String,
    },
    /// Generate TTS voices for each scene in a script (mirrors script.generate_voices)
    ScriptGenerateVoices {
        #[arg(short, long)]
        script: String,
        #[arg(long, default_value = "artifacts/voices")]
        output_dir: String,
    },
    /// Build captions from voiceover manifest (mirrors script.build_captions)
    ScriptBuildCaptions {
        #[arg(short, long)]
        script: String,
        #[arg(long)]
        voiceover_manifest: String,
        #[arg(long, default_value = "artifacts/captions.ass")]
        output_path: String,
    },
    /// Fetch a background video clip (mirrors background.fetch)
    BackgroundFetch {
        #[arg(long)]
        query: String,
        #[arg(long, default_value = "30")]
        duration_s: f64,
        #[arg(long, default_value = "9:16")]
        aspect: String,
    },
    /// Load an SVG sticker preset (mirrors sticker.load_preset)
    StickerLoadPreset {
        #[arg(long)]
        preset_name: String,
    },
    /// Render an animated sticker overlay (mirrors sticker.render)
    StickerRender {
        #[arg(long)]
        wav_path: String,
        #[arg(long)]
        preset_name: String,
        #[arg(long, default_value = "top-left")]
        position: String,
        #[arg(long, default_value = "0.25")]
        scale: f64,
        #[arg(long, default_value = "artifacts/sticker.html")]
        output_path: String,
    },
    /// Build a timeline from a script (mirrors script.to_timeline)
    ScriptToTimeline {
        #[arg(short, long)]
        script: String,
        #[arg(long, default_value = "artifacts")]
        output_dir: String,
        #[arg(long)]
        skip_background: bool,
        #[arg(long)]
        skip_stickers: bool,
    },
    /// One-call from-scratch video creation (mirrors script.to_video)
    ScriptToVideo {
        #[arg(short, long)]
        script: String,
        #[arg(long, default_value = "output.mp4")]
        output_path: String,
        #[arg(long, default_value = "artifacts")]
        output_dir: String,
        #[arg(long)]
        skip_background: bool,
        #[arg(long)]
        skip_stickers: bool,
        #[arg(long)]
        preview_mode: bool,
    },
    /// List all available MCP tools (for discovery)
    ListTools,
    /// Probe available subsystems (mirrors system.capabilities MCP tool)
    SystemCapabilities,
    /// Natural-language tool discovery (mirrors help.tool MCP tool)
    HelpTool {
        #[arg(short, long)]
        query: String,
        #[arg(long, default_value = "8")]
        limit: u32,
    },
    /// Search procedural backgrounds by mood (mirrors background.search)
    BackgroundSearch {
        #[arg(long)]
        mood: Option<String>,
        #[arg(long)]
        energy: Option<String>,
        #[arg(long)]
        motion_intensity: Option<String>,
        #[arg(long, default_value = "10")]
        limit: u32,
    },
    /// Verify audio quality of a rendered video (mirrors verify.audio)
    VerifyAudio {
        #[arg(short, long)]
        video_path: String,
    },
    /// Verify caption timing/burn of a rendered video (mirrors verify.captions)
    VerifyCaptions {
        #[arg(short, long)]
        video_path: String,
        #[arg(long)]
        srt_path: Option<String>,
    },
    /// Verify render quality of a video (mirrors verify.render)
    VerifyRender {
        #[arg(short, long)]
        video_path: String,
        #[arg(long)]
        timeline_path: String,
        #[arg(long, default_value = "9:16")]
        expected_aspect: String,
    },
    /// Production beauty KPI gate (mirrors verify.production) — stock/music/stickers grade
    VerifyProduction {
        #[arg(short, long)]
        video_path: String,
        #[arg(long)]
        timeline_path: String,
        #[arg(long)]
        captions_path: Option<String>,
        #[arg(long, default_value = "0")]
        sticker_count: u32,
        #[arg(long, default_value = "0")]
        meme_count: u32,
        #[arg(long, default_value = "B")]
        min_grade: String,
    },
    /// Search YouTube for videos (mirrors youtube.search)
    YoutubeSearch {
        #[arg(short, long)]
        query: String,
        #[arg(long, default_value = "5")]
        limit: u32,
    },
    /// Download a YouTube video clip (mirrors youtube.download)
    YoutubeDownload {
        #[arg(short, long)]
        url: String,
        #[arg(long)]
        start_s: Option<f64>,
        #[arg(long, default_value = "30")]
        duration_s: f64,
        #[arg(long, default_value = "9:16")]
        aspect: String,
        #[arg(long, default_value = "mcp/assets/background_cache")]
        output_dir: String,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("openscript=info".parse()?),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    // Default to RunMcp when no subcommand is given — MCP clients can invoke
    // the binary directly without specifying a subcommand.
    match cli.command.unwrap_or(Commands::RunMcp) {
        Commands::RunMcp => {
            tracing::info!("Starting OpenScript MCP server");
            openscript_mcp::server::run().await?;
        }
        Commands::RunTui { timeline, source } => {
            run_tui(timeline, source).await?;
        }
        Commands::ScriptParse { script } => {
            let args = serde_json::json!({"script": script});
            let result = openscript_mcp::tools::route_tool("script.parse", args).await;
            print_cli_result("script.parse", result);
        }
        Commands::ScriptGenerateVoices { script, output_dir } => {
            let args = serde_json::json!({"script": script, "output_dir": output_dir});
            let result = openscript_mcp::tools::route_tool("script.generate_voices", args).await;
            print_cli_result("script.generate_voices", result);
        }
        Commands::ScriptBuildCaptions {
            script,
            voiceover_manifest,
            output_path,
        } => {
            let args = serde_json::json!({
                "script": script,
                "voiceover_manifest": voiceover_manifest,
                "output_path": output_path,
            });
            let result = openscript_mcp::tools::route_tool("script.build_captions", args).await;
            print_cli_result("script.build_captions", result);
        }
        Commands::BackgroundFetch {
            query,
            duration_s,
            aspect,
        } => {
            let args = serde_json::json!({
                "query": query,
                "duration_s": duration_s,
                "aspect": aspect,
            });
            let result = openscript_mcp::tools::route_tool("background.fetch", args).await;
            print_cli_result("background.fetch", result);
        }
        Commands::StickerLoadPreset { preset_name } => {
            let args = serde_json::json!({"preset_name": preset_name});
            let result = openscript_mcp::tools::route_tool("sticker.load_preset", args).await;
            print_cli_result("sticker.load_preset", result);
        }
        Commands::StickerRender {
            wav_path,
            preset_name,
            position,
            scale,
            output_path,
        } => {
            let args = serde_json::json!({
                "wav_path": wav_path,
                "preset_name": preset_name,
                "position": position,
                "scale": scale,
                "output_path": output_path,
            });
            let result = openscript_mcp::tools::route_tool("sticker.render", args).await;
            print_cli_result("sticker.render", result);
        }
        Commands::ScriptToTimeline {
            script,
            output_dir,
            skip_background,
            skip_stickers,
        } => {
            let args = serde_json::json!({
                "script": script,
                "output_dir": output_dir,
                "skip_background": skip_background,
                "skip_stickers": skip_stickers,
            });
            let result = openscript_mcp::tools::route_tool("script.to_timeline", args).await;
            print_cli_result("script.to_timeline", result);
        }
        Commands::ScriptToVideo {
            script,
            output_path,
            output_dir,
            skip_background,
            skip_stickers,
            preview_mode,
        } => {
            // Resolve the final output path. If output_path is a bare filename
            // (no directory component), join it with output_dir. If output_path
            // is already an absolute or relative path with a directory, respect
            // it as-is. This fixes the round-2 UX audit GAP #8 where
            // --output-dir artifacts --output-path healing.mp4 produced
            // ./healing.mp4 in CWD instead of artifacts/healing.mp4.
            let resolved_output_path = {
                let p = std::path::Path::new(&output_path);
                if p.parent().map(|p| p.as_os_str().is_empty()).unwrap_or(true) {
                    // Bare filename — join with output_dir
                    std::path::Path::new(&output_dir)
                        .join(&output_path)
                        .to_string_lossy()
                        .to_string()
                } else {
                    // Path with a directory component — respect as-is
                    output_path.clone()
                }
            };
            let args = serde_json::json!({
                "script": script,
                "output_path": resolved_output_path,
                "output_dir": output_dir,
                "skip_background": skip_background,
                "skip_stickers": skip_stickers,
                "preview_mode": preview_mode,
            });
            let result = openscript_mcp::tools::route_tool("script.to_video", args).await;
            print_cli_result("script.to_video", result);
        }
        Commands::ListTools => {
            let tools = openscript_mcp::tools::tool_definitions();
            println!("{}", serde_json::to_string_pretty(&tools)?);
        }
        Commands::SystemCapabilities => {
            let result = openscript_mcp::tools::route_tool("system.capabilities", serde_json::json!({})).await;
            print_cli_result("system.capabilities", result);
        }
        Commands::HelpTool { query, limit } => {
            let args = serde_json::json!({"query": query, "limit": limit});
            let result = openscript_mcp::tools::route_tool("help.tool", args).await;
            print_cli_result("help.tool", result);
        }
        Commands::BackgroundSearch { mood, energy, motion_intensity, limit } => {
            let mut args = serde_json::json!({"limit": limit});
            if let Some(m) = mood { args["mood"] = serde_json::json!(m); }
            if let Some(e) = energy { args["energy"] = serde_json::json!(e); }
            if let Some(mi) = motion_intensity { args["motion_intensity"] = serde_json::json!(mi); }
            let result = openscript_mcp::tools::route_tool("background.search", args).await;
            print_cli_result("background.search", result);
        }
        Commands::VerifyAudio { video_path } => {
            let args = serde_json::json!({"video_path": video_path});
            let result = openscript_mcp::tools::route_tool("verify.audio", args).await;
            print_cli_result("verify.audio", result);
        }
        Commands::VerifyCaptions { video_path, srt_path } => {
            let mut args = serde_json::json!({"video_path": video_path});
            if let Some(s) = srt_path { args["srt_path"] = serde_json::json!(s); }
            let result = openscript_mcp::tools::route_tool("verify.captions", args).await;
            print_cli_result("verify.captions", result);
        }
        Commands::VerifyRender { video_path, timeline_path, expected_aspect } => {
            let args = serde_json::json!({"video_path": video_path, "timeline_path": timeline_path, "expected_aspect": expected_aspect});
            let result = openscript_mcp::tools::route_tool("verify.render", args).await;
            print_cli_result("verify.render", result);
        }
        Commands::VerifyProduction {
            video_path,
            timeline_path,
            captions_path,
            sticker_count,
            meme_count,
            min_grade,
        } => {
            let mut args = serde_json::json!({
                "video_path": video_path,
                "timeline_path": timeline_path,
                "sticker_count": sticker_count,
                "meme_count": meme_count,
                "min_grade": min_grade,
            });
            if let Some(c) = captions_path {
                args["captions_path"] = serde_json::json!(c);
            }
            let result = openscript_mcp::tools::route_tool("verify.production", args).await;
            print_cli_result("verify.production", result);
        }
        Commands::YoutubeSearch { query, limit } => {
            let args = serde_json::json!({"query": query, "limit": limit});
            let result = openscript_mcp::tools::route_tool("youtube.search", args).await;
            print_cli_result("youtube.search", result);
        }
        Commands::YoutubeDownload { url, start_s, duration_s, aspect, output_dir } => {
            let mut args = serde_json::json!({
                "url": url,
                "duration_s": duration_s,
                "aspect": aspect,
                "output_dir": output_dir
            });
            if let Some(s) = start_s { args["start_s"] = serde_json::json!(s); }
            let result = openscript_mcp::tools::route_tool("youtube.download", args).await;
            print_cli_result("youtube.download", result);
        }
    }

    Ok(())
}

/// Print a CLI tool result as JSON, or the error message.
fn print_cli_result(
    tool_name: &str,
    result: Result<serde_json::Value, openscript_mcp::error::ToolError>,
) {
    match result {
        Ok(val) => println!(
            "{}",
            serde_json::to_string_pretty(&val).unwrap_or_else(|_| val.to_string())
        ),
        Err(e) => {
            eprintln!("Error ({}): {}", tool_name, e);
            std::process::exit(1);
        }
    }
}

async fn run_tui(
    timeline_path: Option<String>,
    source: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let timeline: Arc<RwLock<Timeline>>;
    let path_str: String;

    if let Some(path) = timeline_path {
        let tl = Timeline::load(&path)?;
        path_str = path;
        timeline = Arc::new(RwLock::new(tl));
    } else if let Some(src) = source {
        let src_path = std::path::PathBuf::from(&src);
        let stem = src_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("timeline");
        let tl_path = format!("{}.timeline.json", stem);
        let tl = Timeline::new(src_path, "9:16", 30, None);
        tl.save(&tl_path)?;
        tracing::info!("Created new timeline: {}", tl_path);
        path_str = tl_path;
        timeline = Arc::new(RwLock::new(tl));
    } else {
        return Err("Either --timeline or --source is required".into());
    }

    stdout().execute(EnterAlternateScreen)?;
    execute!(stdout(), EnableMouseCapture)?;

    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.hide_cursor()?;

    let mut app = App::new(timeline, path_str);

    if let Err(e) = setup_file_watcher(&mut app).await {
        tracing::warn!("File watcher init failed (app will work without auto-reload): {e}");
    }

    let mut running = true;
    let tick_rate = std::time::Duration::from_millis(50);
    let mut render_rx: Option<
        tokio::sync::oneshot::Receiver<Result<String, Box<dyn std::error::Error + Send + Sync>>>,
    > = None;

    while running {
        app.process_file_events();

        terminal.draw(|f| openscript_ui::ui::render(f, &app))?;

        if let Some(rx) = &mut render_rx {
            if let Ok(result) = rx.try_recv() {
                match result {
                    Ok(output) => app.complete_render(output),
                    Err(e) => app.fail_render(e.to_string()),
                }
                render_rx = None;
            }
        }

        if crossterm::event::poll(tick_rate)? {
            if let crossterm::event::Event::Key(key) = crossterm::event::read()? {
                if key.kind != crossterm::event::KeyEventKind::Press {
                    continue;
                }

                match app.mode {
                    openscript_ui::app::AppMode::Normal => {
                        handle_normal_mode(&mut app, key.code, &mut running, &mut render_rx)
                    }
                    openscript_ui::app::AppMode::EditCaption => handle_caption_mode(&mut app, key),
                    openscript_ui::app::AppMode::AddingSegment => handle_adding_mode(&mut app, key),
                }
            }
        }
    }

    terminal.show_cursor()?;
    execute!(stdout(), DisableMouseCapture)?;
    execute!(stdout(), LeaveAlternateScreen)?;

    Ok(())
}

fn handle_normal_mode(
    app: &mut App,
    key: crossterm::event::KeyCode,
    running: &mut bool,
    render_rx: &mut Option<
        tokio::sync::oneshot::Receiver<Result<String, Box<dyn std::error::Error + Send + Sync>>>,
    >,
) {
    use crossterm::event::KeyCode;
    use openscript_ui::app::StatusType;

    if app.is_rendering {
        app.set_status(
            "Render in progress... press Esc to cancel",
            StatusType::Info,
        );
        if key == KeyCode::Esc {
            *render_rx = None;
            app.is_rendering = false;
            app.set_status("Render cancelled", StatusType::Info);
        }
        return;
    }

    match key {
        KeyCode::Char('j') | KeyCode::Down => app.navigate_down(),
        KeyCode::Char('k') | KeyCode::Up => app.navigate_up(),
        KeyCode::Char('t') => app.cycle_track(),
        KeyCode::Tab => app.toggle_view_focus(),

        KeyCode::Char('v') => {
            let preview = app.get_or_compute_preview();
            if preview.render_ready {
                app.set_status("Timeline is valid and render-ready", StatusType::Success);
            } else {
                app.set_status(
                    &format!("{} issue(s)", preview.validation_errors.len()),
                    StatusType::Error,
                );
            }
        }

        KeyCode::Enter => {
            let has_segments = app
                .timeline
                .try_read()
                .map(|tl| !tl.segments.is_empty())
                .unwrap_or(false);
            if has_segments {
                app.start_caption_edit();
            } else {
                app.set_status(
                    "No segments to edit. Add one first with 'a'",
                    StatusType::Info,
                );
            }
        }

        KeyCode::Char('a') => {
            app.start_add_segment();
        }

        KeyCode::Char('d') => {
            app.trigger_delete();
        }

        KeyCode::Char('r') => {
            if let Some(rx) = spawn_render_task(app) {
                *render_rx = Some(rx);
            }
        }

        KeyCode::Char('x') => {
            app.show_track_details = !app.show_track_details;
            app.set_status(
                if app.show_track_details {
                    "Track details shown"
                } else {
                    "Track details hidden"
                },
                StatusType::Info,
            );
        }

        KeyCode::Char('l') => match Timeline::load(&app.timeline_path) {
            Ok(tl) => {
                if let Ok(mut guard) = app.timeline.try_write() {
                    *guard = tl;
                }
                app.timeline_revision += 1;
                app.invalidate_preview();
                app.set_status("Timeline reloaded from disk", StatusType::Success);
            }
            Err(e) => app.set_status(&format!("Reload failed: {e}"), StatusType::Error),
        },

        KeyCode::Char('q') => *running = false,

        KeyCode::Esc => {
            app.cancel_delete();
            app.set_status("", StatusType::Info);
        }

        _ => {}
    }
}

fn handle_caption_mode(app: &mut App, key: crossterm::event::KeyEvent) {
    match key.code {
        crossterm::event::KeyCode::Enter => app.commit_caption(),
        crossterm::event::KeyCode::Esc => app.cancel_caption(),
        crossterm::event::KeyCode::Backspace => app.caption_input_backspace(),
        crossterm::event::KeyCode::Delete => app.caption_input_delete(),
        crossterm::event::KeyCode::Left => app.caption_input_left(),
        crossterm::event::KeyCode::Right => app.caption_input_right(),
        crossterm::event::KeyCode::Char(c) => app.caption_input_char(c),
        _ => {}
    }
}

fn handle_adding_mode(app: &mut App, key: crossterm::event::KeyEvent) {
    match key.code {
        crossterm::event::KeyCode::Enter => app.add_input_submit(),
        crossterm::event::KeyCode::Esc => app.cancel_add_segment(),
        crossterm::event::KeyCode::Backspace => app.add_input_backspace(),
        crossterm::event::KeyCode::Char(c) => app.add_input_char(c),
        _ => {}
    }
}

fn spawn_render_task(
    app: &mut App,
) -> Option<tokio::sync::oneshot::Receiver<Result<String, Box<dyn std::error::Error + Send + Sync>>>>
{
    use openscript_ui::app::StatusType;

    let render_data = match extract_render_data(app) {
        Ok(data) => data,
        Err(msg) => {
            app.set_status(&msg, StatusType::Error);
            return None;
        }
    };

    let edl_path = render_data.edl_path.clone();
    if let Err(e) = std::fs::write(
        &edl_path,
        serde_json::to_string_pretty(&render_data.edl_data).unwrap_or_default(),
    ) {
        app.set_status(&format!("Failed to write EDL: {e}"), StatusType::Error);
        return None;
    }

    app.start_render();

    let (tx, rx) = tokio::sync::oneshot::channel();

    tokio::spawn(async move {
        let config = openscript_ffmpeg::render::RenderConfig {
            video_path: render_data.video_path,
            edl_path: render_data.edl_path,
            burn_captions: render_data.burn_captions,
            srt_path: None,
            ass_path: None,
            overlay_mov: None,
            aspect: render_data.aspect,
            crf: 20,
            fps: render_data.fps,
        };

        let result = openscript_ffmpeg::render::render(config)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>);

        let _ = tx.send(result);
    });

    Some(rx)
}

struct RenderData {
    video_path: String,
    burn_captions: bool,
    aspect: String,
    fps: u32,
    edl_path: String,
    edl_data: serde_json::Value,
}

fn extract_render_data(app: &App) -> Result<RenderData, String> {
    let guard = app
        .timeline
        .try_read()
        .map_err(|_| "Timeline locked".to_string())?;

    if guard.segments.is_empty() {
        return Err("No segments to render".to_string());
    }

    let source = guard.source.clone();
    if !source.exists() {
        return Err(format!("Source video not found: {:?}", source));
    }

    let burn_captions = guard.effects.burn_captions;
    let aspect = guard.target.aspect.clone();
    let fps = guard.target.fps;
    let loudnorm = guard.effects.audio.loudnorm;
    let segments = guard.segments.clone();

    let edl_data = serde_json::json!({
        "version": "2.0",
        "source": source.to_string_lossy(),
        "target": {
            "aspect": &aspect,
            "fps": fps,
        },
        "segments": segments,
        "effects": {
            "burn_captions": burn_captions,
            "audio": {
                "loudnorm": loudnorm,
            },
        },
    });

    let edl_path = std::path::Path::new(&app.timeline_path)
        .with_extension("edl.json")
        .to_string_lossy()
        .to_string();

    Ok(RenderData {
        video_path: source.to_string_lossy().to_string(),
        burn_captions,
        aspect,
        fps,
        edl_path,
        edl_data,
    })
}

async fn setup_file_watcher(app: &mut App) -> Result<(), Box<dyn std::error::Error>> {
    use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
    use std::sync::mpsc as std_mpsc;

    let (tx, rx) = tokio::sync::mpsc::channel(32);
    let timeline_path_clone = app.timeline_path.clone();
    let watcher_tx = tx.clone();

    let (std_tx, std_rx) = std_mpsc::channel();
    let mut watcher = RecommendedWatcher::new(std_tx, Config::default())?;

    let watch_path = Path::new(&timeline_path_clone);
    if let Some(parent) = watch_path.parent() {
        watcher.watch(parent, RecursiveMode::NonRecursive)?;
    } else {
        watcher.watch(watch_path, RecursiveMode::NonRecursive)?;
    }

    tokio::spawn(async move {
        let timeline_path = timeline_path_clone;
        let basename = Path::new(&timeline_path)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        while let Ok(event) = std_rx.recv() {
            if let Ok(event) = event {
                for path in event.paths {
                    let fname = path
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default();
                    if fname == basename || fname.ends_with(".timeline.json") {
                        match event.kind {
                            EventKind::Modify(_) => {
                                let _ = watcher_tx
                                    .send(openscript_ui::app::FileWatchEvent::Modified)
                                    .await;
                            }
                            EventKind::Remove(_) => {
                                let _ = watcher_tx
                                    .send(openscript_ui::app::FileWatchEvent::Deleted)
                                    .await;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    });

    std::mem::forget(watcher);

    app.set_file_watcher(rx);
    Ok(())
}

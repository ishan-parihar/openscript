// ---------------------------------------------------------------------------
// tools_media — Stock media handlers (stock.fetch, youtube, media, gif, library)
// Split out of tools.rs (pure-move refactor). `use super::*` grants access
// to tools.rs's helpers, imports, and re-exported sibling handlers.
// ---------------------------------------------------------------------------
use super::*;

pub(crate) async fn handle_stock_fetch(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let media_type = extract_str(&args, "type")?; // "music" or "video"
    let query = extract_str(&args, "query")?;
    let limit = default_u32(&args, "limit", 5) as usize;
    let output_dir = default_str(&args, "output_dir", "mcp/assets/stock_cache");

    std::fs::create_dir_all(&output_dir)?;

    report_progress(0.0, 100.0, &format!("Searching for {}...", media_type))
        .await
        .ok();

    if media_type == "music" {
        // Try Pixabay music API
        let pixabay_key_val = if pixabay_key().is_empty() { None } else { Some(pixabay_key()) };
        if let Some(key) = pixabay_key_val {
            let url = format!(
                "https://pixabay.com/api/audio/?key={}&q={}&per_page={}",
                key,
                urlencoding::encode(query),
                limit
            );
            let client = reqwest::Client::new();
            match client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let body: serde_json::Value = resp
                        .json()
                        .await
                        .map_err(|e| ToolError::Asset(e.to_string()))?;
                    let hits = body.get("hits").cloned().unwrap_or(json!([]));
                    let mut results = Vec::new();

                    if let Some(arr) = hits.as_array() {
                        for hit in arr.iter().take(limit) {
                            let audio_url = hit.get("audio").and_then(|v| v.as_str()).unwrap_or("");
                            let title = hit
                                .get("tags")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown");
                            let duration =
                                hit.get("duration").and_then(|v| v.as_i64()).unwrap_or(0);

                            if !audio_url.is_empty() {
                                let filename = format!(
                                    "{}/{}_{}.mp3",
                                    output_dir,
                                    query.replace(' ', "_"),
                                    results.len()
                                );
                                match client.get(audio_url).send().await {
                                    Ok(resp) if resp.status().is_success() => {
                                        let bytes = resp
                                            .bytes()
                                            .await
                                            .map_err(|e| ToolError::Asset(e.to_string()))?;
                                        std::fs::write(&filename, &bytes)?;
                                        results.push(json!({
                                            "title": title,
                                            "path": filename,
                                            "duration_s": duration,
                                            "source": "pixabay",
                                        }));
                                    }
                                    Err(e) => {
                                        tracing::warn!("[stock.fetch] Download failed: {}", e)
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }

                    report_progress(
                        100.0,
                        100.0,
                        &format!("Downloaded {} tracks", results.len()),
                    )
                    .await
                    .ok();
                    return Ok(json!({
                        "status": "fetched",
                        "type": "music",
                        "source": "pixabay",
                        "count": results.len(),
                        "results": results,
                    }));
                }
                _ => tracing::warn!("[stock.fetch] Pixabay API failed"),
            }
        }

        // Fallback: return local stock library results
        report_progress(100.0, 100.0, "Using local stock library")
            .await
            .ok();
        return Ok(json!({
            "status": "fallback",
            "type": "music",
            "source": "local",
            "message": "Set PIXABAY_API_KEY env var to download from Pixabay. Using local stock library.",
            "local_library": "mcp/assets/music_index.json",
        }));
    }

    if media_type == "video" {
        // Try Pixabay video API
        let pixabay_key_val = if pixabay_key().is_empty() { None } else { Some(pixabay_key()) };
        if let Some(key) = pixabay_key_val {
            let video_type = default_str(&args, "video_type", "film");
            let url = format!(
                "https://pixabay.com/api/videos/?key={}&q={}&per_page={}&video_type={}",
                key,
                urlencoding::encode(query),
                limit,
                video_type
            );
            let client = reqwest::Client::new();
            match client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let body: serde_json::Value = resp
                        .json()
                        .await
                        .map_err(|e| ToolError::Asset(e.to_string()))?;
                    let hits = body.get("hits").cloned().unwrap_or(json!([]));
                    let mut results = Vec::new();

                    if let Some(arr) = hits.as_array() {
                        for hit in arr.iter().take(limit) {
                            // Get the best quality video URL
                            let videos = hit.get("videos");
                            let video_url = videos
                                .and_then(|v| v.get("large"))
                                .or_else(|| videos.and_then(|v| v.get("medium")))
                                .or_else(|| videos.and_then(|v| v.get("small")))
                                .and_then(|q| q.get("url"))
                                .and_then(|u| u.as_str())
                                .unwrap_or("");

                            let tags = hit
                                .get("tags")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown");
                            let duration =
                                hit.get("duration").and_then(|v| v.as_i64()).unwrap_or(0);

                            if !video_url.is_empty() {
                                let filename = format!(
                                    "{}/{}_{}.mp4",
                                    output_dir,
                                    query.replace(' ', "_"),
                                    results.len()
                                );
                                match client.get(video_url).send().await {
                                    Ok(resp) if resp.status().is_success() => {
                                        let bytes = resp
                                            .bytes()
                                            .await
                                            .map_err(|e| ToolError::Asset(e.to_string()))?;
                                        std::fs::write(&filename, &bytes)?;
                                        results.push(json!({
                                            "title": tags,
                                            "path": filename,
                                            "duration_s": duration,
                                            "source": "pixabay",
                                        }));
                                    }
                                    Err(e) => {
                                        tracing::warn!("[stock.fetch] Download failed: {}", e)
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }

                    report_progress(
                        100.0,
                        100.0,
                        &format!("Downloaded {} videos", results.len()),
                    )
                    .await
                    .ok();
                    return Ok(json!({
                        "status": "fetched",
                        "type": "video",
                        "source": "pixabay",
                        "count": results.len(),
                        "results": results,
                    }));
                }
                _ => tracing::warn!("[stock.fetch] Pixabay API failed"),
            }
        }

        // Fallback: return local stock library
        report_progress(100.0, 100.0, "Using local stock library")
            .await
            .ok();
        return Ok(json!({
            "status": "fallback",
            "type": "video",
            "source": "local",
            "message": "Set PIXABAY_API_KEY env var to download from Pixabay. Using local stock library.",
            "local_library": "mcp/assets/backgrounds/",
        }));
    }

    Err(ToolError::InvalidArg(format!(
        "Unknown media type: {}. Use 'music' or 'video'.",
        media_type
    )))
}

pub(crate) async fn handle_youtube_download(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let query = extract_str(&args, "query")?; // URL or search query
    let duration_s = default_f64(&args, "duration_s", 30.0);
    let start_s = default_opt_f64(&args, "start_s"); // Optional: specific start time
    let aspect = default_str(&args, "aspect", "9:16");
    let cache_dir = default_str(&args, "cache_dir", "mcp/assets/background_cache");
    let use_cookies = default_bool(&args, "use_cookies", true);

    std::fs::create_dir_all(&cache_dir)?;

    report_progress(0.0, 100.0, "Downloading from YouTube...")
        .await
        .ok();

    // Determine if query is a URL or a search term
    let is_url = query.starts_with("http://")
        || query.starts_with("https://")
        || query.starts_with("youtu.be");
    let cache_key = format!("{:x}", md5_hash(query.as_bytes()));
    let clip_path = format!("{}/{}_clip.mp4", cache_dir, cache_key);

    // If start_s is specified, use --download-sections to download only the range
    // This avoids downloading a 10-hour video when we only need 100 seconds
    if let Some(start) = start_s {
        let end = start + duration_s;
        let start_fmt = format_seconds_to_timestamp(start);
        let end_fmt = format_seconds_to_timestamp(end);
        let section_arg = format!("*{}-{}", start_fmt, end_fmt);

        report_progress(
            20.0,
            100.0,
            &format!("Downloading range {}-{}...", start_fmt, end_fmt),
        )
        .await
        .ok();

        let mut yt_args = vec![
            "--download-sections".to_string(),
            section_arg,
            "--force-keyframes-at-cuts".to_string(),
            "--format".to_string(),
            "best[height<=720]".to_string(),
            "--output".to_string(),
            clip_path.clone(),
            "--no-playlist".to_string(),
        ];

        if use_cookies {
            yt_args.push("--cookies-from-browser".to_string());
            yt_args.push("chrome".to_string());
        }
        yt_args.push("--user-agent".to_string());
        yt_args.push("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36".to_string());

        if is_url {
            yt_args.push(query.to_string());
        } else {
            yt_args.push(format!("ytsearch1:{}", query));
        }

        let yt_result = tokio::process::Command::new("yt-dlp")
            .args(&yt_args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .output()
            .await;

        match yt_result {
            Ok(output) if output.status.success() => {
                // Clip is already the right duration — just crop to aspect
                report_progress(70.0, 100.0, "Cropping to aspect ratio...")
                    .await
                    .ok();
                let (crop_w, crop_h) = aspect_to_crop_dims(&aspect);

                let cropped_path = format!("{}/{}_cropped.mp4", cache_dir, cache_key);
                // NOTE: intentionally NOT GPU-accelerated (unlike the golden-path
                // trims in tools::build_stock_trim_command). This is the legacy
                // youtube.download crop — different shape (no -t, crop-only) and
                // a ponytail path; keep it CPU until a caller migrates to the
                // unified background.fetch chain.
                let crop_result = tokio::process::Command::new("ffmpeg")
                    .arg("-y")
                    .arg("-i")
                    .arg(&clip_path)
                    .arg("-vf")
                    .arg(format!("crop={}:{}", crop_w, crop_h))
                    .arg("-c:v")
                    .arg("libx264")
                    .arg("-preset")
                    .arg("fast")
                    .arg("-crf")
                    .arg("23")
                    .arg("-an")
                    .arg(&cropped_path)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::piped())
                    .kill_on_drop(true)
                    .output()
                    .await;

                if let Ok(o) = crop_result {
                    if o.status.success() {
                        let _ = std::fs::rename(&cropped_path, &clip_path);
                    } else {
                        let _ = std::fs::remove_file(&cropped_path);
                    }
                }

                report_progress(100.0, 100.0, "Clip downloaded").await.ok();
                return Ok(json!({
                    "status": "downloaded",
                    "clip_path": clip_path,
                    "start_s": start,
                    "duration_s": duration_s,
                    "aspect": aspect,
                    "method": "range_download",
                    "cached": false,
                }));
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(ToolError::Ffmpeg(format!(
                    "YouTube range download failed: {}",
                    stderr.lines().last().unwrap_or("unknown error")
                )));
            }
            Err(e) => {
                return Err(ToolError::Ffmpeg(format!("yt-dlp not available: {}", e)));
            }
        }
    }

    // No start_s specified — download full video (or use cache), then extract random clip
    let full_video_path = format!("{}/{}.mp4", cache_dir, cache_key);

    // Check cache first
    if Path::new(&full_video_path).exists() {
        report_progress(50.0, 100.0, "Using cached video...")
            .await
            .ok();
    } else {
        // Build yt-dlp command
        let mut yt_args = vec![
            "--format".to_string(),
            "best[height<=720]".to_string(),
            "--output".to_string(),
            full_video_path.clone(),
            "--no-playlist".to_string(),
            "--quiet".to_string(),
        ];

        // Add cookies if enabled
        if use_cookies {
            // Try chrome first — if it fails, yt-dlp will continue without cookies
            yt_args.push("--cookies-from-browser".to_string());
            yt_args.push("chrome".to_string());
        }

        // Add user agent to avoid bot detection
        yt_args.push("--user-agent".to_string());
        yt_args.push("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36".to_string());

        // Search or direct URL
        if is_url {
            yt_args.push(query.to_string());
        } else {
            yt_args.push(format!("ytsearch1:{}", query));
        }

        let yt_result = tokio::process::Command::new("yt-dlp")
            .args(&yt_args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .output()
            .await;

        match yt_result {
            Ok(output) if output.status.success() => {
                report_progress(50.0, 100.0, "Downloaded, extracting clip...")
                    .await
                    .ok();
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                tracing::warn!("[youtube.download] yt-dlp failed: {}", stderr);
                return Err(ToolError::Ffmpeg(format!(
                    "YouTube download failed: {}. Try providing a direct URL, or set PIXABAY_API_KEY for stock footage.",
                    stderr.lines().last().unwrap_or("unknown error")
                )));
            }
            Err(e) => {
                return Err(ToolError::Ffmpeg(format!(
                    "yt-dlp not available: {}. Install with: pip install yt-dlp",
                    e
                )));
            }
        }
    }

    // Get video duration
    let probe_output = tokio::process::Command::new("ffprobe")
        .arg("-v")
        .arg("error")
        .arg("-show_entries")
        .arg("format=duration")
        .arg("-of")
        .arg("default=noprint_wrappers=1:nokey=1")
        .arg(&full_video_path)
        .output()
        .await;

    let source_duration_s: f64 = match probe_output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .trim()
            .parse()
            .unwrap_or(duration_s),
        _ => duration_s,
    };

    // Pick random start time
    let max_start = (source_duration_s - duration_s).max(0.0);
    let start_s = if max_start > 0.0 {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0) as u64;
        (seed as f64 / u64::MAX as f64) * max_start
    } else {
        0.0
    };

    // Crop dimensions
    let (crop_w, crop_h) = aspect_to_crop_dims(&aspect);

    // Extract clip with crop
    let extract_result = tokio::process::Command::new("ffmpeg")
        .arg("-y")
        .arg("-ss")
        .arg(start_s.to_string())
        .arg("-i")
        .arg(&full_video_path)
        .arg("-t")
        .arg(duration_s.to_string())
        .arg("-vf")
        .arg(format!("crop={}:{}", crop_w, crop_h))
        .arg("-c:v")
        .arg("libx264")
        .arg("-preset")
        .arg("fast")
        .arg("-crf")
        .arg("23")
        .arg("-an")
        .arg(&clip_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await;

    match extract_result {
        Ok(o) if o.status.success() => {
            report_progress(100.0, 100.0, "Clip extracted").await.ok();
            Ok(json!({
                "status": "downloaded",
                "clip_path": clip_path,
                "source_duration_s": source_duration_s,
                "start_s": start_s,
                "duration_s": duration_s,
                "aspect": aspect,
                "cached": Path::new(&full_video_path).exists(),
            }))
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            Err(ToolError::Ffmpeg(format!(
                "Clip extraction failed: {}",
                stderr
            )))
        }
        Err(e) => Err(ToolError::Ffmpeg(format!("FFmpeg failed: {}", e))),
    }
}

pub(crate) async fn handle_youtube_search(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let query = extract_str(&args, "query")?;
    let limit = default_u32(&args, "limit", 10) as usize;

    report_progress(0.0, 100.0, "Searching YouTube...")
        .await
        .ok();

    // Use yt-dlp to search (flat, no download)
    let search_query = format!("ytsearch{}:{}", limit, query);

    let result = tokio::process::Command::new("yt-dlp")
        .arg("--flat-playlist")
        .arg("--dump-json")
        .arg("--no-playlist")
        .arg(&search_query)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await;

    match result {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let mut results = Vec::new();

            for line in stdout.lines() {
                if let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) {
                    let title = entry
                        .get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Unknown");
                    let url = entry
                        .get("url")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| {
                            entry
                                .get("id")
                                .and_then(|v| v.as_str())
                                .map(|id| format!("https://youtube.com/watch?v={}", id))
                        })
                        .unwrap_or_default();
                    let duration = entry
                        .get("duration")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                    let view_count = entry
                        .get("view_count")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let uploader = entry
                        .get("uploader")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Unknown");

                    results.push(json!({
                        "title": title,
                        "url": url,
                        "duration_s": duration,
                        "view_count": view_count,
                        "uploader": uploader,
                    }));
                }
            }

            report_progress(100.0, 100.0, &format!("Found {} results", results.len()))
                .await
                .ok();

            Ok(json!({
                "status": "searched",
                "query": query,
                "count": results.len(),
                "results": results,
            }))
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(ToolError::Ffmpeg(format!(
                "YouTube search failed: {}",
                stderr.lines().last().unwrap_or("unknown error")
            )))
        }
        Err(e) => Err(ToolError::Ffmpeg(format!(
            "yt-dlp not available: {}. Install with: pip install yt-dlp",
            e
        ))),
    }
}

pub(crate) async fn handle_stock_search(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let media_type = extract_str(&args, "type")?;
    let query = extract_str(&args, "query")?;
    let limit = default_u32(&args, "limit", 10) as usize;

    report_progress(
        0.0,
        100.0,
        &format!("Searching Pixabay for {}...", media_type),
    )
    .await
    .ok();

    let pixabay_key_val = if pixabay_key().is_empty() { None } else { Some(pixabay_key()) };

    if let Some(key) = pixabay_key_val {
        let endpoint = if media_type == "music" {
            "https://pixabay.com/api/audio/"
        } else {
            "https://pixabay.com/api/videos/"
        };

        let video_type = default_str(&args, "video_type", "film");
        let url = if media_type == "music" {
            format!(
                "{}?key={}&q={}&per_page={}",
                endpoint,
                key,
                urlencoding::encode(query),
                limit
            )
        } else {
            format!(
                "{}?key={}&q={}&per_page={}&video_type={}",
                endpoint,
                key,
                urlencoding::encode(query),
                limit,
                video_type
            )
        };

        let client = reqwest::Client::new();
        match client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => {
                let body: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| ToolError::Asset(e.to_string()))?;

                let total = body.get("totalHits").and_then(|v| v.as_u64()).unwrap_or(0);
                let hits = body.get("hits").cloned().unwrap_or(json!([]));

                let results: Vec<serde_json::Value> = hits
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .take(limit)
                            .map(|hit| {
                                let title = hit
                                    .get("tags")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("Unknown");
                                let duration =
                                    hit.get("duration").and_then(|v| v.as_i64()).unwrap_or(0);
                                let user = hit
                                    .get("user")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("Unknown");
                                let views = hit.get("views").and_then(|v| v.as_u64()).unwrap_or(0);
                                let likes = hit.get("likes").and_then(|v| v.as_u64()).unwrap_or(0);

                                if media_type == "music" {
                                    let preview_url =
                                        hit.get("audio").and_then(|v| v.as_str()).unwrap_or("");
                                    json!({
                                        "title": title,
                                        "duration_s": duration,
                                        "user": user,
                                        "views": views,
                                        "likes": likes,
                                        "preview_url": preview_url,
                                    })
                                } else {
                                    let videos = hit.get("videos");
                                    let thumb = hit
                                        .get("previewURL")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("");
                                    let video_url = videos
                                        .and_then(|v| v.get("large"))
                                        .or_else(|| videos.and_then(|v| v.get("medium")))
                                        .or_else(|| videos.and_then(|v| v.get("small")))
                                        .and_then(|q| q.get("url"))
                                        .and_then(|u| u.as_str())
                                        .unwrap_or("");
                                    json!({
                                        "title": title,
                                        "duration_s": duration,
                                        "user": user,
                                        "views": views,
                                        "likes": likes,
                                        "thumbnail": thumb,
                                        "video_url": video_url,
                                    })
                                }
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                report_progress(100.0, 100.0, &format!("Found {} results", results.len()))
                    .await
                    .ok();

                return Ok(json!({
                    "status": "searched",
                    "type": media_type,
                    "source": "pixabay",
                    "query": query,
                    "total_hits": total,
                    "count": results.len(),
                    "results": results,
                }));
            }
            _ => tracing::warn!("[stock.search] Pixabay API failed"),
        }
    }

    // Fallback: list local stock library
    report_progress(100.0, 100.0, "Using local stock library")
        .await
        .ok();

    if media_type == "music" {
        let index_path = std::env::var("OPENSCRIPT_MUSIC_INDEX")
            .unwrap_or_else(|_| "mcp/assets/music_index.json".to_string());
        if let Ok(content) = std::fs::read_to_string(&index_path) {
            if let Ok(index) = serde_json::from_str::<serde_json::Value>(&content) {
                let assets = index.get("assets").cloned().unwrap_or(json!([]));
                let results: Vec<serde_json::Value> = assets
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter(|a| {
                                let title = a
                                    .get("title")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_lowercase();
                                let mood = a
                                    .get("mood")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_lowercase();
                                title.contains(&query.to_lowercase())
                                    || mood.contains(&query.to_lowercase())
                            })
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();
                return Ok(json!({
                    "status": "fallback",
                    "type": "music",
                    "source": "local",
                    "query": query,
                    "count": results.len(),
                    "results": results,
                    "message": "Set PIXABAY_API_KEY to search Pixabay. Showing local library matches.",
                }));
            }
        }
    }

    // Video fallback: list local backgrounds
    let bg_dir = "mcp/assets/backgrounds";
    let mut results = Vec::new();
    if let Ok(entries) = std::fs::read_dir(bg_dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".mp4") {
                let path = format!("{}/{}", bg_dir, name);
                results.push(json!({
                    "title": name,
                    "path": path,
                    "source": "local",
                }));
            }
        }
    }

    Ok(json!({
        "status": "fallback",
        "type": media_type,
        "source": "local",
        "query": query,
        "count": results.len(),
        "results": results,
        "message": "Set PIXABAY_API_KEY to search Pixabay. Showing local library.",
    }))
}

pub(crate) async fn handle_media_search(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let query = extract_str(&args, "query")?;
    let limit = default_u32(&args, "limit", 10) as usize;
    let source = default_str(&args, "source", "auto");

    report_progress(0.0, 100.0, &format!("Searching for images: {}...", query))
        .await
        .ok();

    let pexels_key_val = pexels_key();

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| ToolError::Asset(format!("HTTP client error: {}", e)))?;

    // Try Pexels Images API first (if key available and source allows)
    if source != "openverse" && !pexels_key().is_empty() {
        let url = format!(
            "https://api.pexels.com/v1/search?query={}&per_page={}&orientation=portrait",
            urlencoding::encode(query),
            limit
        );

        match client
            .get(&url)
            .header("Authorization", &pexels_key_val)
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                let body: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| ToolError::Asset(format!("Pexels parse error: {}", e)))?;

                let results: Vec<serde_json::Value> = body.get("photos")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().take(limit).map(|p| {
                        let src = p.get("src").cloned().unwrap_or(json!({}));
                        json!({
                            "id": p.get("id"),
                            "title": format!("Photo by {}", p.get("photographer").and_then(|v| v.as_str()).unwrap_or("Unknown")),
                            "url": src.get("original").and_then(|v| v.as_str()).unwrap_or(""),
                            "medium_url": src.get("medium").and_then(|v| v.as_str()).unwrap_or(""),
                            "large_url": src.get("large").and_then(|v| v.as_str()).unwrap_or(""),
                            "width": p.get("width"),
                            "height": p.get("height"),
                            "source": "pexels",
                            "license": "pexels-license",
                        })
                    }).collect())
                    .unwrap_or_default();

                if !results.is_empty() {
                    report_progress(100.0, 100.0, &format!("Found {} images", results.len()))
                        .await
                        .ok();
                    return Ok(json!({
                        "status": "searched",
                        "query": query,
                        "source": "pexels",
                        "count": results.len(),
                        "results": results,
                    }));
                }
            }
            _ => tracing::warn!("[media.search] Pexels API failed, trying Openverse"),
        }
    }

    // Fallback: Openverse API (free, no key needed)
    if source != "pexels" {
        let url = format!(
            "https://api.openverse.org/v1/images/?q={}&page_size={}",
            urlencoding::encode(query),
            limit
        );

        match client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => {
                let body: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| ToolError::Asset(format!("Openverse parse error: {}", e)))?;

                let results: Vec<serde_json::Value> = body
                    .get("results")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .take(limit)
                            .map(|r| {
                                json!({
                                    "id": r.get("id"),
                                    "title": r.get("title"),
                                    "url": r.get("url"),
                                    "thumbnail": r.get("thumbnail"),
                                    "width": r.get("width"),
                                    "height": r.get("height"),
                                    "source": "openverse",
                                    "license": r.get("license"),
                                    "creator": r.get("creator"),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                report_progress(100.0, 100.0, &format!("Found {} images", results.len()))
                    .await
                    .ok();
                return Ok(json!({
                    "status": "searched",
                    "query": query,
                    "source": "openverse",
                    "count": results.len(),
                    "results": results,
                }));
            }
            _ => tracing::warn!("[media.search] Openverse API failed"),
        }
    }

    Ok(json!({
        "status": "no_results",
        "query": query,
        "count": 0,
        "results": [],
        "message": "No images found. Try a different query.",
    }))
}

pub(crate) async fn handle_gif_search(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let query = extract_str(&args, "query")?;
    let limit = default_u32(&args, "limit", 10) as usize;
    let rating = default_str(&args, "rating", "g");

    report_progress(0.0, 100.0, &format!("Searching GIPHY for: {}...", query))
        .await
        .ok();

    let giphy_key = Some(giphy_key()).filter(|s| !s.is_empty());

    if let Some(key) = giphy_key {
        if !key.is_empty() {
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .map_err(|e| ToolError::Asset(format!("HTTP client error: {}", e)))?;

            // Search GIPHY stickers (transparent GIFs)
            let url = format!(
                "https://api.giphy.com/v1/stickers/search?api_key={}&q={}&limit={}&rating={}&bundle=sticker_layering",
                key,
                urlencoding::encode(query),
                limit,
                rating
            );

            match client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let body: serde_json::Value = resp
                        .json()
                        .await
                        .map_err(|e| ToolError::Asset(format!("GIPHY parse error: {}", e)))?;

                    let results: Vec<serde_json::Value> = body
                        .get("data")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .take(limit)
                                .map(|g| {
                                    let images = g.get("images").cloned().unwrap_or(json!({}));
                                    let original =
                                        images.get("original").cloned().unwrap_or(json!({}));
                                    let downsized =
                                        images.get("downsized").cloned().unwrap_or(json!({}));
                                    json!({
                                        "id": g.get("id"),
                                        "title": g.get("title"),
                                        "url": g.get("url"),
                                        "gif_url": original.get("url"),
                                        "webp_url": original.get("webp"),
                                        "preview_url": downsized.get("url"),
                                        "width": original.get("width"),
                                        "height": original.get("height"),
                                        "size_bytes": original.get("size"),
                                        "source": "giphy",
                                        "transparent": true,
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();

                    report_progress(100.0, 100.0, &format!("Found {} stickers", results.len()))
                        .await
                        .ok();
                    return Ok(json!({
                        "status": "searched",
                        "query": query,
                        "source": "giphy",
                        "count": results.len(),
                        "results": results,
                    }));
                }
                _ => tracing::warn!("[gif.search] GIPHY API failed"),
            }
        }
    }

    // Fallback: Pexels video search for short clips
    report_progress(
        50.0,
        100.0,
        "GIPHY key not set, searching Pexels for short clips...",
    )
    .await
    .ok();
    let pexels_key_val = pexels_key();

    let url = format!(
        "https://api.pexels.com/videos/search?query={}&per_page={}&orientation=square",
        urlencoding::encode(query),
        limit
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| ToolError::Asset(format!("HTTP client error: {}", e)))?;

    match client
        .get(&url)
        .header("Authorization", &pexels_key_val)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| ToolError::Asset(format!("Pexels parse error: {}", e)))?;

            let results: Vec<serde_json::Value> = body.get("videos")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().take(limit).filter_map(|v| {
                    let duration = v.get("duration").and_then(|v| v.as_i64()).unwrap_or(0);
                    if duration > 10 { return None; } // Only short clips
                    let video_files = v.get("video_files").and_then(|v| v.as_array())?;
                    let best = video_files.iter()
                        .find(|f| {
                            let w = f.get("width").and_then(|v| v.as_u64()).unwrap_or(0);
                            (360..=720).contains(&w)
                        })?;
                    Some(json!({
                        "id": v.get("id"),
                        "title": format!("Pexels video {}", v.get("id").and_then(|v| v.as_u64()).unwrap_or(0)),
                        "url": v.get("url"),
                        "video_url": best.get("link"),
                        "width": best.get("width"),
                        "height": best.get("height"),
                        "duration_s": duration,
                        "source": "pexels",
                        "transparent": false,
                    }))
                }).collect())
                .unwrap_or_default();

            report_progress(100.0, 100.0, &format!("Found {} clips", results.len()))
                .await
                .ok();
            return Ok(json!({
                "status": "searched",
                "query": query,
                "source": "pexels",
                "count": results.len(),
                "results": results,
                "message": "GIPHY_API_KEY not set. Set it to search GIPHY stickers. Showing Pexels short clips instead.",
            }));
        }
        _ => {}
    }

    Ok(json!({
        "status": "no_results",
        "query": query,
        "count": 0,
        "results": [],
        "message": "Set GIPHY_API_KEY env var for GIPHY sticker search. Get free key at https://developers.giphy.com",
    }))
}

pub(crate) async fn handle_library_search(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let query = extract_str(&args, "query")?;
    let media_type = default_opt_str(&args, "type");
    let limit = default_u32(&args, "limit", 10) as usize;
    // New filters (audit bug #18): mood/energy/duration/source/license.
    // These make library.search as filterable as music.search + sfx.search,
    // so an agent can find a 30s "epic cinematic" track without paging
    // through hundreds of irrelevant results.
    let source_filter = default_opt_str(&args, "source");
    let license_filter = default_opt_str(&args, "license");
    let min_duration_s = args
        .get("min_duration_s")
        .and_then(|v| v.as_f64());
    let max_duration_s = args
        .get("max_duration_s")
        .and_then(|v| v.as_f64());
    let tag_filter: Option<String> = default_opt_str(&args, "tag");
    let mood_filter: Option<String> = default_opt_str(&args, "mood");
    let energy_filter: Option<String> = default_opt_str(&args, "energy");

    // Resolve path CWD-independently (round-2 GAP #12 fix — same as
    // background.search). library.search only worked from repo root before.
    let index_path_raw = std::env::var("OPENSCRIPT_MUSIC_LIBRARY_INDEX")
        .unwrap_or_else(|_| "mcp/assets/music_library_index.json".to_string());
    let index_path = resolve_repo_path(&index_path_raw);

    if !index_path.exists() {
        return Err(ToolError::NotFound(format!(
            "Music library index not found at {} (resolved from {}). Run the library.build MCP tool to generate it (requires yt-dlp on PATH).",
            index_path.display(),
            index_path_raw
        )));
    }

    let index_str = std::fs::read_to_string(&index_path)?;
    let index: serde_json::Value = serde_json::from_str(&index_str)?;

    let entries = index
        .get("entries")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let query_lower = query.to_lowercase();
    let query_words: Vec<&str> = query_lower.split_whitespace().collect();

    let mut results: Vec<serde_json::Value> = Vec::new();
    let mut filtered_by_duration = 0u32;
    let mut filtered_by_source = 0u32;
    let mut filtered_by_license = 0u32;
    let mut filtered_by_tag = 0u32;
    let mut filtered_by_mood = 0u32;
    let mut filtered_by_energy = 0u32;

    for entry in &entries {
        // Filter by media type if specified
        if let Some(ref mt) = media_type {
            let entry_type = entry
                .get("media_type")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if entry_type != mt {
                continue;
            }
        }

        // Filter by source channel (e.g. "NoCopyrightSounds")
        if let Some(ref src) = source_filter {
            let entry_source = entry
                .get("source")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if entry_source != src {
                filtered_by_source += 1;
                continue;
            }
        }

        // Filter by license (e.g. "no-copyright", "creative-commons")
        if let Some(ref lic) = license_filter {
            let entry_license = entry
                .get("license")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if entry_license != lic {
                filtered_by_license += 1;
                continue;
            }
        }

        // Filter by duration range (in seconds)
        let duration = entry
            .get("duration_s")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        if let Some(min_d) = min_duration_s {
            if duration < min_d {
                filtered_by_duration += 1;
                continue;
            }
        }
        if let Some(max_d) = max_duration_s {
            if duration > max_d {
                filtered_by_duration += 1;
                continue;
            }
        }

        // Filter by tag (substring match against the entry's tags array)
        if let Some(ref tag_q) = tag_filter {
            let tag_lower = tag_q.to_lowercase();
            let tags: Vec<String> = entry
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|t| t.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            let matches_tag = tags
                .iter()
                .any(|t| t.to_lowercase().contains(&tag_lower));
            if !matches_tag {
                filtered_by_tag += 1;
                continue;
            }
        }

        // Filter by mood (exact match against enriched mood field)
        if let Some(ref mood_q) = mood_filter {
            let entry_mood = entry
                .get("mood")
                .and_then(|v| v.as_str())
                .unwrap_or("neutral");
            if entry_mood != mood_q {
                filtered_by_mood += 1;
                continue;
            }
        }

        // Filter by energy (exact match against enriched energy field)
        if let Some(ref energy_q) = energy_filter {
            let entry_energy = entry
                .get("energy")
                .and_then(|v| v.as_str())
                .unwrap_or("medium");
            if entry_energy != energy_q {
                filtered_by_energy += 1;
                continue;
            }
        }

        let title = entry
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase();
        let tags: Vec<String> = entry
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| t.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let mut score = 0i32;

        // Exact title match
        if query_lower.contains(&title) || title.contains(&query_lower) {
            score += 10;
        }

        // Word matches
        for word in &query_words {
            if title.contains(word) {
                score += 3;
            }
            if tags.iter().any(|t| t == word) {
                score += 5;
            }
        }

        // Phase 3: mood/energy/genre scoring weights (audit bug #19).
        // Without these, a text-match "calm" on a title returns energetic
        // tracks that happen to say "calm" in their description.
        let entry_mood = entry
            .get("mood")
            .and_then(|v| v.as_str())
            .unwrap_or("neutral");
        let entry_energy = entry
            .get("energy")
            .and_then(|v| v.as_str())
            .unwrap_or("medium");
        let entry_genre = entry
            .get("genre")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Mood match: strong signal when agent filters by mood.
        if mood_filter.is_some() && entry_mood == mood_filter.as_deref().unwrap_or("") {
            score += 8;
        }

        // Energy match: moderate signal when agent filters by energy.
        if energy_filter.is_some() && entry_energy == energy_filter.as_deref().unwrap_or("") {
            score += 4;
        }

        // Genre match: if query words appear in genre field, boost.
        if !entry_genre.is_empty() {
            let genre_lower = entry_genre.to_lowercase();
            for word in &query_words {
                if genre_lower.contains(word) {
                    score += 3;
                }
            }
        }

        // Penalize mood mismatch when mood filter is active.
        if mood_filter.is_some() && entry_mood != mood_filter.as_deref().unwrap_or("") {
            score -= 5;
        }

        if score > 0 {
            let mut result = entry.clone();
            if let Some(obj) = result.as_object_mut() {
                obj.insert("relevance_score".into(), json!(score));
            }
            results.push(result);
        }
    }

    // Sort by relevance
    results.sort_by(|a, b| {
        let sa = a
            .get("relevance_score")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let sb = b
            .get("relevance_score")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        sb.cmp(&sa)
    });

    let total = results.len();
    results.truncate(limit);

    // Surface filter stats so an agent can tell why results are sparse.
    let mut filter_stats = serde_json::Map::new();
    if filtered_by_duration > 0 {
        filter_stats.insert("filtered_by_duration".into(), json!(filtered_by_duration));
    }
    if filtered_by_source > 0 {
        filter_stats.insert("filtered_by_source".into(), json!(filtered_by_source));
    }
    if filtered_by_license > 0 {
        filter_stats.insert("filtered_by_license".into(), json!(filtered_by_license));
    }
    if filtered_by_tag > 0 {
        filter_stats.insert("filtered_by_tag".into(), json!(filtered_by_tag));
    }
    if filtered_by_mood > 0 {
        filter_stats.insert("filtered_by_mood".into(), json!(filtered_by_mood));
    }
    if filtered_by_energy > 0 {
        filter_stats.insert("filtered_by_energy".into(), json!(filtered_by_energy));
    }

    Ok(json!({
        "status": "searched",
        "query": query,
        "type": media_type,
        "total_matches": total,
        "count": results.len(),
        "results": results,
        "filters_applied": {
            "source": source_filter,
            "license": license_filter,
            "min_duration_s": min_duration_s,
            "max_duration_s": max_duration_s,
            "tag": tag_filter,
            "mood": mood_filter,
            "energy": energy_filter,
        },
        "filter_stats": filter_stats,
        "index_stats": {
            "total_entries": index.get("total_entries"),
            "music_count": index.get("music_count"),
            "sfx_count": index.get("sfx_count"),
            "sources": index.get("sources"),
        },
    }))
}

pub(crate) async fn handle_library_download(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let filename = extract_str(&args, "filename")?;
    let output_dir = default_str(&args, "output_dir", "mcp/assets/music_cache");
    let output_dir_owned = output_dir.to_string();

    std::fs::create_dir_all(&output_dir_owned)?;

    let index_path = std::env::var("OPENSCRIPT_MUSIC_LIBRARY_INDEX")
        .unwrap_or_else(|_| "mcp/assets/music_library_index.json".to_string());

    let index_str = std::fs::read_to_string(&index_path)?;
    let index: serde_json::Value = serde_json::from_str(&index_str)?;

    let entries = index
        .get("entries")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let entry = entries
        .iter()
        .find(|e| e.get("filename").and_then(|v| v.as_str()).unwrap_or("") == filename)
        .ok_or_else(|| ToolError::NotFound(format!("Entry not found in library: {}", filename)))?;

    let source_type = entry
        .get("source_type")
        .and_then(|v| v.as_str())
        .unwrap_or("local")
        .to_string();
    let download_url = entry
        .get("download_url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let output_path = format!("{}/{}", output_dir_owned, filename);

    // Check if already downloaded
    if Path::new(&output_path).exists() {
        return Ok(json!({
            "status": "cached",
            "path": output_path,
            "filename": filename,
            "source": entry.get("source"),
        }));
    }

    if source_type == "local" {
        // Local file — just return the path
        return Ok(json!({
            "status": "local",
            "path": download_url,
            "filename": filename,
            "source": entry.get("source"),
        }));
    }

    // Download with yt-dlp (include bot-detection evasion)
    report_progress(0.0, 100.0, &format!("Downloading: {}", filename))
        .await
        .ok();

    let result = tokio::process::Command::new("yt-dlp")
        .arg("-x").arg("--audio-format").arg("mp3")
        .arg("--audio-quality").arg("0")
        .arg("-o").arg(&output_path)
        .arg("--no-playlist")
        .arg("--quiet")
        // ponytail: skip --cookies-from-browser — NCS/AudioLibrary tracks are
        // public. Chrome cookies fail on headless/server environments. Only add
        // cookies for age-gated / private content.
        .arg("--user-agent").arg("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .arg(&download_url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await;

    match result {
        Ok(output) if output.status.success() => {
            report_progress(100.0, 100.0, "Downloaded").await.ok();
            let file_size = std::fs::metadata(&output_path)
                .map(|m| m.len())
                .unwrap_or(0);
            Ok(json!({
                "status": "downloaded",
                "path": output_path,
                "filename": filename,
                "file_size_bytes": file_size,
                "source": entry.get("source"),
                "title": entry.get("title"),
            }))
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(ToolError::Asset(format!(
                "Download failed: {}",
                stderr.lines().last().unwrap_or("unknown")
            )))
        }
        Err(e) => Err(ToolError::Asset(format!("yt-dlp not available: {}", e))),
    }
}

pub(crate) async fn handle_library_build(_args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    report_progress(0.0, 100.0, "Building music/SFX library index...")
        .await
        .ok();

    // C2 fix: prior versions shelled out to `python3 mcp/scripts/music_library_indexer.py --build`,
    // which required Python + yt-dlp at runtime. Now uses the native Rust port
    // in `library_indexer.rs`, which shells out to yt-dlp directly and builds
    // the JSON index with serde_json. No Python dependency.
    let index_path = "mcp/assets/music_library_index.json";
    let index = crate::library_indexer::build_index(index_path)
        .await
        .map_err(|e| ToolError::Asset(format!("Index build failed: {}", e)))?;

    report_progress(100.0, 100.0, "Library index built")
        .await
        .ok();

    Ok(json!({
        "status": "built",
        "index_path": index_path,
        "total_entries": index.get("total_entries"),
        "music_count": index.get("music_count"),
        "sfx_count": index.get("sfx_count"),
        "sources": index.get("sources"),
    }))
}

/// Download an image from a URL to a local file. Caches in
/// mcp/assets/image_cache/ to avoid re-downloading.
pub(crate) async fn handle_media_download(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let url = extract_str(&args, "url")?;
    let output_path = default_opt_str(&args, "output_path");

    // Determine cache dir + output path
    let cache_dir = "mcp/assets/image_cache";
    std::fs::create_dir_all(cache_dir).ok();

    let resolved_path = if let Some(p) = output_path {
        if !p.is_empty() {
            p.to_string()
        } else {
            format!("{}/img_{}.{}", cache_dir, md5_hash(url.as_bytes()), url_extension(url))
        }
    } else {
        format!("{}/img_{}.{}", cache_dir, md5_hash(url.as_bytes()), url_extension(url))
    };

    // Check cache
    if std::path::Path::new(&resolved_path).exists() {
        return Ok(json!({
            "status": "cached",
            "path": resolved_path,
            "url": url,
        }));
    }

    // Download
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| ToolError::Asset(format!("HTTP client error: {}", e)))?;

    let resp = client.get(url).send().await
        .map_err(|e| ToolError::Asset(format!("Failed to download image: {}", e)))?;

    if !resp.status().is_success() {
        return Err(ToolError::Asset(format!(
            "Image download failed: HTTP {} for {}",
            resp.status(),
            url
        )));
    }

    let bytes = resp.bytes().await
        .map_err(|e| ToolError::Asset(format!("Failed to read image bytes: {}", e)))?;

    std::fs::write(&resolved_path, &bytes)
        .map_err(|e| ToolError::Asset(format!("Failed to write image to {}: {}", resolved_path, e)))?;

    Ok(json!({
        "status": "downloaded",
        "path": resolved_path,
        "url": url,
        "size_bytes": bytes.len(),
    }))
}

/// Download a GIF from a URL to a local file. Caches in mcp/assets/stickers/
/// so it can be used directly by script.to_video's sticker pipeline.
pub(crate) async fn handle_gif_download(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let url = extract_str(&args, "url")?;
    let output_path = default_opt_str(&args, "output_path");

    let cache_dir = "mcp/assets/stickers";
    std::fs::create_dir_all(cache_dir).ok();

    let resolved_path = if let Some(p) = output_path {
        if !p.is_empty() {
            p.to_string()
        } else {
            format!("{}/gif_{}.gif", cache_dir, md5_hash(url.as_bytes()))
        }
    } else {
        format!("{}/gif_{}.gif", cache_dir, md5_hash(url.as_bytes()))
    };

    // Check cache
    if std::path::Path::new(&resolved_path).exists() {
        return Ok(json!({
            "status": "cached",
            "path": resolved_path,
            "url": url,
        }));
    }

    // Download
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| ToolError::Asset(format!("HTTP client error: {}", e)))?;

    let resp = client.get(url).send().await
        .map_err(|e| ToolError::Asset(format!("Failed to download GIF: {}", e)))?;

    if !resp.status().is_success() {
        return Err(ToolError::Asset(format!(
            "GIF download failed: HTTP {} for {}",
            resp.status(),
            url
        )));
    }

    let bytes = resp.bytes().await
        .map_err(|e| ToolError::Asset(format!("Failed to read GIF bytes: {}", e)))?;

    std::fs::write(&resolved_path, &bytes)
        .map_err(|e| ToolError::Asset(format!("Failed to write GIF to {}: {}", resolved_path, e)))?;

    Ok(json!({
        "status": "downloaded",
        "path": resolved_path,
        "url": url,
        "size_bytes": bytes.len(),
    }))
}


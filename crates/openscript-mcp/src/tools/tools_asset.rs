// ---------------------------------------------------------------------------
// tools_asset — Asset-development pipeline handlers (asset.*)
// Split out of tools.rs (pure-move refactor). `use super::*` grants access
// to tools.rs's helpers, imports, and re-exported sibling handlers.
// ---------------------------------------------------------------------------
use super::*;

pub(crate) async fn handle_asset_library_status(_args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let lib = crate::asset_library::AssetLibrary::load()?;
    let mut by_source: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let mut by_status: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for a in &lib.assets {
        *by_source.entry(a.source.clone()).or_insert(0) += 1;
        *by_status.entry(a.curation_status.clone()).or_insert(0) += 1;
    }
    Ok(json!({
        "status": "success",
        "version": lib.version,
        "root": crate::asset_library::LIBRARY_ROOT,
        "total_assets": lib.assets.len(),
        "by_source": by_source,
        "by_status": by_status,
    }))
}

pub(crate) async fn handle_asset_ingest(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let dir = default_str(&args, "dir", crate::asset_library::LIBRARY_ROOT);
    let mut lib = crate::asset_library::AssetLibrary::load()?;
    let report = lib.ingest_dir(&dir).await?;
    lib.save()?;
    Ok(json!({
        "status": "success",
        "dir": dir,
        "indexed": report.indexed,
        "skipped_duplicates": report.skipped_duplicates,
        "errors": report.errors,
        "total_assets": lib.assets.len(),
    }))
}

pub(crate) async fn handle_asset_probe(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let query = extract_str(&args, "query")?;
    let aspect = default_str(&args, "aspect", "9:16");
    let min_duration_s = default_f64(&args, "min_duration_s", 0.0);
    let max_duration_s = default_f64(&args, "max_duration_s", 0.0);
    let per_provider = default_f64(&args, "per_provider", 8.0) as usize;
    let signal: Vec<String> = args
        .get("signal")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_else(|| crate::stock_signal::signal_tokens_from_scene(query, &[]));
    let q = crate::stock_pool::StockPoolQuery {
        query: query.to_string(),
        aspect: aspect.to_string(),
        min_duration_s,
        max_duration_s,
        per_provider,
        signal,
    };
    let outcome = crate::stock_pool::search_stock_pool(&q).await;
    let candidates: Vec<serde_json::Value> = outcome
        .candidates
        .iter()
        .map(|c| {
            json!({
                "provider": c.provider,
                "id": c.id,
                "title": c.title,
                "duration_s": c.duration_s,
                "width": c.width,
                "height": c.height,
                "thumbnail_url": c.thumbnail_url,
                "page_url": c.page_url,
                "direct_url": c.direct_url,
                "lexical": c.lexical,
            })
        })
        .collect();
    Ok(json!({
        "status": "success",
        "query": query,
        "per_provider": outcome.per_provider,
        "count": candidates.len(),
        "candidates": candidates,
    }))
}

pub(crate) async fn handle_asset_rate(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let id = extract_str(&args, "id")?;
    let quality_rating = default_f64(&args, "quality_rating", 0.0);
    let mood = default_str(&args, "mood", "");
    let energy = default_str(&args, "energy", "");
    let motion_intensity = default_str(&args, "motion_intensity", "");
    let status = default_str(&args, "status", crate::asset_library::STATUS_CANDIDATE);
    let tags: Vec<String> = args
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let relevance: std::collections::HashMap<String, f64> = args
        .get("relevance")
        .and_then(|v| v.as_object())
        .map(|o| o.iter().filter_map(|(k, v)| v.as_f64().map(|f| (k.clone(), f))).collect())
        .unwrap_or_default();
    let mut lib = crate::asset_library::AssetLibrary::load()?;
    let updated = lib.rate(
        id,
        relevance,
        quality_rating,
        &mood,
        &energy,
        &motion_intensity,
        tags,
        &status,
    );
    match updated {
        Some(a) => {
            let summary = (a.id.clone(), a.curation_status.clone(), a.quality_rating);
            lib.save()?;
            Ok(json!({
                "status": "success",
                "asset_id": summary.0,
                "curation_status": summary.1,
                "quality_rating": summary.2,
            }))
        }
        None => Err(ToolError::NotFound(format!("asset id not found: {id}"))),
    }
}

pub(crate) async fn handle_asset_import(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let source_url = default_str(&args, "url", "");
    let local_path = default_str(&args, "path", "");
    let title = default_str(&args, "title", "");
    let source = default_str(&args, "source", "user_upload");
    let provider_id = args
        .get("provider_id")
        .and_then(|v| v.as_str())
        .map(String::from);
    let keywords: Vec<String> = args
        .get("keywords")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();

    std::fs::create_dir_all(crate::asset_library::LIBRARY_ROOT)?;
    let dest_stem = format!(
        "{}/import_{}",
        crate::asset_library::LIBRARY_ROOT,
        chrono::Utc::now().timestamp_millis()
    );

    let imported_path: String = if !source_url.is_empty() {
        if source_url.contains("youtube") || source_url.contains("youtu.be") {
            // YouTube: yt-dlp best ≤720p merged mp4.
            let out_tpl = format!("{dest_stem}.%(ext)s");
            let result = tokio::process::Command::new("yt-dlp")
                .args([
                    "--format",
                    "best[height<=720][ext=mp4]/best[height<=720]/best",
                    "--merge-output-format",
                    "mp4",
                    "--output",
                    &out_tpl,
                    "--no-playlist",
                    "--quiet",
                    "--no-warnings",
                    "--socket-timeout",
                    "25",
                    &source_url,
                ])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true)
                .output()
                .await
                .map_err(|e| ToolError::Asset(format!("yt-dlp spawn error: {e}")))?;
            if !result.status.success() {
                let err = String::from_utf8_lossy(&result.stderr);
                return Err(ToolError::Asset(format!(
                    "yt-dlp import failed: {}",
                    err.chars().take(300).collect::<String>()
                )));
            }
            let mut found: Option<String> = None;
            for entry in std::fs::read_dir(crate::asset_library::LIBRARY_ROOT)? {
                let e = entry?;
                if e.path().to_string_lossy().starts_with(&format!("{dest_stem}.")) {
                    found = Some(e.path().to_string_lossy().to_string());
                }
            }
            found.ok_or_else(|| ToolError::Asset("yt-dlp import produced no file".to_string()))?
        } else {
            // Direct file URL (Pexels/Pixabay).
            let resp = reqwest::Client::new()
                .get(&source_url)
                .send()
                .await
                .map_err(|e| ToolError::Asset(format!("download error: {e}")))?;
            if !resp.status().is_success() {
                return Err(ToolError::Asset(format!(
                    "download failed: HTTP {}",
                    resp.status()
                )));
            }
            let bytes = resp
                .bytes()
                .await
                .map_err(|e| ToolError::Asset(format!("download error: {e}")))?;
            let path = format!("{dest_stem}.mp4");
            std::fs::write(&path, &bytes)?;
            path
        }
    } else if !local_path.is_empty() {
        let path = format!("{dest_stem}.mp4");
        std::fs::copy(&local_path, &path)?;
        path
    } else {
        return Err(ToolError::InvalidArg(
            "provide url or path".to_string(),
        ));
    };

    let mut lib = crate::asset_library::AssetLibrary::load()?;
    let id = lib
        .add_external(&imported_path, &source, provider_id, &title, keywords)
        .await?;
    lib.save()?;
    Ok(json!({
        "status": "success",
        "asset_id": id,
        "path": imported_path,
        "total_assets": lib.assets.len(),
    }))
}

pub(crate) async fn handle_asset_search(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let keywords = extract_str(&args, "keywords")?;
    let quality_floor = default_f64(
        &args,
        "quality_floor",
        crate::scene_media::LIBRARY_QUALITY_FLOOR,
    );
    let lib = crate::asset_library::AssetLibrary::load()?;
    let signal = crate::stock_signal::signal_tokens_from_scene(keywords, &[]);
    let hits = lib.search(&signal, quality_floor);
    let assets: Vec<serde_json::Value> = hits
        .iter()
        .map(|a| {
            json!({
                "id": a.id,
                "path": a.path,
                "title": a.title,
                "keywords": a.keywords,
                "mood": a.mood,
                "energy": a.energy,
                "quality_rating": a.quality_rating,
                "duration_s": a.duration_s,
                "aspect": a.aspect,
                "relevance": a.relevance,
                "usage_count": a.usage_count,
            })
        })
        .collect();
    Ok(json!({
        "status": "success",
        "count": assets.len(),
        "assets": assets,
    }))
}


use openscript_core::timeline::Timeline;
use openscript_core::types::TrackType;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

pub struct App {
    pub timeline_path: String,
    pub timeline: Arc<RwLock<Timeline>>,
    pub selected_segment: usize,
    pub selected_track: TrackType,
    pub scroll_offset: usize,
    pub mode: AppMode,
    pub status_message: String,
    pub status_type: StatusType,
    pub show_tracks: bool,
    pub last_updated: String,
    pub view_focus: ViewFocus,

    // Text input state
    pub caption_input: String,
    pub caption_cursor: usize,

    // Add segment workflow state
    pub adding_segment_phase: u8,
    pub add_start_input: String,
    pub add_end_input: String,
    pub add_caption_input: String,
    pub add_cursor: usize,

    // Render state
    pub is_rendering: bool,
    pub render_progress: f64,
    pub render_output: Option<String>,
    pub render_error: Option<String>,

    // File watcher state
    pub file_watcher_rx: Option<tokio::sync::mpsc::Receiver<FileWatchEvent>>,
    pub timeline_revision: u64,

    // Delete confirmation
    pub pending_delete: bool,

    // Track event detail view
    pub show_track_details: bool,

    // Timeline preview cache
    pub preview_cache: Option<TimelinePreview>,
    pub preview_cache_age: Option<Instant>,
}

#[derive(Clone, Copy, PartialEq)]
pub enum AppMode {
    Normal,
    EditCaption,
    AddingSegment,
}

#[derive(Clone, Copy, PartialEq)]
pub enum StatusType {
    Info,
    Success,
    Error,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ViewFocus {
    Segments,
    Tracks,
}

#[derive(Clone, Debug)]
pub enum FileWatchEvent {
    Modified,
    Deleted,
}

#[derive(Clone, Debug)]
pub struct TimelinePreview {
    pub total_duration_ms: i64,
    pub segment_count: usize,
    pub event_counts: HashMap<String, usize>,
    pub validation_errors: Vec<String>,
    pub render_ready: bool,
}

impl App {
    pub fn new(timeline: Arc<RwLock<Timeline>>, path: String) -> Self {
        Self {
            timeline_path: path,
            timeline,
            selected_segment: 0,
            selected_track: TrackType::Dialogue,
            scroll_offset: 0,
            mode: AppMode::Normal,
            status_message: String::new(),
            status_type: StatusType::Info,
            show_tracks: false,
            last_updated: String::new(),
            view_focus: ViewFocus::Segments,
            caption_input: String::new(),
            caption_cursor: 0,
            adding_segment_phase: 0,
            add_start_input: String::new(),
            add_end_input: String::new(),
            add_caption_input: String::new(),
            add_cursor: 0,
            is_rendering: false,
            render_progress: 0.0,
            render_output: None,
            render_error: None,
            file_watcher_rx: None,
            timeline_revision: 0,
            pending_delete: false,
            show_track_details: false,
            preview_cache: None,
            preview_cache_age: None,
        }
    }

    pub fn set_status(&mut self, msg: &str, status_type: StatusType) {
        self.status_message = msg.to_string();
        self.status_type = status_type;
    }

    pub fn navigate_up(&mut self) {
        if self.selected_segment > 0 {
            self.selected_segment -= 1;
        }
        if self.scroll_offset > self.selected_segment {
            self.scroll_offset = self.selected_segment;
        }
    }

    pub fn navigate_down(&mut self) {
        let guard = self.timeline.try_read();
        if let Ok(tl) = guard {
            let max = tl.segments.len().saturating_sub(1);
            if self.selected_segment < max {
                self.selected_segment += 1;
            }
        }
    }

    pub fn cycle_track(&mut self) {
        let tracks = [
            TrackType::Dialogue,
            TrackType::Voiceover,
            TrackType::Captions,
            TrackType::Broll,
            TrackType::Music,
            TrackType::Sfx,
        ];
        let current_idx = tracks
            .iter()
            .position(|t| t == &self.selected_track)
            .unwrap_or(0);
        self.selected_track = tracks[(current_idx + 1) % tracks.len()].clone();
    }

    pub fn toggle_view_focus(&mut self) {
        self.view_focus = match self.view_focus {
            ViewFocus::Segments => ViewFocus::Tracks,
            ViewFocus::Tracks => ViewFocus::Segments,
        };
    }

    // ── Text input: caption editing ──

    pub fn start_caption_edit(&mut self) {
        let guard = self.timeline.try_read();
        let caption = if let Ok(tl) = guard {
            tl.segments
                .get(self.selected_segment)
                .map(|s| s.caption.clone())
                .unwrap_or_default()
        } else {
            String::new()
        };
        self.caption_input = caption;
        self.caption_cursor = self.caption_input.len();
        self.mode = AppMode::EditCaption;
    }

    pub fn caption_input_char(&mut self, c: char) {
        let idx = self.caption_cursor.min(self.caption_input.len());
        let mut chars: Vec<char> = self.caption_input.chars().collect();
        chars.insert(idx, c);
        self.caption_input = chars.into_iter().collect();
        self.caption_cursor += 1;
    }

    pub fn caption_input_backspace(&mut self) {
        if self.caption_cursor == 0 || self.caption_input.is_empty() {
            return;
        }
        let mut chars: Vec<char> = self.caption_input.chars().collect();
        let idx = self.caption_cursor.min(chars.len());
        if idx > 0 {
            chars.remove(idx - 1);
            self.caption_cursor -= 1;
        }
        self.caption_input = chars.into_iter().collect();
    }

    pub fn caption_input_delete(&mut self) {
        let mut chars: Vec<char> = self.caption_input.chars().collect();
        let idx = self.caption_cursor.min(chars.len());
        if idx < chars.len() {
            chars.remove(idx);
        }
        self.caption_input = chars.into_iter().collect();
    }

    pub fn caption_input_left(&mut self) {
        if self.caption_cursor > 0 {
            self.caption_cursor -= 1;
        }
    }

    pub fn caption_input_right(&mut self) {
        let len = self.caption_input.chars().count();
        if self.caption_cursor < len {
            self.caption_cursor += 1;
        }
    }

    pub fn commit_caption(&mut self) {
        let new_caption = self.caption_input.clone();
        let idx = self.selected_segment;
        let path = self.timeline_path.clone();
        let save_result: Result<(), String> = match self.timeline.try_write() {
            Ok(mut guard) => {
                if let Some(seg) = guard.segments.get_mut(idx) {
                    seg.caption = new_caption;
                }
                guard.updated_at = chrono::Utc::now();
                guard.save(&path).map_err(|e| format!("Save failed: {e}"))
            }
            Err(_) => Err("Timeline locked".to_string()),
        };
        match save_result {
            Ok(()) => self.set_status("Caption updated", StatusType::Success),
            Err(e) => self.set_status(&e, StatusType::Error),
        }
        self.invalidate_preview();
        self.mode = AppMode::Normal;
        self.caption_input.clear();
        self.caption_cursor = 0;
    }

    pub fn cancel_caption(&mut self) {
        self.mode = AppMode::Normal;
        self.caption_input.clear();
        self.caption_cursor = 0;
    }

    // ── Text input: adding segment (multi-phase) ──

    pub fn start_add_segment(&mut self) {
        self.adding_segment_phase = 0;
        self.add_start_input.clear();
        self.add_end_input.clear();
        self.add_caption_input.clear();
        self.add_cursor = 0;
        self.mode = AppMode::AddingSegment;
        self.set_status("Enter start time (seconds):", StatusType::Info);
    }

    pub fn add_input_char(&mut self, c: char) {
        let idx = self.add_cursor;
        let input = match self.adding_segment_phase {
            0 => &mut self.add_start_input,
            1 => &mut self.add_end_input,
            _ => &mut self.add_caption_input,
        };
        let mut chars: Vec<char> = input.chars().collect();
        let pos = idx.min(chars.len());
        chars.insert(pos, c);
        *input = chars.into_iter().collect();
        self.add_cursor += 1;
    }

    pub fn add_input_backspace(&mut self) {
        let input = match self.adding_segment_phase {
            0 => &mut self.add_start_input,
            1 => &mut self.add_end_input,
            _ => &mut self.add_caption_input,
        };
        if self.add_cursor == 0 || input.is_empty() {
            return;
        }
        let mut chars: Vec<char> = input.chars().collect();
        let idx = self.add_cursor.min(chars.len());
        if idx > 0 {
            chars.remove(idx - 1);
            self.add_cursor -= 1;
        }
        *input = chars.into_iter().collect();
    }

    pub fn add_input_submit(&mut self) {
        match self.adding_segment_phase {
            0 => {
                if self.add_start_input.trim().is_empty() {
                    self.set_status("Start time required", StatusType::Error);
                    return;
                }
                self.adding_segment_phase = 1;
                self.add_cursor = self.add_end_input.len();
                self.set_status("Enter end time (seconds):", StatusType::Info);
            }
            1 => {
                if self.add_end_input.trim().is_empty() {
                    self.set_status("End time required", StatusType::Error);
                    return;
                }
                let start: f64 = match self.add_start_input.trim().parse() {
                    Ok(v) => v,
                    Err(_) => {
                        self.set_status("Invalid start time", StatusType::Error);
                        self.adding_segment_phase = 0;
                        return;
                    }
                };
                let end: f64 = match self.add_end_input.trim().parse() {
                    Ok(v) => v,
                    Err(_) => {
                        self.set_status("Invalid end time", StatusType::Error);
                        return;
                    }
                };
                if start >= end {
                    self.set_status("Start must be before end", StatusType::Error);
                    return;
                }
                self.adding_segment_phase = 2;
                self.add_cursor = self.add_caption_input.len();
                self.set_status("Enter caption:", StatusType::Info);
            }
            _ => {
                let start: f64 = self.add_start_input.trim().parse().unwrap_or(0.0);
                let end: f64 = self.add_end_input.trim().parse().unwrap_or(0.0);
                let caption = self.add_caption_input.clone();
                let path = self.timeline_path.clone();
                let save_result: Result<(), String> = match self.timeline.try_write() {
                    Ok(mut guard) => {
                        guard.add_segment(start, end, &caption, 80, None);
                        guard.updated_at = chrono::Utc::now();
                        guard.save(&path).map_err(|e| format!("Save failed: {e}"))
                    }
                    Err(_) => Err("Timeline locked".to_string()),
                };
                match save_result {
                    Ok(()) => self.set_status("Segment added", StatusType::Success),
                    Err(e) => self.set_status(&e, StatusType::Error),
                }
                self.invalidate_preview();
                self.mode = AppMode::Normal;
                self.add_start_input.clear();
                self.add_end_input.clear();
                self.add_caption_input.clear();
                self.add_cursor = 0;
            }
        }
    }

    pub fn cancel_add_segment(&mut self) {
        self.mode = AppMode::Normal;
        self.add_start_input.clear();
        self.add_end_input.clear();
        self.add_caption_input.clear();
        self.add_cursor = 0;
        self.adding_segment_phase = 0;
        self.set_status("Add cancelled", StatusType::Info);
    }

    // ── Segment CRUD ──

    pub fn delete_selected_segment(&mut self) -> Result<(), String> {
        let idx = self.selected_segment;
        let path = self.timeline_path.clone();
        let seg_len = {
            let mut guard = self
                .timeline
                .try_write()
                .map_err(|_| "Timeline locked".to_string())?;
            if idx >= guard.segments.len() {
                return Err("No segment selected".to_string());
            }
            guard.segments.remove(idx);
            for (i, seg) in guard.segments.iter_mut().enumerate() {
                seg.id = format!("seg_{:03}", i + 1);
            }
            guard.updated_at = chrono::Utc::now();
            let new_len = guard.segments.len();
            guard.save(&path).map_err(|e| format!("Save failed: {e}"))?;
            new_len
        };
        if seg_len == 0 {
            self.selected_segment = 0;
        } else if self.selected_segment >= seg_len {
            self.selected_segment = seg_len - 1;
        }
        self.invalidate_preview();
        Ok(())
    }

    // ── Render state management ──

    pub fn start_render(&mut self) {
        self.is_rendering = true;
        self.render_progress = 0.0;
        self.render_output = None;
        self.render_error = None;
        self.set_status("Rendering started...", StatusType::Info);
    }

    pub fn update_render_progress(&mut self, progress: f64) {
        self.render_progress = progress.clamp(0.0, 1.0);
    }

    pub fn complete_render(&mut self, output_path: String) {
        self.is_rendering = false;
        self.render_progress = 1.0;
        self.render_output = Some(output_path.clone());
        self.render_error = None;
        self.set_status(
            &format!("Render complete: {output_path}"),
            StatusType::Success,
        );
    }

    pub fn fail_render(&mut self, error: String) {
        self.is_rendering = false;
        self.render_output = None;
        self.render_error = Some(error.clone());
        self.set_status(&format!("Render failed: {error}"), StatusType::Error);
    }

    // ── Timeline preview ──

    pub fn get_or_compute_preview(&mut self) -> TimelinePreview {
        if let Some(ref cache) = self.preview_cache {
            if let Some(age) = self.preview_cache_age {
                if age.elapsed().as_secs() < 5 {
                    return (*cache).clone();
                }
            }
        }
        self.compute_preview()
    }

    fn compute_preview(&mut self) -> TimelinePreview {
        let result = match self.timeline.try_read() {
            Ok(guard) => {
                let total_duration_ms = guard.total_duration_ms();
                let segment_count = guard.segments.len();
                let mut event_counts = HashMap::new();
                for (track, events) in &guard.tracks {
                    event_counts.insert(format!("{track:?}"), events.len());
                }
                let validation_errors = guard.validate();
                let render_ready = validation_errors.is_empty() && segment_count > 0;
                Ok(TimelinePreview {
                    total_duration_ms,
                    segment_count,
                    event_counts,
                    validation_errors,
                    render_ready,
                })
            }
            Err(_) => Err("Timeline locked"),
        };
        match result {
            Ok(preview) => {
                let clone = preview.clone();
                self.preview_cache = Some(preview);
                self.preview_cache_age = Some(Instant::now());
                clone
            }
            Err(_) => TimelinePreview {
                total_duration_ms: 0,
                segment_count: 0,
                event_counts: HashMap::new(),
                validation_errors: vec!["Timeline locked".to_string()],
                render_ready: false,
            },
        }
    }

    pub fn invalidate_preview(&mut self) {
        self.preview_cache = None;
        self.preview_cache_age = None;
    }

    // ── File watcher integration ──

    pub fn set_file_watcher(&mut self, rx: tokio::sync::mpsc::Receiver<FileWatchEvent>) {
        self.file_watcher_rx = Some(rx);
    }

    pub fn process_file_events(&mut self) {
        let events: Vec<FileWatchEvent> = {
            let rx = match self.file_watcher_rx.as_mut() {
                Some(rx) => rx,
                None => return,
            };
            let mut collected = Vec::new();
            while let Ok(event) = rx.try_recv() {
                collected.push(event);
            }
            collected
        };
        for event in events {
            match event {
                FileWatchEvent::Modified => {
                    self.timeline_revision += 1;
                    self.invalidate_preview();
                    self.set_status("Timeline file modified, reloaded", StatusType::Info);
                }
                FileWatchEvent::Deleted => {
                    self.set_status(
                        "Timeline file deleted — save will create a new one",
                        StatusType::Error,
                    );
                }
            }
        }
    }

    // ── Delete confirmation ──

    pub fn trigger_delete(&mut self) {
        if self.pending_delete {
            match self.delete_selected_segment() {
                Ok(()) => {
                    self.pending_delete = false;
                    self.set_status("Segment deleted", StatusType::Success);
                }
                Err(e) => {
                    self.set_status(&e, StatusType::Error);
                }
            }
        } else {
            self.pending_delete = true;
            self.set_status("Press 'd' again to confirm delete", StatusType::Info);
        }
    }

    pub fn cancel_delete(&mut self) {
        self.pending_delete = false;
        self.set_status("Delete cancelled", StatusType::Info);
    }
}

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Stylize,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Cell, Clear, Gauge, List, ListItem, Paragraph, Row, Table,
        TableState, Wrap,
    },
    Frame,
};

use crate::app::{App, AppMode, StatusType, ViewFocus};
use openscript_core::timeline::EventKind;
use openscript_core::types::TrackType;

const VISIBLE_ROWS: usize = 15;

pub fn render(f: &mut Frame, app: &App) {
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(10),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(f.area());

    render_main(f, app, main_chunks[0]);
    render_status_bar(f, app, main_chunks[1]);
    render_help(f, app, main_chunks[2]);

    if app.is_rendering {
        render_render_overlay(f, app, f.area());
    }

    if app.pending_delete {
        render_delete_dialog(f, app, f.area());
    }
}

fn render_main(f: &mut Frame, app: &App, area: Rect) {
    match app.mode {
        AppMode::AddingSegment => {
            render_add_segment_form(f, app, area);
        }
        _ => {
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
                .split(area);
            render_segment_table(f, app, chunks[0]);
            render_right_panel(f, app, chunks[1]);
        }
    }
}

// ── Segment Table ──

fn render_segment_table(f: &mut Frame, app: &App, area: Rect) {
    let guard = app.timeline.try_read();
    let segment_count = guard.as_ref().map(|tl| tl.segments.len()).unwrap_or(0);
    let border_color = if app.view_focus == ViewFocus::Segments {
        Color::Cyan
    } else {
        Color::DarkGray
    };

    let header_cells = ["ID", "Start", "End", "Role", "Caption"].iter().map(|h| {
        Cell::from(*h).style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
    });
    let header = Row::new(header_cells).style(Style::default().fg(Color::White));

    let rows: Vec<Row> = if let Ok(tl) = &guard {
        let start = app.scroll_offset.min(segment_count.saturating_sub(1));
        let end = (start + VISIBLE_ROWS).min(segment_count);
        tl.segments[start..end]
            .iter()
            .enumerate()
            .map(|(offset, seg)| {
                let i = start + offset;
                let id = seg.id.strip_prefix("seg_").unwrap_or(&seg.id);
                let role = seg
                    .semantic_role
                    .as_ref()
                    .map(|r| r.to_string())
                    .unwrap_or_default();
                let caption = if seg.caption.len() > 40 {
                    format!("{}...", &seg.caption[..37])
                } else {
                    seg.caption.clone()
                };
                let event_badge = if app.show_track_details {
                    let total_events: usize = tl.tracks.values().map(|v| v.len()).sum();
                    format!(" [{}ev]", total_events)
                } else {
                    String::new()
                };
                let cells = vec![
                    Cell::from(id.to_string()),
                    Cell::from(format!("{:.2}s", seg.start)),
                    Cell::from(format!("{:.2}s", seg.end)),
                    Cell::from(role.to_uppercase()),
                    Cell::from(format!("{}{}", caption, event_badge)),
                ];
                let style = if i == app.selected_segment {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else if i % 2 == 0 {
                    Style::default().fg(Color::White)
                } else {
                    Style::default().fg(Color::Gray)
                };
                Row::new(cells).style(style)
            })
            .collect()
    } else {
        vec![Row::new(vec![Cell::from("Loading timeline...")])]
    };

    let title = if app.show_track_details {
        format!(" Segments ({}) [track details on] ", segment_count)
    } else {
        format!(" Segments ({}) ", segment_count)
    };

    let table = Table::new(
        rows,
        [
            Constraint::Length(6),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(12),
            Constraint::Min(20),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(border_color))
            .title(title),
    )
    .row_highlight_style(
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );

    let mut state = TableState::default();
    state.select(Some(app.selected_segment.saturating_sub(app.scroll_offset)));
    f.render_stateful_widget(table, area, &mut state);
}

// ── Right Panel (tracks + preview + render status) ──

fn render_right_panel(f: &mut Frame, app: &App, area: Rect) {
    let preview = compute_preview(app);
    let has_render_info = app.render_output.is_some() || app.render_error.is_some();

    let constraints = if has_render_info {
        vec![
            Constraint::Min(6),
            Constraint::Length(3),
            Constraint::Length(3),
        ]
    } else {
        vec![Constraint::Min(6), Constraint::Length(3)]
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    render_track_list(f, app, chunks[0]);
    render_preview_panel(f, &preview, app, chunks[1]);

    if has_render_info {
        render_render_status(f, app, chunks[2]);
    }
}

fn render_track_list(f: &mut Frame, app: &App, area: Rect) {
    let tracks = [
        TrackType::Dialogue,
        TrackType::Voiceover,
        TrackType::Captions,
        TrackType::Broll,
        TrackType::Music,
        TrackType::Sfx,
    ];

    let guard = app.timeline.try_read();
    let border_color = if app.view_focus == ViewFocus::Tracks {
        Color::Cyan
    } else {
        Color::DarkGray
    };

    if app.show_track_details {
        // Expanded view: show track list + selected track events
        let selected_events: Vec<ListItem> = if let Ok(tl) = &guard {
            tl.tracks
                .get(&app.selected_track)
                .map(|events| {
                    events
                        .iter()
                        .map(|evt| {
                            let kind_info = format_event_kind(&evt.kind);
                            let detail = format!(
                                "{} | {:.0}→{:.0}ms | {:.1}dB{}",
                                evt.id, evt.start_ms, evt.end_ms, evt.gain_db, kind_info
                            );
                            ListItem::new(Line::from(Span::styled(
                                detail,
                                Style::default().fg(Color::Gray),
                            )))
                        })
                        .collect()
                })
                .unwrap_or_default()
        } else {
            vec![]
        };

        let track_names: Vec<ListItem> = tracks
            .iter()
            .map(|track| {
                let event_count = guard
                    .as_ref()
                    .map(|tl| tl.tracks.get(track).map(|e| e.len()).unwrap_or(0))
                    .unwrap_or(0);
                let is_selected = app.selected_track == *track;
                let marker = if is_selected { "▸" } else { " " };
                let style = if is_selected {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Gray)
                };
                ListItem::new(Line::from(vec![
                    Span::styled(marker, Style::default().fg(Color::Yellow)),
                    Span::raw(" "),
                    Span::styled(format!("{:?}", track), style),
                    Span::styled(
                        format!(" ({})", event_count),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]))
            })
            .collect();

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(8), Constraint::Min(4)])
            .split(area);

        let track_list = List::new(track_names).block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(border_color))
                .title(" Tracks "),
        );
        f.render_widget(track_list, chunks[0]);

        let event_list = List::new(selected_events).block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(format!(" {:?} Events ", app.selected_track)),
        );
        f.render_widget(event_list, chunks[1]);
    } else {
        // Compact view
        let items: Vec<ListItem> = tracks
            .iter()
            .map(|track| {
                let event_count = guard
                    .as_ref()
                    .map(|tl| tl.tracks.get(track).map(|e| e.len()).unwrap_or(0))
                    .unwrap_or(0);
                let is_selected = app.selected_track == *track;
                let marker = if is_selected { "▸" } else { " " };
                let style = if is_selected {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Gray)
                };
                ListItem::new(Line::from(vec![
                    Span::styled(marker, Style::default().fg(Color::Yellow)),
                    Span::raw(" "),
                    Span::styled(format!("{:?}", track), style),
                    Span::styled(
                        format!(" ({})", event_count),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]))
            })
            .collect();

        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(border_color))
                .title(" Tracks "),
        );
        f.render_widget(list, area);
    }
}

fn format_event_kind(kind: &EventKind) -> String {
    match kind {
        EventKind::Dialogue => String::new(),
        EventKind::Voiceover {
            voice_profile_id,
            text,
            ..
        } => {
            let preview = if text.len() > 20 {
                format!("{}...", &text[..17])
            } else {
                text.clone()
            };
            format!(" | vo:{} \"{}\"", voice_profile_id, preview)
        }
        EventKind::Caption { text, style, .. } => {
            let preview = if text.len() > 20 {
                format!("{}...", &text[..17])
            } else {
                text.clone()
            };
            format!(" | \"{}\" ({})", preview, style)
        }
        EventKind::Broll {
            concept,
            transition_style,
            ..
        } => format!(" | {} [{}]", concept, transition_style),
        EventKind::Music { mood, energy, .. } => {
            format!(" | {} / {}", mood, energy)
        }
        EventKind::Sfx {
            editorial_role,
            category,
            ..
        } => format!(" | {} ({})", editorial_role, category),
    }
}

fn render_preview_panel(
    f: &mut Frame,
    preview: &crate::app::TimelinePreview,
    app: &App,
    area: Rect,
) {
    let issue_count = preview.validation_errors.len();
    let render_label = if preview.render_ready {
        Span::styled("✅ Ready", Style::default().fg(Color::Green))
    } else if issue_count > 0 {
        Span::styled(
            format!("⚠ {} issue(s)", issue_count),
            Style::default().fg(Color::Yellow),
        )
    } else {
        Span::styled("⏳ Loading", Style::default().fg(Color::Gray))
    };

    let lines = vec![
        Line::from(vec![
            Span::styled("Duration: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format_duration(preview.total_duration_ms),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("Segments: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                preview.segment_count.to_string(),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(vec![
            Span::styled("Render: ", Style::default().fg(Color::DarkGray)),
            render_label,
        ]),
        Line::from(vec![
            Span::styled("Rev: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                app.timeline_revision.to_string(),
                Style::default().fg(Color::Gray),
            ),
        ]),
    ];

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(" Preview "),
        )
        .wrap(Wrap { trim: true });

    f.render_widget(paragraph, area);
}

fn format_duration(ms: i64) -> String {
    let total_secs = ms / 1000;
    let mins = total_secs / 60;
    let secs = total_secs % 60;
    format!("{:02}:{:02}", mins, secs)
}

fn compute_preview(app: &App) -> crate::app::TimelinePreview {
    use std::collections::HashMap;
    if let Some(ref cache) = app.preview_cache {
        if let Some(age) = app.preview_cache_age {
            if age.elapsed().as_secs() < 5 {
                return (*cache).clone();
            }
        }
    }
    match app.timeline.try_read() {
        Ok(guard) => {
            let total_duration_ms = guard.total_duration_ms();
            let segment_count = guard.segments.len();
            let mut event_counts = HashMap::new();
            for (track, events) in &guard.tracks {
                event_counts.insert(format!("{:?}", track), events.len());
            }
            let validation_errors = guard.validate();
            let render_ready = validation_errors.is_empty() && segment_count > 0;
            crate::app::TimelinePreview {
                total_duration_ms,
                segment_count,
                event_counts,
                validation_errors,
                render_ready,
            }
        }
        Err(_) => crate::app::TimelinePreview {
            total_duration_ms: 0,
            segment_count: 0,
            event_counts: HashMap::new(),
            validation_errors: vec!["Timeline locked".to_string()],
            render_ready: false,
        },
    }
}

fn render_render_status(f: &mut Frame, app: &App, area: Rect) {
    let lines = if let Some(ref path) = app.render_output {
        vec![Line::from(vec![
            Span::styled("✅ ", Style::default().fg(Color::Green)),
            Span::styled("Last: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                path,
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
        ])]
    } else if let Some(ref err) = app.render_error {
        vec![Line::from(vec![
            Span::styled("❌ ", Style::default().fg(Color::Red)),
            Span::styled("Failed: ", Style::default().fg(Color::DarkGray)),
            Span::styled(err, Style::default().fg(Color::Red)),
        ])]
    } else {
        vec![]
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(" Render ");

    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: true });
    f.render_widget(paragraph, area);
}

// ── Add Segment Form ──

fn render_add_segment_form(f: &mut Frame, app: &App, area: Rect) {
    let phase_labels = [
        ("1", "Start time (seconds)"),
        ("2", "End time (seconds)"),
        ("3", "Caption"),
    ];
    let phase_values = [
        &app.add_start_input,
        &app.add_end_input,
        &app.add_caption_input,
    ];

    let mut lines = Vec::new();

    lines.push(Line::from(
        Span::styled(
            " Add Segment ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .bg(Color::DarkGray),
    ));
    lines.push(Line::from(""));

    for (i, (num, label)) in phase_labels.iter().enumerate() {
        let phase = i as u8;
        let value = phase_values[i];
        let is_current = phase == app.adding_segment_phase;
        let is_done = phase < app.adding_segment_phase;

        let prefix_style = if is_current {
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD)
        } else if is_done {
            Style::default().fg(Color::Gray)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let label_style = if is_current {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else if is_done {
            Style::default().fg(Color::Gray)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let done_marker = if is_done { " ✓" } else { "" };
        lines.push(Line::from(vec![
            Span::styled(format!("[{}] ", num), prefix_style),
            Span::styled(format!("{}:", label), label_style),
            Span::raw(done_marker),
        ]));

        if is_current {
            let input_line = render_input_line(value, app.add_cursor, 50);
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled("▸ ", Style::default().fg(Color::Green)),
            ]));
            for span in input_line.spans {
                lines.push(Line::from(vec![Span::raw("    "), span]));
            }
        } else if is_done {
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(value, Style::default().fg(Color::Gray)),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled("...", Style::default().fg(Color::DarkGray)),
            ]));
        }
        lines.push(Line::from(""));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Green))
        .title(" Add Segment ");

    let paragraph = Paragraph::new(lines).block(block);
    f.render_widget(paragraph, area);
}

fn render_input_line(text: &str, cursor: usize, max_width: usize) -> Line<'static> {
    let chars: Vec<char> = text.chars().collect();
    let display_cursor = cursor.min(chars.len());

    // Truncate for display if too long
    let start = if display_cursor > max_width - 2 {
        display_cursor - (max_width - 2)
    } else {
        0
    };
    let end = (start + max_width - 1).min(chars.len());

    let visible_chars: Vec<char> = chars[start..end].iter().copied().collect();
    let relative_cursor = display_cursor - start;

    let mut spans = vec![Span::styled("[", Style::default().fg(Color::DarkGray))];

    for (i, c) in visible_chars.iter().enumerate() {
        if i == relative_cursor {
            spans.push(Span::styled(
                c.to_string(),
                Style::default()
                    .bg(Color::White)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(
                c.to_string(),
                Style::default().fg(Color::White),
            ));
        }
    }

    if relative_cursor >= visible_chars.len() {
        spans.push(Span::styled(
            "█",
            Style::default()
                .bg(Color::White)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        ));
    }

    spans.push(Span::styled("]", Style::default().fg(Color::DarkGray)));
    Line::from(spans)
}

// ── Status Bar ──

fn render_status_bar(f: &mut Frame, app: &App, area: Rect) {
    let content = if app.mode == AppMode::EditCaption {
        Line::from(vec![
            Span::styled("📝 ", Style::default().fg(Color::Yellow)),
            Span::styled(
                "Editing caption",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " — Enter: save, Esc: cancel",
                Style::default().fg(Color::DarkGray),
            ),
        ])
    } else if app.mode == AppMode::AddingSegment {
        let phase_prompt = match app.adding_segment_phase {
            0 => "Phase 1/3: Enter start time",
            1 => "Phase 2/3: Enter end time",
            _ => "Phase 3/3: Enter caption",
        };
        Line::from(vec![
            Span::styled("➕ ", Style::default().fg(Color::Green)),
            Span::styled(
                phase_prompt,
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " — Enter: next, Esc: cancel",
                Style::default().fg(Color::DarkGray),
            ),
        ])
    } else if app.pending_delete {
        Line::from(vec![
            Span::styled("⚠️ ", Style::default().fg(Color::Red)),
            Span::styled(
                "Press 'd' again to confirm, Esc to cancel",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
        ])
    } else {
        let color = match app.status_type {
            StatusType::Info => Color::Blue,
            StatusType::Success => Color::Green,
            StatusType::Error => Color::Red,
        };
        Line::from(vec![
            Span::styled("● ", Style::default().fg(color)),
            Span::styled(&app.status_message, Style::default().fg(color)),
        ])
    };

    let paragraph = Paragraph::new(content).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::DarkGray)),
    );

    f.render_widget(paragraph, area);
}

// ── Help Bar ──

fn render_help(f: &mut Frame, app: &App, area: Rect) {
    let help_text = match app.mode {
        AppMode::Normal => {
            "j/k: navigate  Tab: switch focus  Enter: edit  a: add  d: delete  t: track details  v: validate  r: render  q: quit"
        }
        AppMode::EditCaption => {
            "←/→: move cursor  Backspace/Delete: edit chars  Enter: save  Esc: cancel"
        }
        AppMode::AddingSegment => {
            "Type: input  Enter: next phase  Esc: cancel"
        }
    };

    let paragraph = Paragraph::new(Line::from(Span::styled(
        help_text,
        Style::default().fg(Color::DarkGray),
    )));

    f.render_widget(paragraph, area);
}

// ── Render Progress Overlay ──

fn render_render_overlay(f: &mut Frame, app: &App, area: Rect) {
    let popup = centered_rect(50, 12, area);

    // Dim background: rendered as the popup's own block below

    let gauge = Gauge::default()
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Cyan))
                .title(" Rendering "),
        )
        .gauge_style(Style::default().fg(Color::Cyan))
        .ratio(app.render_progress.clamp(0.0, 1.0))
        .label(Span::styled(
            format!("{:.0}%", app.render_progress * 100.0),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ));

    f.render_widget(Clear, popup);
    f.render_widget(gauge, popup);
}

// ── Delete Confirmation Dialog ──

fn render_delete_dialog(f: &mut Frame, app: &App, area: Rect) {
    let popup = centered_rect(45, 15, area);

    let seg_id = if let Ok(tl) = app.timeline.try_read() {
        tl.segments
            .get(app.selected_segment)
            .map(|s| s.id.clone())
            .unwrap_or_else(|| "unknown".to_string())
    } else {
        "unknown".to_string()
    };

    let lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("⚠ ", Style::default().fg(Color::Red)),
            Span::styled(
                "Delete segment ",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![Span::styled(
            format!("  {}?", seg_id),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "d",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" = confirm  ", Style::default().fg(Color::Gray)),
            Span::styled(
                "Esc",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" = cancel", Style::default().fg(Color::Gray)),
        ]),
        Line::from(""),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Red))
        .title(" Confirm Delete ");

    let paragraph = Paragraph::new(lines)
        .block(block)
        .alignment(ratatui::layout::Alignment::Center);

    f.render_widget(Clear, popup);
    f.render_widget(paragraph, popup);
}

// ── Utility ──

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

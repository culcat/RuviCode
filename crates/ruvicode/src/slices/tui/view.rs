use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ruvi_core::{AgentMode, MessageRole, SessionSummary, ThemePalette};

use super::theme::rgb;
use super::{ActiveModal, COMMANDS, TuiApp};

const SPINNER_FRAMES: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];

/// Renders the entire `OpenCode`-style `RuviCode` terminal frame.
pub fn render(frame: &mut Frame<'_>, app: &TuiApp) {
    let palette = app.current_palette();
    let root = frame.area();

    frame.render_widget(
        Block::default().style(
            Style::default()
                .bg(rgb(palette.background))
                .fg(rgb(palette.text)),
        ),
        root,
    );

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(root);

    render_header(frame, app, palette, rows[0]);
    render_body(frame, app, palette, rows[1]);
    render_composer(frame, app, palette, rows[2]);
    render_footer(frame, app, palette, rows[3]);
    render_modal(frame, app, palette, root);
}

fn render_header(frame: &mut Frame<'_>, app: &TuiApp, palette: ThemePalette, area: Rect) {
    let mode = app.engine.mode();
    let mode_color = match mode {
        AgentMode::Build => rgb(palette.success),
        AgentMode::Plan => rgb(palette.warning),
    };

    let header_line = Line::from(vec![
        Span::styled(
            " RUVICODE ",
            Style::default()
                .fg(rgb(palette.background))
                .bg(rgb(palette.accent))
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            format!("git:{}", app.git_branch),
            Style::default().fg(rgb(palette.muted)),
        ),
        Span::raw("  "),
        Span::styled(
            format!("[{}]", mode.badge()),
            Style::default()
                .fg(rgb(palette.background))
                .bg(mode_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            format!("{} / {}", app.engine.provider_id(), app.engine.model()),
            Style::default()
                .fg(rgb(palette.text))
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            format!("theme:{}", app.theme_id),
            Style::default().fg(rgb(palette.muted)),
        ),
    ]);

    let widget = Paragraph::new(header_line).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(rgb(palette.border)))
            .style(Style::default().bg(rgb(palette.surface))),
    );
    frame.render_widget(widget, area);
}

fn render_body(frame: &mut Frame<'_>, app: &TuiApp, palette: ThemePalette, area: Rect) {
    if app.show_sidebar && area.width >= 100 {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(60), Constraint::Length(34)])
            .split(area);
        render_main_panel(frame, app, palette, cols[0]);
        render_sidebar(frame, app, palette, cols[1]);
    } else {
        render_main_panel(frame, app, palette, area);
    }
}

fn render_main_panel(frame: &mut Frame<'_>, app: &TuiApp, palette: ThemePalette, area: Rect) {
    if app.engine.messages().is_empty() && !app.in_flight {
        render_hero(frame, app, palette, area);
    } else {
        render_timeline(frame, app, palette, area);
    }
}

fn render_hero(frame: &mut Frame<'_>, app: &TuiApp, palette: ThemePalette, area: Rect) {
    let ctx = app.engine.context();
    let banner = vec![
        Line::from(""),
        Line::from(Span::styled(
            "██████╗ ██╗   ██╗██╗   ██╗██╗ ██████╗ ██████╗ ██████╗ ███████╗",
            Style::default()
                .fg(rgb(palette.accent))
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "██╔══██╗██║   ██║██║   ██║██║██╔════╝██╔═══██╗██╔══██╗██╔════╝",
            Style::default()
                .fg(rgb(palette.accent))
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "██████╔╝██║   ██║██║   ██║██║██║     ██║   ██║██║  ██║█████╗  ",
            Style::default().fg(rgb(palette.text)),
        )),
        Line::from(Span::styled(
            "██╔══██╗██║   ██║╚██╗ ██╔╝██║██║     ██║   ██║██║  ██║██╔══╝  ",
            Style::default().fg(rgb(palette.text)),
        )),
        Line::from(Span::styled(
            "██║  ██║╚██████╔╝ ╚████╔╝ ██║╚██████╗╚██████╔╝██████╔╝███████╗",
            Style::default().fg(rgb(palette.muted)),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Modular Rust AI Agent Harness  |  SOLID (OCP) + Vertical Slices",
            Style::default()
                .fg(rgb(palette.text))
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!("Workspace: {}", app.workspace_display),
            Style::default().fg(rgb(palette.muted)),
        )),
        Line::from(Span::styled(
            format!(
                "Loaded: {} rules  •  {} skills  •  {} MCP  •  {} plugins",
                ctx.rules.len(),
                ctx.skills.len(),
                ctx.mcp_servers.len(),
                app.registry.plugin_manifests().len()
            ),
            Style::default().fg(rgb(palette.success)),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("/models", Style::default().fg(rgb(palette.accent))),
            Span::raw(" switch provider/model   "),
            Span::styled("/local", Style::default().fg(rgb(palette.accent))),
            Span::raw(" install Ollama / llama.cpp / Whisper"),
        ]),
        Line::from(vec![
            Span::styled("/skills", Style::default().fg(rgb(palette.accent))),
            Span::raw(" inspect skills          "),
            Span::styled("/plugins", Style::default().fg(rgb(palette.accent))),
            Span::raw(" WASM plugins   "),
            Span::styled("/bots", Style::default().fg(rgb(palette.accent))),
            Span::raw(" Telegram & Discord"),
        ]),
        Line::from(vec![
            Span::styled("Tab", Style::default().fg(rgb(palette.warning))),
            Span::raw(" toggle BUILD/PLAN   "),
            Span::styled("Ctrl+P", Style::default().fg(rgb(palette.warning))),
            Span::raw(" command palette   "),
            Span::styled("Ctrl+B", Style::default().fg(rgb(palette.warning))),
            Span::raw(" sidebar   "),
            Span::styled("Ctrl+Q", Style::default().fg(rgb(palette.warning))),
            Span::raw(" quit"),
        ]),
    ];

    let widget = Paragraph::new(banner)
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: false })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Session ")
                .border_style(Style::default().fg(rgb(palette.border))),
        );
    frame.render_widget(widget, area);
}

fn render_timeline(frame: &mut Frame<'_>, app: &TuiApp, palette: ThemePalette, area: Rect) {
    let mut lines: Vec<Line<'_>> = Vec::new();
    for msg in app.engine.messages() {
        let role_color = match msg.role {
            MessageRole::User => rgb(palette.accent),
            MessageRole::Assistant => rgb(palette.success),
            MessageRole::System => rgb(palette.warning),
            MessageRole::Tool => rgb(palette.muted),
        };

        lines.push(Line::from(vec![Span::styled(
            format!("[{}]", msg.role.label()),
            Style::default().fg(role_color).add_modifier(Modifier::BOLD),
        )]));

        for content_line in msg.content.lines() {
            lines.push(Line::from(Span::styled(
                format!("  {content_line}"),
                Style::default().fg(rgb(palette.text)),
            )));
        }
        if msg.content.is_empty() {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(""));
    }

    if app.in_flight {
        if !app.streaming_buffer.is_empty() {
            lines.push(Line::from(vec![Span::styled(
                "[ASSISTANT (streaming)]",
                Style::default()
                    .fg(rgb(palette.success))
                    .add_modifier(Modifier::BOLD),
            )]));
            for content_line in app.streaming_buffer.lines() {
                lines.push(Line::from(Span::styled(
                    format!("  {content_line}"),
                    Style::default().fg(rgb(palette.text)),
                )));
            }
            lines.push(Line::from(""));
        }

        let frame_icon = SPINNER_FRAMES[app.spinner_tick % SPINNER_FRAMES.len()];
        lines.push(Line::from(vec![Span::styled(
            format!(
                "{frame_icon} Thinking with {} / {}...",
                app.engine.provider_id(),
                app.engine.model()
            ),
            Style::default()
                .fg(rgb(palette.warning))
                .add_modifier(Modifier::BOLD),
        )]));
    }

    let widget = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((app.scroll_offset, 0))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" Chat ({}) ", app.engine.session_id()))
                .border_style(Style::default().fg(rgb(palette.border))),
        );
    frame.render_widget(widget, area);
}

fn push_sidebar_list(
    lines: &mut Vec<Line<'static>>,
    title: String,
    items: Vec<String>,
    empty_label: &'static str,
    palette: ThemePalette,
) {
    lines.push(Line::from(Span::styled(
        title,
        Style::default()
            .fg(rgb(palette.accent))
            .add_modifier(Modifier::BOLD),
    )));
    if items.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("  {empty_label}"),
            Style::default().fg(rgb(palette.muted)),
        )));
    } else {
        for item in items {
            lines.push(Line::from(format!("  • {item}")));
        }
    }
    lines.push(Line::from(""));
}

fn render_sidebar(frame: &mut Frame<'_>, app: &TuiApp, palette: ThemePalette, area: Rect) {
    let ctx = app.engine.context();
    let mut lines: Vec<Line<'static>> = Vec::new();

    push_sidebar_list(
        &mut lines,
        "Rules & Instructions".to_string(),
        ctx.rules.iter().take(4).map(|r| r.name.clone()).collect(),
        "(none)",
        palette,
    );
    push_sidebar_list(
        &mut lines,
        format!("Skills ({})", ctx.skills.len()),
        ctx.skills
            .iter()
            .take(5)
            .map(|s| format!("{} [{}]", s.name, s.source_harness))
            .collect(),
        "(none)",
        palette,
    );
    push_sidebar_list(
        &mut lines,
        format!("MCP Servers ({})", ctx.mcp_servers.len()),
        ctx.mcp_servers
            .iter()
            .take(4)
            .map(|m| m.name.clone())
            .collect(),
        "(none in .mcp.json/opencode.json)",
        palette,
    );

    lines.push(Line::from(Span::styled(
        "Local AI Runtimes (/local)",
        Style::default()
            .fg(rgb(palette.accent))
            .add_modifier(Modifier::BOLD),
    )));
    for rt in &app.runtime_statuses {
        let (label, color) = if rt.running {
            ("UP", rgb(palette.success))
        } else if rt.installed {
            ("INST", rgb(palette.warning))
        } else {
            ("MISS", rgb(palette.muted))
        };
        lines.push(Line::from(vec![
            Span::raw(format!("  • {:<11} ", rt.display_name)),
            Span::styled(label, Style::default().fg(color)),
        ]));
    }
    lines.push(Line::from(""));

    push_sidebar_list(
        &mut lines,
        format!("WASM Plugins ({})", app.registry.plugin_manifests().len()),
        app.registry
            .plugin_manifests()
            .iter()
            .take(3)
            .map(|p| p.name.clone())
            .collect(),
        "drop .wasm/.wat in .ruvi/plugins",
        palette,
    );

    lines.push(Line::from(Span::styled(
        "Bot Channels (/bots)",
        Style::default()
            .fg(rgb(palette.accent))
            .add_modifier(Modifier::BOLD),
    )));
    for ch in app.registry.channels() {
        let (label, color) = if ch.is_configured() {
            ("READY", rgb(palette.success))
        } else {
            ("UNSET", rgb(palette.muted))
        };
        lines.push(Line::from(vec![
            Span::raw(format!("  • {:<11} ", ch.display_name())),
            Span::styled(label, Style::default().fg(color)),
        ]));
    }

    let widget = Paragraph::new(lines).wrap(Wrap { trim: false }).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Context & Extensions ")
            .border_style(Style::default().fg(rgb(palette.border)))
            .style(Style::default().bg(rgb(palette.surface))),
    );
    frame.render_widget(widget, area);
}

fn render_composer(frame: &mut Frame<'_>, app: &TuiApp, palette: ThemePalette, area: Rect) {
    let prompt_text = format!("> {}▌", app.input);
    let title = format!(
        " Prompt [{}] — Enter to send, /help for commands ",
        app.engine.mode().badge()
    );
    let widget = Paragraph::new(prompt_text).block(
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(rgb(palette.accent)))
            .style(Style::default().bg(rgb(palette.surface))),
    );
    frame.render_widget(widget, area);
}

fn render_footer(frame: &mut Frame<'_>, app: &TuiApp, palette: ThemePalette, area: Rect) {
    let line = Line::from(vec![
        Span::styled(
            " Ctrl+P Palette | Ctrl+L Local Installer | Tab Mode | Ctrl+B Sidebar | Ctrl+Q Quit ",
            Style::default().fg(rgb(palette.muted)),
        ),
        Span::styled(" | ", Style::default().fg(rgb(palette.border))),
        Span::styled(&app.status_line, Style::default().fg(rgb(palette.accent))),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn render_modal(frame: &mut Frame<'_>, app: &TuiApp, palette: ThemePalette, root: Rect) {
    match &app.modal {
        ActiveModal::None => {}
        ActiveModal::CommandPalette { selected } => {
            render_command_palette(frame, *selected, palette, root);
        }
        ActiveModal::ModelSelector { entries, selected } => {
            render_model_selector(frame, app, entries, *selected, palette, root);
        }
        ActiveModal::LocalInstaller {
            selected,
            busy_message,
        } => {
            render_local_installer(
                frame,
                app,
                *selected,
                busy_message.as_deref(),
                palette,
                root,
            );
        }
        ActiveModal::SessionSelector { sessions, selected } => {
            render_session_selector(frame, sessions, *selected, palette, root);
        }
        ActiveModal::Info { title, lines } => render_info_modal(frame, title, lines, palette, root),
    }
}

fn render_command_palette(
    frame: &mut Frame<'_>,
    selected: usize,
    palette: ThemePalette,
    root: Rect,
) {
    let area = centered_rect(64, 58, root);
    frame.render_widget(Clear, area);
    let items: Vec<ListItem<'_>> = COMMANDS
        .iter()
        .enumerate()
        .map(|(idx, (cmd, desc))| {
            let style = if idx == selected {
                Style::default()
                    .fg(rgb(palette.background))
                    .bg(rgb(palette.accent))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(rgb(palette.text))
            };
            ListItem::new(format!(" {cmd:<10}  {desc}")).style(style)
        })
        .collect();
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Command Palette (Enter to run, Esc to close) ")
            .border_style(Style::default().fg(rgb(palette.accent)))
            .style(Style::default().bg(rgb(palette.surface))),
    );
    frame.render_widget(list, area);
}

fn render_model_selector(
    frame: &mut Frame<'_>,
    app: &TuiApp,
    entries: &[(String, String)],
    selected: usize,
    palette: ThemePalette,
    root: Rect,
) {
    let area = centered_rect(70, 62, root);
    frame.render_widget(Clear, area);
    let items: Vec<ListItem<'_>> = entries
        .iter()
        .enumerate()
        .map(|(idx, (provider, model))| {
            let active = provider == app.engine.provider_id() && model == app.engine.model();
            let marker = if active { "*" } else { " " };
            let style = if idx == selected {
                Style::default()
                    .fg(rgb(palette.background))
                    .bg(rgb(palette.accent))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(rgb(palette.text))
            };
            ListItem::new(format!(" {marker} {provider:<18}  {model}")).style(style)
        })
        .collect();
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Select Provider & Model (Enter to select, Esc to close) ")
            .border_style(Style::default().fg(rgb(palette.accent)))
            .style(Style::default().bg(rgb(palette.surface))),
    );
    frame.render_widget(list, area);
}

fn render_local_installer(
    frame: &mut Frame<'_>,
    app: &TuiApp,
    selected: usize,
    busy_message: Option<&str>,
    palette: ThemePalette,
    root: Rect,
) {
    let area = centered_rect(78, 65, root);
    frame.render_widget(Clear, area);
    let mut lines = vec![
        Line::from(Span::styled(
            "Manage local LLM & Whisper engines directly from RuviCode:",
            Style::default().fg(rgb(palette.muted)),
        )),
        Line::from(""),
    ];
    for (idx, rt) in app.runtime_statuses.iter().enumerate() {
        let pointer = if idx == selected { ">" } else { " " };
        let state = if rt.running {
            "RUNNING"
        } else if rt.installed {
            "INSTALLED (daemon stopped)"
        } else {
            "NOT INSTALLED"
        };
        let style = if idx == selected {
            Style::default()
                .fg(rgb(palette.accent))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(rgb(palette.text))
        };
        lines.push(Line::from(Span::styled(
            format!("{pointer} {} ({}) — {state}", rt.display_name, rt.id),
            style,
        )));
        if let Some(ver) = &rt.version {
            lines.push(Line::from(Span::styled(
                format!("    version: {ver}"),
                Style::default().fg(rgb(palette.muted)),
            )));
        }
        if !rt.models.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("    models: {}", rt.models.join(", ")),
                Style::default().fg(rgb(palette.success)),
            )));
        }
        lines.push(Line::from(Span::styled(
            format!("    hint: {}", rt.install_hint),
            Style::default().fg(rgb(palette.muted)),
        )));
        lines.push(Line::from(""));
    }
    if let Some(busy) = busy_message {
        lines.push(Line::from(Span::styled(
            format!("Status: {busy}"),
            Style::default()
                .fg(rgb(palette.warning))
                .add_modifier(Modifier::BOLD),
        )));
    }
    lines.push(Line::from(Span::styled(
        "Keys: [Up/Down] select  [i] install selected  [p] pull qwen2.5-coder:7b (Ollama)  [r] refresh  [Esc] close",
        Style::default().fg(rgb(palette.accent)),
    )));

    let widget = Paragraph::new(lines).wrap(Wrap { trim: false }).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Local AI Runtime Installer ")
            .border_style(Style::default().fg(rgb(palette.accent)))
            .style(Style::default().bg(rgb(palette.surface))),
    );
    frame.render_widget(widget, area);
}

fn render_session_selector(
    frame: &mut Frame<'_>,
    sessions: &[SessionSummary],
    selected: usize,
    palette: ThemePalette,
    root: Rect,
) {
    let area = centered_rect(76, 62, root);
    frame.render_widget(Clear, area);
    let items: Vec<ListItem<'_>> = if sessions.is_empty() {
        vec![
            ListItem::new(" No saved sessions yet in .ruvi/sessions/")
                .style(Style::default().fg(rgb(palette.muted))),
        ]
    } else {
        sessions
            .iter()
            .enumerate()
            .map(|(idx, s)| {
                let style = if idx == selected {
                    Style::default()
                        .fg(rgb(palette.background))
                        .bg(rgb(palette.accent))
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(rgb(palette.text))
                };
                ListItem::new(format!(
                    " {}  ({} msgs, {}/{}) — {}",
                    s.title, s.message_count, s.provider_id, s.model, s.id
                ))
                .style(style)
            })
            .collect()
    };
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Saved Sessions (Enter to resume, Esc to close) ")
            .border_style(Style::default().fg(rgb(palette.accent)))
            .style(Style::default().bg(rgb(palette.surface))),
    );
    frame.render_widget(list, area);
}

fn render_info_modal(
    frame: &mut Frame<'_>,
    title: &str,
    lines: &[String],
    palette: ThemePalette,
    root: Rect,
) {
    let area = centered_rect(76, 65, root);
    frame.render_widget(Clear, area);
    let formatted: Vec<Line<'_>> = lines
        .iter()
        .map(|l| {
            Line::from(Span::styled(
                l.as_str(),
                Style::default().fg(rgb(palette.text)),
            ))
        })
        .collect();
    let widget = Paragraph::new(formatted).wrap(Wrap { trim: false }).block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" {title} (Esc to close) "))
            .border_style(Style::default().fg(rgb(palette.accent)))
            .style(Style::default().bg(rgb(palette.surface))),
    );
    frame.render_widget(widget, area);
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

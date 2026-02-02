use ratatui::layout::{Alignment, Constraint, Layout, Position, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::Frame;

use crate::protocol::ClusterSummary;
use crate::screens::launcher::State;
use crate::ui::theme;

/// Maximum width for the centered content area.
const MAX_CONTENT_WIDTH: u16 = 60;

pub fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &State,
    provider_override: Option<&str>,
    recent_clusters: &[ClusterSummary],
) {
    let centered = center_rect(area, MAX_CONTENT_WIDTH);

    // Vertical layout: logo (3) + input (3) + gap (1) + quick actions (6) + gap (1) + recent (rest)
    let [logo_area, input_area, _, actions_area, _, recent_area] = Layout::vertical([
        Constraint::Length(3), // logo
        Constraint::Length(3), // input
        Constraint::Length(1), // gap
        Constraint::Length(6), // quick actions
        Constraint::Length(1), // gap
        Constraint::Min(2),    // recent clusters
    ])
    .areas(centered);

    render_logo(frame, logo_area, provider_override);
    render_input(frame, input_area, state);
    render_quick_actions(frame, actions_area);
    render_recent(frame, recent_area, recent_clusters);
}

fn render_logo(frame: &mut Frame<'_>, area: Rect, provider_override: Option<&str>) {
    let provider = provider_override.unwrap_or("default");
    let lines = vec![
        Line::from(""),
        Line::from(vec![Span::styled(
            "\u{25c6}  Z E R O S H O T",
            theme::logo_style(),
        )]),
        Line::from(vec![
            Span::styled("Multi-Agent Orchestrator", theme::dim_style()),
            Span::raw("  "),
            Span::styled(format!("[{provider}]"), theme::muted_style()),
        ]),
    ];
    let widget = Paragraph::new(lines).alignment(Alignment::Center);
    frame.render_widget(widget, area);
}

fn render_input(frame: &mut Frame<'_>, area: Rect, state: &State) {
    let content = if state.input.is_empty() {
        Line::from(Span::styled(
            "Describe a task or paste an issue URL...",
            theme::muted_style(),
        ))
    } else {
        Line::from(Span::styled(state.input.as_str(), theme::title_style()))
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::focus_border_style())
        .border_type(BorderType::Rounded);

    let widget = Paragraph::new(content).block(block);
    frame.render_widget(widget, area);

    // Set cursor
    if area.height > 2 && area.width > 2 {
        let max_x = area.x + area.width.saturating_sub(2);
        let cursor_x = area.x + 1 + state.cursor as u16;
        let cursor_x = cursor_x.min(max_x);
        let cursor_y = area.y + 1;
        frame.set_cursor_position(Position::new(cursor_x, cursor_y));
    }
}

fn render_quick_actions(frame: &mut Frame<'_>, area: Rect) {
    let lines = vec![
        Line::from(vec![
            Span::raw("  "),
            Span::styled("/issue", theme::key_style()),
            Span::styled(" org/repo#123   ", theme::dim_style()),
            Span::styled("Start from issue", theme::dim_style()),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("/monitor", theme::key_style()),
            Span::styled("               ", theme::dim_style()),
            Span::styled("View active runs", theme::dim_style()),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("/provider", theme::key_style()),
            Span::styled(" <name>     ", theme::dim_style()),
            Span::styled("Switch AI model", theme::dim_style()),
        ]),
    ];

    let block = Block::default()
        .title(Span::styled(" Quick Actions ", theme::dim_style()))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::unfocus_border_style());

    let widget = Paragraph::new(lines).block(block);
    frame.render_widget(widget, area);
}

fn render_recent(frame: &mut Frame<'_>, area: Rect, clusters: &[ClusterSummary]) {
    let mut lines = vec![Line::from(Span::styled("Recent", theme::dim_style()))];

    if clusters.is_empty() {
        lines.push(Line::from(Span::styled(
            "(no recent clusters)",
            theme::muted_style(),
        )));
    } else {
        for cluster in clusters.iter().take(3) {
            let state_style = theme::status_style(&cluster.state);
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(&cluster.id, theme::dim_style()),
                Span::raw("  "),
                Span::styled(&cluster.state, state_style),
            ]));
        }
    }

    let widget = Paragraph::new(lines);
    frame.render_widget(widget, area);
}

/// Center a rect horizontally within the outer area, capping at max_width.
fn center_rect(outer: Rect, max_width: u16) -> Rect {
    let width = outer.width.min(max_width);
    let x = outer.x + (outer.width.saturating_sub(width)) / 2;

    // Vertically center if enough space (aim for ~1/3 from top)
    let content_height = 17u16; // approximate total height of all sections
    let y = if outer.height > content_height + 4 {
        outer.y + (outer.height.saturating_sub(content_height)) / 3
    } else {
        outer.y
    };
    let height = outer.height.saturating_sub(y - outer.y);

    Rect::new(x, y, width, height)
}

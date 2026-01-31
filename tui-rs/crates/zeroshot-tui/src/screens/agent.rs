use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

#[derive(Debug, Clone, Default)]
pub struct State {
    pub heartbeat: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Noop,
}

impl State {
    pub fn bump(&mut self) {
        self.heartbeat = self.heartbeat.saturating_add(1);
    }
}

pub fn render(frame: &mut Frame<'_>, area: Rect, state: &State) {
    let lines = vec![
        Line::from("Agent View"),
        Line::from(format!("Heartbeat: {}", state.heartbeat)),
        Line::from("Awaiting agent logs."),
    ];

    let widget = Paragraph::new(lines).block(Block::default().borders(Borders::ALL));
    frame.render_widget(widget, area);
}

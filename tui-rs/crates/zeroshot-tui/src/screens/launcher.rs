use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

#[derive(Debug, Clone, Default)]
pub struct State {
    pub input: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Submit,
}

pub fn render(frame: &mut Frame<'_>, area: Rect, state: &State) {
    let lines = vec![
        Line::from("Launcher"),
        Line::from("Type a task or /command to start a cluster."),
        Line::from(format!("Input: {}", state.input)),
        Line::from("Press Enter to launch, Esc to go back."),
    ];

    let widget = Paragraph::new(lines).block(Block::default().borders(Borders::ALL));
    frame.render_widget(widget, area);
}

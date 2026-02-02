use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::Frame;
use ratatui::Terminal;

pub fn buffer_lines(buffer: &Buffer) -> Vec<String> {
    let area = buffer.area;
    let mut lines = Vec::new();
    for y in area.top()..area.bottom() {
        let mut line = String::new();
        for x in area.left()..area.right() {
            line.push_str(buffer.cell((x, y)).map_or("", |c| c.symbol()));
        }
        lines.push(line);
    }
    lines
}

pub fn buffer_text(buffer: &Buffer) -> String {
    buffer_lines(buffer).join("\n")
}

pub fn render_to_buffer<F>(width: u16, height: u16, draw: F) -> Buffer
where
    F: FnOnce(&mut Frame<'_>),
{
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal.draw(draw).expect("draw");
    terminal.backend().buffer().clone()
}

pub fn render_to_text<F>(width: u16, height: u16, draw: F) -> String
where
    F: FnOnce(&mut Frame<'_>),
{
    let buffer = render_to_buffer(width, height, draw);
    buffer_text(&buffer)
}

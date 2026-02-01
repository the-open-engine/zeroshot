use ratatui::buffer::Buffer;

pub fn line_text(buffer: &Buffer, y: u16) -> String {
    let area = buffer.area;
    let mut line = String::new();
    for x in area.left()..area.right() {
        line.push_str(buffer.cell((x, y)).map_or("", |c| c.symbol()));
    }
    line
}

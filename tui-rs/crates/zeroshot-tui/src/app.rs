use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Default)]
pub struct App {
    pub tick_count: u64,
    pub last_size: Option<(u16, u16)>,
    pub should_quit: bool,
}

impl App {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn on_tick(&mut self) {
        self.tick_count = self.tick_count.saturating_add(1);
    }

    pub fn on_resize(&mut self, width: u16, height: u16) {
        self.last_size = Some((width, height));
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                self.should_quit = true;
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quits_on_q() {
        let mut app = App::new();
        app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(app.should_quit);
    }
}

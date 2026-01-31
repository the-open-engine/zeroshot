use std::env;
use std::io::{self, stdout};
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyEventKind};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Terminal;

use zeroshot_tui::app::App;
use zeroshot_tui::terminal::TerminalGuard;

fn main() -> io::Result<()> {
    let guard = TerminalGuard::new()?;
    guard.install_panic_hook();

    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;

    if env::var("ZEROSHOT_TUI_PANIC").ok().as_deref() == Some("1") {
        panic!("ZEROSHOT_TUI_PANIC=1 requested");
    }

    let mut app = App::new();
    let tick_rate = Duration::from_millis(250);
    let mut last_tick = Instant::now();

    loop {
        terminal.draw(|frame| render(frame, &app))?;

        if app.should_quit {
            break;
        }

        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(key) => {
                    if key.kind == KeyEventKind::Press {
                        app.handle_key(key);
                    }
                }
                Event::Resize(width, height) => {
                    app.on_resize(width, height);
                }
                _ => {}
            }
        }

        if last_tick.elapsed() >= tick_rate {
            app.on_tick();
            last_tick = Instant::now();
        }
    }

    drop(terminal);
    guard.restore()?;

    Ok(())
}

fn render(frame: &mut ratatui::Frame<'_>, app: &App) {
    let size = frame.size();
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1)])
        .split(size);

    let mut lines = vec![
        Line::from("Zeroshot TUI v2"),
        Line::from("Ratatui skeleton running."),
        Line::from(format!("Ticks: {}", app.tick_count)),
        Line::from("Press q, Esc, or Ctrl-C to quit."),
    ];

    if let Some((width, height)) = app.last_size {
        lines.push(Line::from(format!(
            "Last resize: {width}x{height}"
        )));
    }

    let paragraph = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("zeroshot-tui"));
    frame.render_widget(paragraph, layout[0]);
}

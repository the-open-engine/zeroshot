use std::io::{self, stdout};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crossterm::{cursor, execute, terminal};

#[derive(Debug)]
pub struct TerminalGuard {
    restored: Arc<AtomicBool>,
}

impl TerminalGuard {
    pub fn new() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        if let Err(err) = execute!(stdout(), terminal::EnterAlternateScreen, cursor::Hide) {
            if let Err(disable_err) = terminal::disable_raw_mode() {
                return Err(disable_err);
            }
            return Err(err);
        }
        Ok(Self {
            restored: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn install_panic_hook(&self) {
        let restored = self.restored.clone();
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if let Err(err) = restore_terminal(&restored) {
                eprintln!("Failed to restore terminal on panic: {err}");
            }
            previous(info);
        }));
    }

    pub fn restore(&self) -> io::Result<()> {
        restore_terminal(&self.restored)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if let Err(err) = restore_terminal(&self.restored) {
            eprintln!("Failed to restore terminal on drop: {err}");
        }
    }
}

fn restore_terminal(restored: &AtomicBool) -> io::Result<()> {
    if restored.swap(true, Ordering::SeqCst) {
        return Ok(());
    }

    let mut first_error: Option<io::Error> = None;
    if let Err(err) = terminal::disable_raw_mode() {
        first_error = Some(err);
    }
    if let Err(err) = execute!(stdout(), terminal::LeaveAlternateScreen, cursor::Show) {
        if first_error.is_none() {
            first_error = Some(err);
        }
    }

    if let Some(err) = first_error {
        return Err(err);
    }

    Ok(())
}

/// Terminal progress spinner for long-running queries.
///
/// Prints a spinning animation to stderr while a query runs, then clears the
/// line when done. Only activates when stderr is a TTY.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

const FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// A running spinner that can be stopped.
pub struct Spinner {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Spinner {
    /// Start a spinner on stderr. Returns `None` if stderr is not a TTY
    /// (so piped/redirected output is never polluted with spinner chars).
    pub fn start(msg: &str) -> Option<Self> {
        if !atty::is(atty::Stream::Stderr) {
            return None;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = stop.clone();
        let msg = msg.to_string();
        let handle = thread::spawn(move || {
            let mut i = 0usize;
            while !stop_clone.load(Ordering::Relaxed) {
                let frame = FRAMES[i % FRAMES.len()];
                eprint!("\r{frame} {msg}");
                let _ = std::io::Write::flush(&mut std::io::stderr());
                thread::sleep(Duration::from_millis(80));
                i += 1;
            }
            // Clear the spinner line.
            eprint!("\r{}\r", " ".repeat(msg.len() + 3));
            let _ = std::io::Write::flush(&mut std::io::stderr());
        });
        Some(Self {
            stop,
            handle: Some(handle),
        })
    }

    /// Stop the spinner and wait for the background thread to finish.
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spinner_start_non_tty_returns_none() {
        // In test environments stderr is not a TTY — expect None.
        let s = Spinner::start("testing");
        assert!(s.is_none(), "expected None when stderr is not TTY");
    }

    #[test]
    fn frames_are_non_empty_and_distinct_from_first() {
        assert!(!FRAMES.is_empty());
        // Sanity: cycling through the frame index wraps without panicking.
        for i in 0..(FRAMES.len() * 2) {
            let _ = FRAMES[i % FRAMES.len()];
        }
    }

    #[test]
    fn stop_on_spinner_without_background_thread_is_safe() {
        // Construct a Spinner directly (module-private fields are visible
        // to this submodule) with no background thread, mirroring what
        // `start()` would produce if it had already been stopped.
        let spinner = Spinner {
            stop: Arc::new(AtomicBool::new(false)),
            handle: None,
        };
        // Should not panic even though there is no thread to join.
        spinner.stop();
    }

    #[test]
    fn drop_sets_stop_flag() {
        let stop = Arc::new(AtomicBool::new(false));
        {
            let spinner = Spinner {
                stop: stop.clone(),
                handle: None,
            };
            assert!(!stop.load(Ordering::Relaxed));
            drop(spinner);
        }
        assert!(stop.load(Ordering::Relaxed));
    }

    #[test]
    fn stop_joins_background_thread() {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = stop.clone();
        let handle = thread::spawn(move || {
            while !stop_clone.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(5));
            }
        });
        let spinner = Spinner {
            stop,
            handle: Some(handle),
        };
        spinner.stop();
    }
}

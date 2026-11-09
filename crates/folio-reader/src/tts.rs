//! In-process speech. Pause, stop, and dropping the reader handle all stop it.
//!
//! There is no notification and no foreground service here.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Shared speaking flag. The handle and the observer look at the same value.
#[derive(Debug, Clone)]
pub struct Speech {
    speaking: Arc<AtomicBool>,
}

impl Speech {
    /// Speech that is not running.
    pub fn new() -> Self {
        Self {
            speaking: Arc::new(AtomicBool::new(false)),
        }
    }

    /// A handle the reader owns. Dropping it stops speech.
    pub fn handle(&self) -> SpeechHandle {
        SpeechHandle {
            // clone: the handle and this flag share one atomic
            speaking: Arc::clone(&self.speaking),
        }
    }

    /// Whether speech is running.
    pub fn is_speaking(&self) -> bool {
        self.speaking.load(Ordering::SeqCst)
    }
}

impl Default for Speech {
    fn default() -> Self {
        Self::new()
    }
}

/// The reader’s hold on speech.
#[derive(Debug)]
pub struct SpeechHandle {
    speaking: Arc<AtomicBool>,
}

impl SpeechHandle {
    /// Start speaking.
    pub fn start(&self) {
        self.speaking.store(true, Ordering::SeqCst);
    }

    /// Home, screen-off, or another activity covering the app.
    pub fn on_pause(&self) {
        self.stop();
    }

    /// The activity is no longer visible.
    pub fn on_stop(&self) {
        self.stop();
    }

    fn stop(&self) {
        self.speaking.store(false, Ordering::SeqCst);
    }
}

impl Drop for SpeechHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::Speech;

    #[test]
    fn tts_stops_when_reader_closes() {
        let speech = Speech::new();
        {
            let handle = speech.handle();
            handle.start();
            assert!(speech.is_speaking());
            handle.on_pause();
            assert!(!speech.is_speaking());
            handle.start();
            handle.on_stop();
            assert!(!speech.is_speaking());
            handle.start();
            assert!(speech.is_speaking());
        }
        assert!(!speech.is_speaking());
    }
}

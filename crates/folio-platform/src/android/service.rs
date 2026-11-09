//! In-process gate for work that continues after the screen is no longer visible.
//!
//! This is not an Android `Service` class.

/// Work that may keep running after the reader leaves the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundJob {
    /// Catalog scan.
    Scan,
    /// Download queue.
    Download,
    /// Reader speech.
    Tts,
}

/// Starts only when a job asks to continue past the visible screen.
#[derive(Debug, Default)]
pub struct ServiceGate {
    active: Option<BackgroundJob>,
}

impl ServiceGate {
    /// An idle gate.
    pub fn new() -> Self {
        Self { active: None }
    }

    /// `None` means the screen was left with no request, so the gate stays stopped.
    pub fn on_leave(&mut self, request: Option<BackgroundJob>) {
        self.active = request;
    }

    /// Stop the active request.
    pub fn stop(&mut self) {
        self.active = None;
    }

    /// The job still running, if one was requested.
    pub fn active(&self) -> Option<BackgroundJob> {
        self.active
    }
}

#[cfg(test)]
mod tests {
    use super::{BackgroundJob, ServiceGate};

    #[test]
    fn service_starts_only_to_continue_past_the_screen() {
        let mut gate = ServiceGate::new();
        assert!(gate.active().is_none());
        gate.on_leave(None);
        assert!(gate.active().is_none());
        for job in [
            BackgroundJob::Scan,
            BackgroundJob::Download,
            BackgroundJob::Tts,
        ] {
            gate.on_leave(Some(job));
            assert_eq!(gate.active(), Some(job));
            gate.stop();
            assert!(gate.active().is_none());
        }
        gate.on_leave(Some(BackgroundJob::Tts));
        gate.on_leave(None);
        assert!(gate.active().is_none());
    }
}

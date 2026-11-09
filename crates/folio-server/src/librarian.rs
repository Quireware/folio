//! librarian content and progress routes are not available.
//!
//! The neighbour server still has no browse, stream, or progress protocol.
//! This module does not invent those endpoints.

/// librarian has no progress-write route.
pub fn progress_write_supported() -> bool {
    false
}

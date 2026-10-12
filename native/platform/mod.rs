// Native process lifetime, window embedding, dialogs and inspection are platform adapters.
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

/// A nonblocking duplex byte stream. Adapters normalize idle reads/writes to
/// WouldBlock and closed connections to BrokenPipe (or EOF on read).
pub(crate) trait InspectionConnection: std::io::Read + std::io::Write + Send {}

#[allow(dead_code)] // Each platform implements only the variants it needs.
pub(crate) enum InspectionConnectError {
    /// The game has not created its endpoint yet, or it is currently busy.
    Retry,
    Failed(std::io::Error),
    Unsupported,
}

#[cfg(not(windows))]
pub(crate) fn connect_inspection(
    _: &str,
) -> Result<Box<dyn InspectionConnection>, InspectionConnectError> {
    Err(InspectionConnectError::Unsupported)
}

#[cfg(not(windows))]
pub(crate) fn configure_inspection(_: &mut std::process::Command, _: &str) {}

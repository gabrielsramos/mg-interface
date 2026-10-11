// Native process lifetime, window embedding and dialogs are platform adapters.
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

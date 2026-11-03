use anyhow::Result;
use std::{path::PathBuf, sync::Mutex};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipboardContent {
    Text(String),
    Image(Vec<u8>),
    Files(Vec<PathBuf>),
}

pub trait Clipboard: Send + Sync + 'static {
    fn poll_changed(&self) -> bool {
        true
    }
    fn sequence(&self) -> Result<u64>;
    fn read(&self) -> Result<Option<ClipboardContent>>;
    /// Return None if an external copy happened before this write could begin.
    fn write_if_sequence(&self, content: &ClipboardContent, expected: u64) -> Result<Option<u64>>;
}

#[derive(Default)]
pub struct MemoryClipboard {
    state: Mutex<(u64, Option<ClipboardContent>)>,
}
impl MemoryClipboard {
    pub fn set(&self, content: ClipboardContent) {
        let mut state = self.state.lock().unwrap();
        state.0 += 1;
        state.1 = Some(content);
    }
    pub fn content(&self) -> Option<ClipboardContent> {
        self.state.lock().unwrap().1.clone()
    }
}
impl Clipboard for MemoryClipboard {
    fn sequence(&self) -> Result<u64> {
        Ok(self.state.lock().unwrap().0)
    }
    fn read(&self) -> Result<Option<ClipboardContent>> {
        Ok(self.content())
    }
    fn write_if_sequence(&self, content: &ClipboardContent, expected: u64) -> Result<Option<u64>> {
        let mut state = self.state.lock().unwrap();
        if state.0 != expected {
            return Ok(None);
        }
        state.0 += 1;
        state.1 = Some(content.clone());
        Ok(Some(state.0))
    }
}

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::NativeClipboard;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::NativeClipboard;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub type NativeClipboard = MemoryClipboard;

//! Windows integration layer.
//!
//! Every public function exists on all platforms; on non-Windows targets they
//! are inert stubs so the rest of the workspace (and the UI) can be developed
//! and tested anywhere.

pub mod display;
pub mod hotkeys;
pub mod overlay;
pub mod process;
pub mod shell;
pub mod sound;
pub mod system;
pub mod watcher;

#[cfg(windows)]
pub(crate) mod wstr {
    use windows::core::HSTRING;

    /// NUL-terminated UTF-16 buffer.
    pub fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn hstring(s: &str) -> HSTRING {
        HSTRING::from(s)
    }

    pub fn from_wide(buf: &[u16]) -> String {
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..end])
    }
}

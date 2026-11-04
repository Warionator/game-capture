//! Outbound named pipe (`\\.\pipe\<name>`) that FFmpeg opens as a raw PCM
//! input file.

use crate::engine::AudioSink;
use std::io;
use windows::core::HSTRING;
use windows::Win32::Foundation::{CloseHandle, ERROR_PIPE_CONNECTED, HANDLE};
use windows::Win32::Storage::FileSystem::{FlushFileBuffers, WriteFile, PIPE_ACCESS_OUTBOUND};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_WAIT,
};

pub struct NamedPipeSink {
    handle: HANDLE,
    pub path: String,
}

// SAFETY: the handle is only used from the owning writer thread.
unsafe impl Send for NamedPipeSink {}

impl NamedPipeSink {
    /// Creates the pipe server end. FFmpeg must open `path` afterwards.
    pub fn create(name: &str) -> io::Result<Self> {
        let path = format!(r"\\.\pipe\{name}");
        let wide = HSTRING::from(path.as_str());
        let handle = unsafe {
            CreateNamedPipeW(
                &wide,
                PIPE_ACCESS_OUTBOUND,
                PIPE_TYPE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                1 << 20,
                0,
                0,
                None,
            )
        };
        if handle.is_invalid() {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { handle, path })
    }
}

impl AudioSink for NamedPipeSink {
    fn connect(&mut self) -> io::Result<()> {
        match unsafe { ConnectNamedPipe(self.handle, None) } {
            Ok(()) => Ok(()),
            Err(e) if e.code() == ERROR_PIPE_CONNECTED.to_hresult() => Ok(()),
            Err(e) => Err(io::Error::other(e)),
        }
    }

    fn write(&mut self, mut bytes: &[u8]) -> io::Result<()> {
        while !bytes.is_empty() {
            let mut written = 0u32;
            unsafe { WriteFile(self.handle, Some(bytes), Some(&mut written), None) }
                .map_err(io::Error::other)?;
            bytes = &bytes[written as usize..];
        }
        Ok(())
    }

    fn unblock_path(&self) -> Option<String> {
        Some(self.path.clone())
    }
}

impl Drop for NamedPipeSink {
    fn drop(&mut self) {
        unsafe {
            let _ = FlushFileBuffers(self.handle);
            let _ = DisconnectNamedPipe(self.handle);
            let _ = CloseHandle(self.handle);
        }
    }
}

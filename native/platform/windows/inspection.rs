//! Windows named-pipe adapter for the shared inspection protocol.
use crate::platform::{InspectionConnectError, InspectionConnection};
use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    process::Command,
};
use windows_sys::Win32::{
    Foundation::{ERROR_BROKEN_PIPE, ERROR_NO_DATA, GENERIC_READ, GENERIC_WRITE},
    Storage::FileSystem::FILE_WRITE_ATTRIBUTES,
    System::Pipes::{PIPE_NOWAIT, PeekNamedPipe, SetNamedPipeHandleState},
};

pub(crate) fn configure_inspection(command: &mut Command, endpoint: &str) {
    command.env("MG_INTERFACE_INSPECTION_PIPE", endpoint);
}

pub(crate) fn connect_inspection(
    endpoint: &str,
) -> Result<Box<dyn InspectionConnection>, InspectionConnectError> {
    let pipe = OpenOptions::new()
        .access_mode(GENERIC_READ | GENERIC_WRITE | FILE_WRITE_ATTRIBUTES)
        .open(format!(r"\\.\pipe\{endpoint}"))
        .map_err(|_| InspectionConnectError::Retry)?;
    let mode = PIPE_NOWAIT;
    // The handle is owned by `pipe`; Windows reads the mode only during this call.
    if unsafe {
        SetNamedPipeHandleState(
            pipe.as_raw_handle() as _,
            &mode,
            std::ptr::null(),
            std::ptr::null(),
        )
    } == 0
    {
        return Err(InspectionConnectError::Failed(io::Error::last_os_error()));
    }
    Ok(Box::new(NamedPipe(pipe)))
}

struct NamedPipe(File);
impl InspectionConnection for NamedPipe {}

fn normalize_error(error: io::Error) -> io::Error {
    match error.raw_os_error().map(|code| code as u32) {
        Some(ERROR_NO_DATA) => io::Error::from(io::ErrorKind::WouldBlock),
        Some(ERROR_BROKEN_PIPE) => io::Error::from(io::ErrorKind::BrokenPipe),
        _ => error,
    }
}

impl Read for NamedPipe {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        // Probe the handle to detect disconnect even when a nonblocking read
        // would otherwise report no data. No peeked bytes are consumed.
        if unsafe {
            PeekNamedPipe(
                self.0.as_raw_handle() as _,
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(normalize_error(io::Error::last_os_error()));
        }
        match self.0.read(buffer).map_err(normalize_error)? {
            0 => Err(io::ErrorKind::WouldBlock.into()),
            count => Ok(count),
        }
    }
}

impl Write for NamedPipe {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.write(bytes).map_err(normalize_error)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush().map_err(normalize_error)
    }
}

//! Version 1 inspection transport consumed by the workspace entity browser.
#![allow(dead_code)]
use serde::Deserialize;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneSnapshot {
    pub version: u32,
    #[serde(rename = "type")]
    pub message_type: String,
    pub sequence: u64,
    pub world_roots: Vec<String>,
    pub ui_roots: Vec<String>,
    pub world_transform: [f32; 16],
    pub entities: Vec<serde_json::Value>,
    #[serde(default)]
    pub selected_entity_id: Option<String>,
}
#[derive(Clone, Debug, Default)]
pub struct InspectionState {
    pub connected: bool,
    pub snapshot: Option<SceneSnapshot>,
    pub error: Option<String>,
}
pub struct InspectionSession {
    pub pipe_name: String,
    state: Arc<Mutex<InspectionState>>,
    selection: Arc<Mutex<(u64, Option<String>)>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl InspectionSession {
    pub fn select_entity(&self, id: Option<&str>) {
        let mut selection = self.selection.lock().unwrap();
        if selection.1.as_deref() != id {
            selection.0 += 1;
            selection.1 = id.map(str::to_owned);
        }
    }
    pub fn state(&self) -> InspectionState {
        self.state.lock().unwrap().clone()
    }
    pub fn start() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let pipe_name = format!(
            "mg-interface-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let state = Arc::new(Mutex::new(InspectionState::default()));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let selection = Arc::new(Mutex::new((0, None)));
        let worker = {
            let name = pipe_name.clone();
            let state = state.clone();
            let stop = stop.clone();
            let selection = selection.clone();
            std::thread::spawn(move || receive(&name, &state, &stop, &selection))
        };
        Self {
            pipe_name,
            state,
            selection,
            stop,
            worker: Some(worker),
        }
    }
}
impl Drop for InspectionSession {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn parse_frame(bytes: &[u8]) -> Result<SceneSnapshot, String> {
    let snapshot: SceneSnapshot = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if snapshot.version != 1 || snapshot.message_type != "sceneSnapshot" {
        return Err("Unsupported inspection protocol".into());
    }
    Ok(snapshot)
}
#[cfg(windows)]
fn receive(
    name: &str,
    state: &Arc<Mutex<InspectionState>>,
    stop: &std::sync::atomic::AtomicBool,
    selection: &Arc<Mutex<(u64, Option<String>)>>,
) {
    use std::{
        io::{Read, Write},
        os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
        sync::atomic::Ordering,
        time::Duration,
    };
    use windows_sys::Win32::System::Pipes::PeekNamedPipe;
    use windows_sys::Win32::System::Pipes::{PIPE_NOWAIT, SetNamedPipeHandleState};
    let path = format!(r"\\.\pipe\{name}");
    while !stop.load(Ordering::Relaxed) {
        let Ok(mut pipe) = std::fs::OpenOptions::new()
            .access_mode(0x80000000 | 0x40000000 | 0x100)
            .open(&path)
        else {
            std::thread::sleep(Duration::from_millis(50));
            continue;
        };
        let mode = PIPE_NOWAIT;
        if unsafe {
            SetNamedPipeHandleState(
                pipe.as_raw_handle() as _,
                &mode,
                std::ptr::null(),
                std::ptr::null(),
            )
        } == 0
        {
            state.lock().unwrap().error = Some(std::io::Error::last_os_error().to_string());
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }
        {
            let mut s = state.lock().unwrap();
            s.connected = true;
            s.error = None;
        }
        let mut frame = Vec::new();
        let mut buffer = [0u8; 16384];
        let mut sent_revision = None;
        let mut command = Vec::new();
        let mut command_offset = 0;
        let mut command_revision = 0;
        'connection: while !stop.load(Ordering::Relaxed) {
            if command.is_empty() {
                let desired = selection.lock().unwrap().clone();
                if sent_revision != Some(desired.0) {
                    command = serde_json::to_vec(&serde_json::json!({
                        "version": 1, "type": "selectEntity", "entityId": desired.1
                    }))
                    .unwrap();
                    command.push(b'\n');
                    command_revision = desired.0;
                    command_offset = 0;
                }
            }
            if !command.is_empty() {
                match pipe.write(&command[command_offset..]) {
                    Ok(count) => {
                        command_offset += count;
                        if command_offset == command.len() {
                            command.clear();
                            sent_revision = Some(command_revision);
                        }
                    }
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            || error.raw_os_error() == Some(232) => {}
                    Err(error) => {
                        state.lock().unwrap().error = Some(error.to_string());
                        break;
                    }
                }
            }
            if unsafe {
                PeekNamedPipe(
                    pipe.as_raw_handle() as _,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            } == 0
            {
                break;
            }
            match pipe.read(&mut buffer) {
                Ok(0) => std::thread::sleep(Duration::from_millis(20)),
                Ok(count) => {
                    for &byte in &buffer[..count] {
                        if byte == b'\n' {
                            match parse_frame(&frame) {
                                Ok(snapshot) => {
                                    let mut s = state.lock().unwrap();
                                    s.snapshot = Some(snapshot);
                                    s.error = None;
                                }
                                Err(error) => {
                                    state.lock().unwrap().error = Some(error);
                                    break 'connection;
                                }
                            }
                            frame.clear();
                        } else {
                            frame.push(byte);
                            if frame.len() >= 8 * 1024 * 1024 {
                                state.lock().unwrap().error =
                                    Some("Inspection frame exceeds 8 MiB".into());
                                break 'connection;
                            }
                        }
                    }
                }
                Err(error) if error.raw_os_error() == Some(232) => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                Err(error) => {
                    if error.raw_os_error() != Some(109) {
                        state.lock().unwrap().error = Some(error.to_string());
                    }
                    break;
                }
            }
        }
        let mut s = state.lock().unwrap();
        s.connected = false;
        s.snapshot = None;
    }
}
#[cfg(not(windows))]
fn receive(
    _: &str,
    _: &Arc<Mutex<InspectionState>>,
    _: &std::sync::atomic::AtomicBool,
    _: &Arc<Mutex<(u64, Option<String>)>>,
) {
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_empty_scene_and_rejects_unknown_protocol() {
        let frame = br#"{"version":1,"type":"sceneSnapshot","sequence":1,"worldRoots":[],"uiRoots":[],"worldTransform":[1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1],"entities":[]}"#;
        assert!(parse_frame(frame).unwrap().entities.is_empty());
        let invalid = String::from_utf8(frame.to_vec())
            .unwrap()
            .replace("\"version\":1", "\"version\":2");
        assert!(parse_frame(invalid.as_bytes()).is_err());
        assert!(parse_frame(b"{}").is_err());
    }
}

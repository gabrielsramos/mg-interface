//! Platform-independent version 1 inspection protocol and session lifecycle.
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
    endpoint: String,
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
    /// Configure the game process with this session's platform transport endpoint.
    pub fn configure_game(&self, command: &mut std::process::Command) {
        crate::platform::configure_inspection(command, &self.endpoint);
    }

    pub fn start() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let endpoint = format!(
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
            let name = endpoint.clone();
            let state = state.clone();
            let stop = stop.clone();
            let selection = selection.clone();
            std::thread::spawn(move || receive(&name, &state, &stop, &selection))
        };
        Self {
            endpoint,
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
fn receive(
    name: &str,
    state: &Arc<Mutex<InspectionState>>,
    stop: &std::sync::atomic::AtomicBool,
    selection: &Arc<Mutex<(u64, Option<String>)>>,
) {
    use std::{sync::atomic::Ordering, time::Duration};
    while !stop.load(Ordering::Relaxed) {
        let mut pipe = match crate::platform::connect_inspection(name) {
            Ok(pipe) => pipe,
            Err(crate::platform::InspectionConnectError::Retry) => {
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
            Err(crate::platform::InspectionConnectError::Failed(error)) => {
                state.lock().unwrap().error = Some(error.to_string());
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
            Err(crate::platform::InspectionConnectError::Unsupported) => return,
        };
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
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(error) => {
                        state.lock().unwrap().error = Some(error.to_string());
                        break;
                    }
                }
            }
            match pipe.read(&mut buffer) {
                Ok(0) => break,
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
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                Err(error) => {
                    if error.kind() != std::io::ErrorKind::BrokenPipe {
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

//! Platform-independent version 1 inspection protocol and session lifecycle.
#![allow(dead_code)]
use serde::Deserialize;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DebugElement {
    pub id: String,
    #[serde(rename = "type")]
    pub element_type: String,
    pub text: String,
    #[serde(default)]
    pub value: Option<bool>,
    pub font_size: Option<f32>,
    pub color: Option<String>,
    pub size: Option<f32>,
    pub gap: Option<f32>,
    pub children: Option<Vec<DebugElement>>,
}
impl DebugElement {
    fn find(&self, id: &str) -> Option<&Self> {
        if self.id == id {
            return Some(self);
        }
        self.children
            .as_deref()
            .unwrap_or_default()
            .iter()
            .find_map(|child| child.find(id))
    }
}
#[derive(Clone, Debug, Deserialize)]
pub struct DebugWindow {
    pub id: String,
    pub title: String,
    pub elements: Vec<DebugElement>,
}
impl DebugWindow {
    fn element(&self, id: &str) -> Option<&DebugElement> {
        self.elements.iter().find_map(|element| element.find(id))
    }
}
#[derive(Clone, Debug, Deserialize)]
pub struct DebugWindows {
    pub revision: u64,
    pub windows: Vec<DebugWindow>,
    pub errors: Vec<String>,
}

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
    pub windows: Option<DebugWindows>,
    pub checkbox_pending: Option<std::time::Instant>,
}
impl InspectionState {
    pub fn checkbox_waiting(&self) -> bool {
        self.checkbox_pending
            .is_some_and(|since| since.elapsed() < std::time::Duration::from_secs(5))
    }
}
pub struct InspectionSession {
    endpoint: String,
    state: Arc<Mutex<InspectionState>>,
    selection: Arc<Mutex<(u64, Option<String>)>>,
    actions: Arc<Mutex<VecDeque<Vec<u8>>>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl InspectionSession {
    pub fn set_checkbox(
        &self,
        revision: u64,
        window_id: &str,
        control_id: &str,
        value: bool,
    ) -> bool {
        let mut state = self.state.lock().unwrap();
        if !state.connected
            || state.checkbox_waiting()
            || state.windows.as_ref().is_none_or(|catalog| {
                catalog.revision != revision
                    || !catalog.windows.iter().any(|window| {
                        window.id == window_id
                            && window.element(control_id).is_some_and(|element| {
                                element.element_type == "checkbox" && element.value.is_some()
                            })
                    })
            })
        {
            return false;
        }
        let Some(command) = checkbox_command(revision, window_id, control_id, value) else {
            return false;
        };
        let mut actions = self.actions.lock().unwrap();
        if actions.len() >= 256 {
            return false;
        }
        actions.push_back(command);
        state.checkbox_pending = Some(std::time::Instant::now());
        true
    }
    pub fn invoke_button(&self, revision: u64, window_id: &str, control_id: &str) -> bool {
        let state = self.state.lock().unwrap();
        if !state.connected
            || state.windows.as_ref().is_none_or(|catalog| {
                catalog.revision != revision
                    || !catalog.windows.iter().any(|window| {
                        window.id == window_id
                            && window
                                .element(control_id)
                                .is_some_and(|element| element.element_type == "button")
                    })
            })
        {
            return false;
        }
        let Some(command) = button_command(revision, window_id, control_id) else {
            return false;
        };
        let mut actions = self.actions.lock().unwrap();
        if actions.len() >= 256 {
            return false;
        }
        actions.push_back(command);
        true
    }
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
        let actions = Arc::new(Mutex::new(VecDeque::new()));
        let worker = {
            let name = endpoint.clone();
            let state = state.clone();
            let stop = stop.clone();
            let selection = selection.clone();
            let actions = actions.clone();
            std::thread::spawn(move || receive(&name, &state, &stop, &selection, &actions))
        };
        Self {
            endpoint,
            state,
            selection,
            actions,
            stop,
            worker: Some(worker),
        }
    }
}
fn button_command(revision: u64, window_id: &str, control_id: &str) -> Option<Vec<u8>> {
    let mut bytes = serde_json::to_vec(&serde_json::json!({
        "version": 1, "type": "invokeDebugButton", "revision": revision,
        "windowId": window_id, "controlId": control_id
    }))
    .ok()?;
    bytes.push(b'\n');
    (bytes.len() <= 4096).then_some(bytes)
}
fn checkbox_command(
    revision: u64,
    window_id: &str,
    control_id: &str,
    value: bool,
) -> Option<Vec<u8>> {
    let mut bytes = serde_json::to_vec(&serde_json::json!({
        "version": 1, "type": "setDebugCheckbox", "revision": revision,
        "windowId": window_id, "controlId": control_id, "value": value
    }))
    .ok()?;
    bytes.push(b'\n');
    (bytes.len() <= 4096).then_some(bytes)
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
enum InspectionMessage {
    Scene(SceneSnapshot),
    Windows(DebugWindows),
}
fn parse_message(bytes: &[u8]) -> Result<InspectionMessage, String> {
    let header: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if header["version"].as_u64() != Some(1) {
        return Err("Unsupported inspection protocol".into());
    }
    match header["type"].as_str() {
        Some("sceneSnapshot") => parse_frame(bytes).map(InspectionMessage::Scene),
        Some("debugWindows") => serde_json::from_slice(bytes)
            .map(InspectionMessage::Windows)
            .map_err(|e| e.to_string()),
        _ => Err("Unsupported inspection message".into()),
    }
}
fn receive(
    name: &str,
    state: &Arc<Mutex<InspectionState>>,
    stop: &std::sync::atomic::AtomicBool,
    selection: &Arc<Mutex<(u64, Option<String>)>>,
    actions: &Arc<Mutex<VecDeque<Vec<u8>>>>,
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
        let mut load_pending = true;
        let mut sent_revision = None;
        let mut command = Vec::new();
        let mut command_offset = 0;
        let mut command_revision = 0;
        let mut command_is_selection = true;
        'connection: while !stop.load(Ordering::Relaxed) {
            if command.is_empty() {
                let desired = selection.lock().unwrap().clone();
                if sent_revision != Some(desired.0) {
                    command_is_selection = true;
                    command = serde_json::to_vec(&serde_json::json!({
                        "version": 1, "type": "selectEntity", "entityId": desired.1
                    }))
                    .unwrap();
                    command.push(b'\n');
                    command_revision = desired.0;
                    command_offset = 0;
                } else if load_pending {
                    load_pending = false;
                    command_is_selection = false;
                    command = b"{\"version\":1,\"type\":\"loadDebugWindows\"}\n".to_vec();
                    command_offset = 0;
                } else if let Some(action) = actions.lock().unwrap().pop_front() {
                    command_is_selection = false;
                    command = action;
                    command_offset = 0;
                }
            }
            if !command.is_empty() {
                match pipe.write(&command[command_offset..]) {
                    Ok(count) => {
                        if count == 0 {
                            break;
                        }
                        command_offset += count;
                        if command_offset == command.len() {
                            command.clear();
                            if command_is_selection {
                                sent_revision = Some(command_revision);
                            }
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
                            match parse_message(&frame) {
                                Ok(InspectionMessage::Scene(snapshot)) => {
                                    let mut s = state.lock().unwrap();
                                    s.snapshot = Some(snapshot);
                                    s.error = None;
                                }
                                Ok(InspectionMessage::Windows(windows)) => {
                                    let mut s = state.lock().unwrap();
                                    s.windows = Some(windows);
                                    s.checkbox_pending = None;
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
        s.windows = None;
        s.checkbox_pending = None;
        actions.lock().unwrap().clear();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn button_queue_checks_catalog_revision_and_capacity() {
        let catalog = DebugWindows {
            revision: 7,
            errors: vec![],
            windows: vec![DebugWindow {
                id: "game".into(),
                title: "Game".into(),
                elements: vec![DebugElement {
                    id: "go".into(),
                    element_type: "button".into(),
                    text: "Go".into(),
                    value: None,
                    font_size: None,
                    color: None,
                    size: None,
                    gap: None,
                    children: None,
                }],
            }],
        };
        let session = InspectionSession {
            endpoint: String::new(),
            state: Arc::new(Mutex::new(InspectionState {
                connected: true,
                windows: Some(catalog),
                ..Default::default()
            })),
            selection: Arc::new(Mutex::new((0, None))),
            actions: Arc::new(Mutex::new(VecDeque::new())),
            stop: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            worker: None,
        };
        assert!(!session.invoke_button(6, "game", "go"));
        assert!(!session.invoke_button(7, "game", "missing"));
        for _ in 0..256 {
            assert!(session.invoke_button(7, "game", "go"));
        }
        assert!(!session.invoke_button(7, "game", "go"));
        let bytes = session.actions.lock().unwrap().pop_front().unwrap();
        let command: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(command["type"], "invokeDebugButton");
        assert_eq!(command["revision"], 7);
        session.state.lock().unwrap().connected = false;
        assert!(!session.invoke_button(7, "game", "go"));
        assert!(button_command(7, "game", &"x".repeat(4096)).is_none());
    }
    #[test]
    fn checkbox_queue_waits_for_confirmation_and_rejects_wrong_controls() {
        let catalog: DebugWindows = serde_json::from_value(serde_json::json!({
            "revision":1,"errors":[],"windows":[{"id":"game","title":"Game","elements":[{"id":"row","type":"row","text":"","gap":8,"children":[
                {"id":"check","type":"checkbox","text":"Enabled","value":false},
                {"id":"divider","type":"separator","text":""}]}]}]
        }))
        .unwrap();
        assert_eq!(
            catalog.windows[0].element("check").unwrap().value,
            Some(false)
        );
        let session = InspectionSession {
            endpoint: String::new(),
            state: Arc::new(Mutex::new(InspectionState {
                connected: true,
                windows: Some(catalog),
                ..Default::default()
            })),
            selection: Arc::new(Mutex::new((0, None))),
            actions: Arc::new(Mutex::new(VecDeque::new())),
            stop: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            worker: None,
        };
        assert!(!session.set_checkbox(0, "game", "check", true));
        assert!(!session.set_checkbox(1, "game", "divider", true));
        assert!(session.set_checkbox(1, "game", "check", true));
        assert!(!session.set_checkbox(1, "game", "check", false));
        assert!(session.state().checkbox_waiting());
        let command: serde_json::Value =
            serde_json::from_slice(&session.actions.lock().unwrap().pop_front().unwrap()).unwrap();
        assert_eq!(command["type"], "setDebugCheckbox");
        assert_eq!(command["value"], true);
        session.state.lock().unwrap().checkbox_pending = None;
        assert!(session.set_checkbox(1, "game", "check", false));
        assert!(checkbox_command(1, "game", &"x".repeat(4096), true).is_none());
    }
    #[test]
    fn nested_rows_preserve_text_style_spacing_and_button_lookup() {
        let frame = serde_json::to_vec(&serde_json::json!({
            "version":1,"type":"debugWindows","revision":1,"errors":[],
            "windows":[{"id":"game","title":"Game","elements":[
                {"id":"heading","type":"label","text":"Title","fontSize":20,"color":"#F4C95D"},
                {"id":"spacer","type":"space","text":"","size":12},
                {"id":"row","type":"row","text":"","gap":8,"children":[
                    {"id":"go","type":"button","text":"Go"}]}]}]
        }))
        .unwrap();
        let InspectionMessage::Windows(catalog) = parse_message(&frame).unwrap() else {
            panic!("Expected catalog")
        };
        let window = &catalog.windows[0];
        assert_eq!(window.element("heading").unwrap().font_size, Some(20.));
        assert_eq!(
            window.element("heading").unwrap().color.as_deref(),
            Some("#F4C95D")
        );
        assert_eq!(window.element("spacer").unwrap().size, Some(12.));
        assert_eq!(window.element("row").unwrap().gap, Some(8.));
        assert_eq!(window.element("go").unwrap().element_type, "button");
        assert!(window.element("missing").is_none());
    }
    #[test]
    fn parses_window_catalog_and_rejects_invalid_frames() {
        let frame = br#"{"version":1,"type":"debugWindows","revision":2,"windows":[{"id":"Gameplay","title":"Gameplay","elements":[{"id":"hello","type":"label","text":"Hello"},{"id":"go","type":"button","text":"Go"}]}],"errors":[]}"#;
        let InspectionMessage::Windows(catalog) = parse_message(frame).unwrap() else {
            panic!("Expected window catalog");
        };
        assert_eq!(catalog.revision, 2);
        assert_eq!(catalog.windows[0].elements[1].element_type, "button");
        assert!(parse_message(br#"{"version":1,"type":"debugWindows"}"#).is_err());
        assert!(
            parse_message(
                br#"{"version":2,"type":"debugWindows","revision":1,"windows":[],"errors":[]}"#
            )
            .is_err()
        );
        assert!(parse_message(br#"{"version":1,"type":"unknown"}"#).is_err());
    }
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

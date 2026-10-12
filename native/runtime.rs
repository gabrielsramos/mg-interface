#[path = "inspection.rs"]
pub(crate) mod inspection;
use std::{
    collections::VecDeque,
    ffi::OsString,
    io::{BufRead, BufReader, Read},
    path::PathBuf,
    process::{Command, ExitStatus},
    sync::{Arc, Mutex},
};

#[derive(Clone, Debug)]
pub struct LogEntry {
    pub timestamp: String,
    pub message: String,
}

#[cfg(windows)]
fn log_timestamp() -> String {
    let mut time = windows_sys::Win32::Foundation::SYSTEMTIME::default();
    unsafe { windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut time) };
    format!("{:02}:{:02}:{:02}", time.wHour, time.wMinute, time.wSecond)
}
#[cfg(not(windows))]
fn log_timestamp() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        % 86400;
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}
#[derive(Clone, Default)]
pub struct ConsoleLog(Arc<Mutex<LogState>>);
#[derive(Default)]
struct LogState {
    lines: VecDeque<LogEntry>,
    changed: bool,
}
impl ConsoleLog {
    pub fn push(&self, line: impl AsRef<str>) {
        let mut state = self.0.lock().unwrap();
        state.lines.push_back(LogEntry {
            timestamp: log_timestamp(),
            message: line.as_ref().chars().take(4096).collect(),
        });
        while state.lines.len() > 200 {
            state.lines.pop_front();
        }
        state.changed = true;
    }
    pub fn take_changed(&self) -> Option<Vec<LogEntry>> {
        let mut state = self.0.lock().unwrap();
        if !state.changed {
            return None;
        }
        state.changed = false;
        Some(state.lines.iter().cloned().collect())
    }
    pub fn capture(
        &self,
        stream: impl Read + Send + 'static,
        stderr: bool,
    ) -> std::thread::JoinHandle<()> {
        let log = self.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stream).lines() {
                match line {
                    Ok(line) => {
                        if stderr {
                            eprintln!("{line}");
                        } else {
                            println!("{line}");
                        }
                        log.push(line);
                    }
                    Err(error) => {
                        log.push(format!("Output read error: {error}"));
                        break;
                    }
                }
            }
        })
    }
}
pub struct LaunchSpec {
    pub project: PathBuf,
    pub args: Vec<OsString>,
}

pub fn validate_project(path: PathBuf) -> Result<PathBuf, String> {
    let path = dunce::canonicalize(path).map_err(|e| format!("Project unavailable: {e}"))?;
    if !path.is_file()
        || !path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("csproj"))
    {
        return Err("Select a standalone MonoGame .csproj file.".into());
    }
    Ok(path)
}

impl LaunchSpec {
    fn command(&self) -> Result<Command, String> {
        let mut command = Command::new("dotnet");
        command
            .arg("run")
            .arg("--project")
            .arg(&self.project)
            .arg("--configuration")
            .arg("Debug")
            .arg("-p:MGInterfaceInspection=true")
            .arg("--")
            .args(&self.args)
            .current_dir(self.project.parent().ok_or("Project has no parent")?);
        Ok(command)
    }
}

/// Owns a process tree. Dropping it must stop and reap the supervised process.
pub trait ManagedProcess {
    fn id(&self) -> u32;
    fn try_wait(&mut self) -> Result<Option<ExitStatus>, String>;
}

/// Applies platform lifecycle setup to a command prepared by shared code.
pub trait ProcessSupervisor {
    fn spawn(&self, command: Command, log: ConsoleLog) -> Result<Box<dyn ManagedProcess>, String>;
}

#[derive(Default)]
pub struct GameRunner {
    pub log: ConsoleLog,
    project: Option<LaunchSpec>,
    process: Option<Box<dyn ManagedProcess>>,
    inspection: Option<inspection::InspectionSession>,
}

impl GameRunner {
    pub fn select_entity(&self, id: Option<&str>) {
        if let Some(session) = &self.inspection {
            session.select_entity(id);
        }
    }
    pub fn inspection_state(&self) -> inspection::InspectionState {
        self.inspection
            .as_ref()
            .map(|s| s.state())
            .unwrap_or_default()
    }

    pub fn is_active(&self) -> bool {
        self.process.is_some()
    }
    pub fn process_id(&self) -> Option<u32> {
        self.process.as_ref().map(|p| p.id())
    }

    pub fn select(&mut self, mut spec: LaunchSpec) -> Result<(), String> {
        // Invalid selections leave the existing project and run intact.
        spec.project = validate_project(spec.project)?;
        self.stop();
        self.project = Some(spec);
        Ok(())
    }

    pub fn play(&mut self, supervisor: &dyn ProcessSupervisor) -> Result<(), String> {
        if self.is_active() {
            return Ok(());
        }
        let mut command = self
            .project
            .as_ref()
            .ok_or("Open a project first.")?
            .command()?;
        self.log = ConsoleLog::default();
        let inspection = inspection::InspectionSession::start();
        command.env("MG_INTERFACE_INSPECTION_PIPE", &inspection.pipe_name);
        self.process = Some(supervisor.spawn(command, self.log.clone())?);
        self.inspection = Some(inspection);
        Ok(())
    }

    pub fn stop(&mut self) {
        self.process.take();
        self.inspection.take();
        self.log = ConsoleLog::default();
    }

    pub fn poll_exit(&mut self) -> Result<Option<ExitStatus>, String> {
        let status = match self.process.as_mut() {
            Some(process) => process.try_wait()?,
            None => None,
        };
        if status.is_some() {
            self.stop();
        }
        Ok(status)
    }
}

#[cfg(test)]
mod console_tests {
    use super::{ConsoleLog, GameRunner};
    use std::io::Cursor;

    #[test]
    fn inspection_build_property_is_passed_before_game_arguments() {
        let spec = super::LaunchSpec {
            project: std::path::PathBuf::from("game/Game.csproj"),
            args: vec!["--example".into()],
        };
        let command = spec.command().unwrap();
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "run",
                "--project",
                "game/Game.csproj",
                "--configuration",
                "Debug",
                "-p:MGInterfaceInspection=true",
                "--",
                "--example"
            ]
        );
    }

    #[test]
    fn stopping_discards_output_from_previous_run() {
        let mut runner = GameRunner::default();
        let previous_output = runner.log.clone();
        previous_output.push("old output");
        runner.stop();
        previous_output.push("late output after stop");
        assert!(runner.log.take_changed().is_none());
        runner.log.push("new run output");
        assert_eq!(
            runner
                .log
                .take_changed()
                .map(|entries| entries[0].message.clone())
                .as_deref(),
            Some("new run output")
        );
    }
    #[test]
    fn captures_both_streams_and_final_line_without_newline() {
        let log = ConsoleLog::default();
        log.capture(
            Cursor::new("build output\r\nOlá\nfinal line".as_bytes()),
            false,
        )
        .join()
        .unwrap();
        log.capture(Cursor::new(b"build error\n"), true)
            .join()
            .unwrap();
        assert_eq!(
            log.take_changed()
                .map(|entries| entries
                    .iter()
                    .map(|entry| entry.message.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"))
                .as_deref(),
            Some("build output\nOlá\nfinal line\nbuild error")
        );
        assert!(log.take_changed().is_none());
    }

    #[test]
    fn keeps_recent_output_and_bounds_long_messages() {
        let log = ConsoleLog::default();
        for index in 0..250 {
            log.push(index.to_string());
        }
        let output = log.take_changed().unwrap();
        assert_eq!(output.len(), 200);
        for entry in &output {
            assert_eq!(entry.timestamp.len(), 8);
            let parts: Vec<u32> = entry
                .timestamp
                .split(':')
                .map(|part| part.parse().unwrap())
                .collect();
            assert!(parts[0] < 24 && parts[1] < 60 && parts[2] < 60);
        }
        assert_eq!(
            output.first().map(|entry| entry.message.as_str()),
            Some("50")
        );
        assert_eq!(
            output.last().map(|entry| entry.message.as_str()),
            Some("249")
        );
        log.push("á".repeat(8000));
        assert_eq!(
            log.take_changed()
                .unwrap()
                .last()
                .unwrap()
                .message
                .chars()
                .count(),
            4096
        );
    }
}

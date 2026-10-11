use std::{
    ffi::OsString,
    path::PathBuf,
    process::{Command, ExitStatus},
};

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
    fn spawn(&self, command: Command) -> Result<Box<dyn ManagedProcess>, String>;
}

#[derive(Default)]
pub struct GameRunner {
    project: Option<LaunchSpec>,
    process: Option<Box<dyn ManagedProcess>>,
}

impl GameRunner {
    pub fn project(&self) -> Option<&LaunchSpec> {
        self.project.as_ref()
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
        let command = self
            .project
            .as_ref()
            .ok_or("Open a project first.")?
            .command()?;
        self.process = Some(supervisor.spawn(command)?);
        Ok(())
    }

    pub fn stop(&mut self) {
        self.process.take();
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

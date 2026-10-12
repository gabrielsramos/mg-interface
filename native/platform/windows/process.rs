use super::error;
use crate::runtime::{ConsoleLog, ManagedProcess, ProcessSupervisor};
use std::{
    mem::size_of,
    os::windows::process::CommandExt,
    process::{Child, Command, ExitStatus, Stdio},
    ptr::null,
};
use windows_sys::Win32::{
    Foundation::*,
    System::{Diagnostics::ToolHelp::*, JobObjects::*, Threading::*},
};

pub struct WindowsSupervisor;
impl ProcessSupervisor for WindowsSupervisor {
    fn spawn(&self, command: Command, log: ConsoleLog) -> Result<Box<dyn ManagedProcess>, String> {
        launch(command, log).map(|process| Box::new(process) as Box<dyn ManagedProcess>)
    }
}
impl ManagedProcess for Session {
    fn id(&self) -> u32 {
        self.child.id()
    }
    fn try_wait(&mut self) -> Result<Option<ExitStatus>, String> {
        self.child.try_wait().map_err(|error| error.to_string())
    }
}
struct Session {
    child: Child,
    job: HANDLE,
}
impl Drop for Session {
    fn drop(&mut self) {
        unsafe {
            TerminateJobObject(self.job, 0);
            CloseHandle(self.job);
        }
        let _ = self.child.wait();
    }
}

fn launch(mut command: Command, log: ConsoleLog) -> Result<Session, String> {
    unsafe {
        let job = CreateJobObjectW(null(), null());
        if job.is_null() {
            return Err(error("CreateJobObjectW"));
        }
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &limits as *const _ as _,
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ) == 0
        {
            let failure = error("SetInformationJobObject");
            CloseHandle(job);
            return Err(failure);
        }
        // Suspend before assigning the job so no child can escape supervision.
        let spawned = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_SUSPENDED)
            .spawn();
        let mut child = match spawned {
            Ok(c) => c,
            Err(e) => {
                CloseHandle(job);

                return Err(format!("Process launch: {e}"));
            }
        };
        use std::os::windows::io::AsRawHandle;
        if AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE) == 0 {
            let failure = error("AssignProcessToJobObject");
            let _ = child.kill();
            let _ = child.wait();
            CloseHandle(job);

            return Err(failure);
        }
        if let Some(stdout) = child.stdout.take() {
            log.capture(stdout, false);
        }
        if let Some(stderr) = child.stderr.take() {
            log.capture(stderr, true);
        }
        let session = Session { child, job };
        // std::process does not expose the initial thread handle; enumerate it.
        let threads = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = size_of::<THREADENTRY32>() as u32;
        let mut resumed = false;
        if threads != INVALID_HANDLE_VALUE {
            if Thread32First(threads, &mut entry) != 0 {
                loop {
                    if entry.th32OwnerProcessID == session.child.id() {
                        let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                        if !thread.is_null() {
                            resumed = ResumeThread(thread) != u32::MAX;
                            CloseHandle(thread);
                        }
                        break;
                    }
                    if Thread32Next(threads, &mut entry) == 0 {
                        break;
                    }
                }
            }
            CloseHandle(threads);
        }
        if !resumed {
            return Err("Could not resume supervised process".into());
        }
        Ok(session)
    }
}

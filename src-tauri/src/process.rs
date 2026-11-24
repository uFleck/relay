//! Shared Windows child-process guarantees. Domain adapters build the command and interpret the
//! result. This module owns hidden startup, Job Object lifetime, bounded streams and deadlines.

use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(20);
const JOIN_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub(crate) struct Output {
    pub success: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub stdin_error: Option<String>,
}

#[derive(Debug)]
pub(crate) enum ProcessError {
    Start(String),
    Timeout(String),
    Cleanup(String),
}

impl std::fmt::Display for ProcessError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Start(reason) => formatter.write_str(reason),
            Self::Timeout(reason) => formatter.write_str(reason),
            Self::Cleanup(reason) => formatter.write_str(reason),
        }
    }
}

impl std::error::Error for ProcessError {}

pub(crate) fn run(
    mut command: Command,
    input: Option<&[u8]>,
    timeout: Duration,
    max_stdout: usize,
    max_stderr: usize,
    label: &str,
) -> Result<Output, ProcessError> {
    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let (mut child, mut job) = spawn(command, label)?;

    let stdin = child.stdin.take();
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ProcessError::Start(format!("{label} stdout is not piped")))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ProcessError::Start(format!("{label} stderr is not piped")))?;
    let payload = input.unwrap_or_default().to_vec();
    let out_thread = std::thread::spawn(move || read_bounded(stdout, max_stdout));
    let err_thread = std::thread::spawn(move || read_bounded(stderr, max_stderr));
    let write_thread = std::thread::spawn(move || {
        if let Some(mut stdin) = stdin {
            stdin
                .write_all(&payload)
                .map_err(|error| error.to_string())?;
            stdin.flush().map_err(|error| error.to_string())?;
        }
        Ok::<(), String>(())
    });

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(error) => {
                terminate(&mut child, &mut job);
                let _ = join_bounded(out_thread, "stdout");
                let _ = join_bounded(err_thread, "stderr");
                let _ = join_bounded(write_thread, "stdin");
                return Err(ProcessError::Start(format!(
                    "cannot wait for {label}: {error}"
                )));
            }
        }
        if started.elapsed() >= timeout {
            terminate(&mut child, &mut job);
            let cleanup = (|| -> Result<(), String> {
                join_bounded(out_thread, "stdout")?;
                join_bounded(err_thread, "stderr")?;
                let _ = join_bounded(write_thread, "stdin")?;
                Ok(())
            })();
            let reason = format!("{label} timed out after {}s", timeout.as_secs());
            return Err(match cleanup {
                Ok(()) => ProcessError::Timeout(reason),
                Err(error) => ProcessError::Cleanup(format!("{reason}; cleanup failed: {error}")),
            });
        }
        std::thread::sleep(POLL);
    };

    // The direct child has exited. Kill any grandchild before joining pipe readers so a leftover
    // process cannot hold a pipe open past the deadline.
    #[cfg(windows)]
    if let Some(job) = job.take() {
        job.terminate();
    }
    let stdout = join_bounded(out_thread, "stdout")
        .map_err(|error| ProcessError::Cleanup(format!("{label} {error}")))?;
    let stderr = join_bounded(err_thread, "stderr")
        .map_err(|error| ProcessError::Cleanup(format!("{label} {error}")))?;
    let write = join_bounded(write_thread, "stdin")
        .map_err(|error| ProcessError::Cleanup(format!("{label} {error}")))?;
    let status =
        status.ok_or_else(|| ProcessError::Start(format!("{label} has no exit status")))?;
    Ok(Output {
        success: status.success(),
        stdout,
        stderr,
        stdin_error: write.err(),
    })
}

fn read_bounded<R: Read>(mut reader: R, limit: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => {
                if bytes.len() < limit {
                    let remaining = limit - bytes.len();
                    bytes.extend_from_slice(&chunk[..count.min(remaining)]);
                }
            }
            Err(_) => break,
        }
    }
    bytes
}

fn join_bounded<T>(handle: std::thread::JoinHandle<T>, label: &str) -> Result<T, String> {
    let deadline = Instant::now() + JOIN_TIMEOUT;
    while !handle.is_finished() {
        if Instant::now() >= deadline {
            return Err(format!("{label} did not stop after the child exited"));
        }
        std::thread::sleep(POLL);
    }
    handle.join().map_err(|_| format!("{label} panicked"))
}

fn terminate(child: &mut Child, #[allow(unused_variables)] job: &mut Option<JobObject>) {
    let _ = child.kill();
    #[cfg(windows)]
    if let Some(job) = job.take() {
        job.terminate();
    }
    let _ = child.wait();
}

#[cfg(windows)]
fn spawn(mut command: Command, label: &str) -> Result<(Child, Option<JobObject>), ProcessError> {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
    let mut child = command
        .spawn()
        .map_err(|error| ProcessError::Start(format!("cannot start {label}: {error}")))?;
    let Some(job) = attach_job_object(&child) else {
        let _ = child.kill();
        return Err(ProcessError::Start(format!(
            "Job Object assignment failed; refusing to run {label}"
        )));
    };
    Ok((child, Some(job)))
}

#[cfg(not(windows))]
fn spawn(command: Command, label: &str) -> Result<(Child, Option<JobObject>), ProcessError> {
    let child = command
        .spawn()
        .map_err(|error| ProcessError::Start(format!("cannot start {label}: {error}")))?;
    Ok((child, None))
}

#[cfg(not(windows))]
struct JobObject;

#[cfg(windows)]
struct JobObject(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl JobObject {
    fn terminate(&self) {
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0, 1);
        }
    }
}

#[cfg(windows)]
impl Drop for JobObject {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(windows)]
fn attach_job_object(child: &Child) -> Option<JobObject> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return None;
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            std::mem::size_of_val(&info) as u32,
        ) == 0
            || AssignProcessToJobObject(job, child.as_raw_handle() as _) == 0
        {
            windows_sys::Win32::Foundation::CloseHandle(job);
            return None;
        }
        Some(JobObject(job))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn bounded_reader_keeps_the_limit() {
        let bytes = read_bounded(Cursor::new(vec![b'x'; 100]), 10);
        assert_eq!(bytes, vec![b'x'; 10]);
    }

    #[cfg(windows)]
    #[test]
    fn hidden_spawn_captures_output() {
        let mut command = Command::new("cmd.exe");
        command.args(["/D", "/V:OFF", "/S", "/C", "echo relay-process"]);
        let output = run(
            command,
            None,
            Duration::from_secs(2),
            1024,
            1024,
            "process fixture",
        )
        .expect("process fixture");
        assert!(output.success);
        assert!(String::from_utf8_lossy(&output.stdout).contains("relay-process"));
    }

    #[cfg(windows)]
    #[test]
    fn grandchild_cannot_hold_pipes_after_parent_exit() {
        let path = std::env::temp_dir().join(format!(
            "relay-process-{}-{}.cmd",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(
            &path,
            "@echo off\r\nstart \"\" powershell -NoProfile -Command \"Start-Sleep -Seconds 5\"\r\n",
        )
        .expect("write fixture");
        let mut command = Command::new("cmd.exe");
        command.args(["/D", "/V:OFF", "/S", "/C", path.to_str().unwrap()]);
        let started = Instant::now();
        let output = run(
            command,
            None,
            Duration::from_secs(2),
            1024,
            1024,
            "grandchild fixture",
        )
        .expect("grandchild fixture");
        std::fs::remove_file(&path).ok();
        assert!(output.success);
        assert!(started.elapsed() < Duration::from_millis(1500));
    }

    #[cfg(windows)]
    #[test]
    fn timeout_terminates_the_process_tree() {
        let mut command = Command::new("cmd.exe");
        command.args(["/D", "/V:OFF", "/S", "/C", "ping -n 6 127.0.0.1 >nul"]);
        let started = Instant::now();
        let error = run(
            command,
            None,
            Duration::from_millis(100),
            1024,
            1024,
            "timeout fixture",
        )
        .unwrap_err();
        assert!(matches!(error, ProcessError::Timeout(_)));
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}

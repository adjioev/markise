//! Running external programs for export (Pandoc, custom export commands) safely:
//!
//! - **No shell.** A command template is split into an argument vector *before* variables are
//!   substituted ([`split_command`], [`expand_argv`]); every substituted value stays inside its
//!   own argument, so document-controlled values (front matter, file names) can never inject
//!   shell syntax (`$(…)`, backticks, quotes, `;`, `%VAR%`).
//! - **Process tree control.** On Unix the child runs in its own process group and the whole
//!   group is killed on cancellation or timeout; on Windows `taskkill /T /F` ends the tree.
//! - **Bounded capture.** stdout/stderr are drained on reader threads into capped buffers (the
//!   rest is read and discarded), and the caller never blocks on a pipe held open by a
//!   descendant beyond the deadline.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use crate::job::JobControl;
use crate::vars::Vars;

/// Capture limits.
pub const MAX_STDOUT: usize = 64 << 20;
pub const MAX_STDERR: usize = 64 << 10;

/// Errors of [`run`].
#[derive(Debug, Clone, PartialEq)]
pub enum ProcError {
    NotFound(String),
    Io(String),
    Failed {
        code: Option<i32>,
        stderr: String,
    },
    Timeout(Duration),
    Cancelled,
    /// The process tree could not be shut down within the deadline (processes may still be
    /// alive, e.g. holding output files).
    Shutdown(String),
}

impl std::fmt::Display for ProcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProcError::NotFound(p) => write!(f, "program not found: {p}"),
            ProcError::Io(e) => write!(f, "cannot run the program: {e}"),
            ProcError::Failed { code, stderr } => match code {
                Some(c) => write!(f, "failed (exit code {c}): {}", stderr.trim()),
                None => write!(f, "terminated by a signal: {}", stderr.trim()),
            },
            ProcError::Timeout(d) => write!(f, "did not finish within {} s", d.as_secs()),
            ProcError::Cancelled => f.write_str("export cancelled"),
            ProcError::Shutdown(e) => write!(f, "could not stop the command: {e}"),
        }
    }
}

impl std::error::Error for ProcError {}

/// Splits a command template into arguments: whitespace separates; `'…'` and `"…"` group
/// (quotes removed; inside `"…"` a backslash escapes `"` and `\`); outside quotes a backslash
/// escapes the next character (on Windows a backslash is literal, so paths work). No other
/// syntax is interpreted: there is no shell.
pub fn split_command(template: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut have = false;
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if have {
                    out.push(std::mem::take(&mut cur));
                    have = false;
                }
            }
            '\'' => {
                have = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(x) => cur.push(x),
                        None => return Err("unterminated ' quote in command".into()),
                    }
                }
            }
            '"' => {
                have = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') if matches!(chars.peek(), Some('"') | Some('\\')) => {
                            cur.push(chars.next().unwrap_or('\\'));
                        }
                        Some(x) => cur.push(x),
                        None => return Err("unterminated \" quote in command".into()),
                    }
                }
            }
            '\\' if !cfg!(windows) => {
                have = true;
                if let Some(x) = chars.next() {
                    cur.push(x);
                }
            }
            c => {
                have = true;
                cur.push(c);
            }
        }
    }
    if have {
        out.push(cur);
    }
    if out.is_empty() {
        return Err("empty command".into());
    }
    Ok(out)
}

/// Splits `template` and substitutes variables inside each argument (values are never
/// re-split or interpreted).
pub fn expand_argv(template: &str, vars: &Vars) -> Result<Vec<String>, String> {
    Ok(split_command(template)?.iter().map(|a| vars.expand(a)).collect())
}

fn reader(mut r: impl Read + Send + 'static, cap: usize) -> Receiver<Vec<u8>> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let mut keep = Vec::new();
        let mut buf = [0u8; 16 * 1024];
        loop {
            match r.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let room = cap.saturating_sub(keep.len());
                    keep.extend_from_slice(&buf[..n.min(room)]);
                }
            }
        }
        let _ = tx.send(keep);
    });
    rx
}

#[cfg(unix)]
fn configure(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
}

/// Windows: the child starts suspended so it is inside the job object before it can run (and
/// start descendants); [`Tree::attach`] resumes it.
#[cfg(windows)]
fn configure(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(windows_sys::Win32::System::Threading::CREATE_SUSPENDED);
}

#[cfg(not(any(unix, windows)))]
fn configure(_cmd: &mut Command) {}

/// Ownership of the child's process tree: a process group on Unix, a job object (closing it
/// kills every process in it) on Windows.
struct Tree {
    #[cfg(windows)]
    job: windows_sys::Win32::Foundation::HANDLE,
    pid: u32,
}

#[cfg(windows)]
mod win {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
    };
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

    fn last_error(what: &str) -> String {
        format!("{what}: {}", std::io::Error::last_os_error())
    }

    /// Creates a kill-on-close job and puts `process` in it. On error the job is closed.
    pub fn job_for(process: HANDLE) -> Result<HANDLE, String> {
        // SAFETY: plain Win32 calls; `info` is a zero-initialised POD struct of the size
        // passed; `process` is a valid process handle owned by the caller.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(last_error("CreateJobObjectW"));
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                let e = last_error("SetInformationJobObject");
                CloseHandle(job);
                return Err(e);
            }
            if AssignProcessToJobObject(job, process) == 0 {
                let e = last_error("AssignProcessToJobObject");
                CloseHandle(job);
                return Err(e);
            }
            Ok(job)
        }
    }

    /// Number of live processes in the job.
    pub fn active(job: HANDLE) -> Result<u32, String> {
        use windows_sys::Win32::System::JobObjects::{
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JobObjectBasicAccountingInformation, QueryInformationJobObject,
        };
        // SAFETY: `info` is a zero-initialised POD struct of the size passed; `job` is a valid
        // job handle owned by the caller.
        unsafe {
            let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = std::mem::zeroed();
            let ok = QueryInformationJobObject(
                job,
                JobObjectBasicAccountingInformation,
                &mut info as *mut _ as *mut core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                std::ptr::null_mut(),
            );
            if ok == 0 {
                Err(last_error("QueryInformationJobObject"))
            } else {
                Ok(info.ActiveProcesses)
            }
        }
    }

    /// Resumes every thread of the (suspended) process `pid`; fails when none was resumed.
    pub fn resume(pid: u32) -> Result<(), String> {
        // SAFETY: Toolhelp snapshot iteration with a correctly sized THREADENTRY32; every
        // handle opened here is closed here.
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snap == INVALID_HANDLE_VALUE {
                return Err(last_error("CreateToolhelp32Snapshot"));
            }
            let mut te: THREADENTRY32 = std::mem::zeroed();
            te.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
            let mut resumed = 0;
            let mut more = Thread32First(snap, &mut te) != 0;
            while more {
                if te.th32OwnerProcessID == pid {
                    let t = OpenThread(THREAD_SUSPEND_RESUME, 0, te.th32ThreadID);
                    if !t.is_null() {
                        if ResumeThread(t) != u32::MAX {
                            resumed += 1;
                        }
                        CloseHandle(t);
                    }
                }
                more = Thread32Next(snap, &mut te) != 0;
            }
            CloseHandle(snap);
            if resumed == 0 {
                return Err("could not resume the started process".into());
            }
            Ok(())
        }
    }
}

impl Tree {
    /// Takes ownership of the started child's tree; on Windows puts the (suspended) child in a
    /// job object and resumes it. An error means the child could not be controlled.
    fn attach(child: &Child) -> Result<Tree, String> {
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            let job = win::job_for(child.as_raw_handle() as _)?;
            let tree = Tree { job, pid: child.id() };
            win::resume(child.id())?; // on error `tree` is dropped: closing the job kills the child
            Ok(tree)
        }
        #[cfg(not(windows))]
        Ok(Tree { pid: child.id() })
    }

    /// Sends the kill to every process of the tree; returns at once (termination is
    /// asynchronous; [`shutdown`] waits for it).
    fn terminate(&self) {
        #[cfg(unix)]
        if let Ok(pid) = i32::try_from(self.pid) {
            // SAFETY: plain syscall; a negative pid addresses the child's process group.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
        #[cfg(windows)]
        if !self.job.is_null() {
            // SAFETY: `job` is a job object handle we created and still own.
            unsafe {
                windows_sys::Win32::System::JobObjects::TerminateJobObject(self.job, 1);
            }
        }
        let _ = self.pid;
    }

    /// Live processes of the tree once the direct child has been reaped.
    fn live(&self) -> Result<u32, String> {
        #[cfg(unix)]
        {
            let pid = i32::try_from(self.pid).map_err(|e| e.to_string())?;
            // SAFETY: signal 0 only checks whether the process group still has members.
            let r = unsafe { libc::kill(-pid, 0) };
            if r == 0 {
                return Ok(1);
            }
            let err = std::io::Error::last_os_error();
            match err.raw_os_error() {
                Some(libc::ESRCH) => Ok(0),
                Some(libc::EPERM) => Ok(1),
                _ => Err(format!("cannot check the process group: {err}")),
            }
        }
        #[cfg(windows)]
        {
            win::active(self.job)
        }
        #[cfg(not(any(unix, windows)))]
        Ok(0)
    }
}

impl Drop for Tree {
    /// Never blocks: the processes were already waited for by [`shutdown`]; this only releases
    /// the handle (on Windows closing the kill-on-close job ends any straggler).
    fn drop(&mut self) {
        #[cfg(windows)]
        if !self.job.is_null() {
            // SAFETY: closing our own handle once.
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(self.job);
            }
        }
    }
}

/// How long the whole termination sequence (kill, child exit, descendants gone) may take.
pub const SHUTDOWN_LIMIT: Duration = Duration::from_secs(10);

/// Options of [`run_opts`].
#[derive(Clone, Debug)]
pub struct RunOptions {
    /// Wall-clock limit of the command.
    pub timeout: Duration,
    /// One deadline for the termination sequence at the end of every run.
    pub shutdown: Duration,
    /// Tests: pretend that processes of the tree stay alive this long after termination
    /// starts (simulates a descendant that does not exit promptly). Test builds only.
    #[cfg(test)]
    pub(crate) simulate_stuck_tree: Option<Duration>,
}

impl RunOptions {
    pub fn new(timeout: Duration) -> Self {
        RunOptions {
            timeout,
            shutdown: SHUTDOWN_LIMIT,
            #[cfg(test)]
            simulate_stuck_tree: None,
        }
    }
}

/// The termination state machine, run at the end of every [`run_opts`]: terminate the whole
/// tree, then wait — with ONE deadline for everything — until the direct child is reaped and
/// no process of the tree is left. Never blocks past `deadline`; an error means processes may
/// still be alive (their output must not be published).
fn shutdown(tree: &Tree, child: &mut Child, opts: &RunOptions, deadline: Instant) -> Result<ExitStatus, String> {
    let started = Instant::now();
    tree.terminate();
    let mut status = None;
    loop {
        if status.is_none() {
            match child.try_wait() {
                Ok(Some(s)) => status = Some(s),
                Ok(None) => {}
                Err(e) => return Err(format!("cannot wait for the process: {e}")),
            }
        }
        let live = match status {
            None => 1,
            Some(_) => {
                #[cfg(test)]
                let stuck = opts.simulate_stuck_tree.is_some_and(|d| started.elapsed() < d);
                #[cfg(not(test))]
                let stuck = {
                    let _ = opts;
                    false
                };
                if stuck { 1 } else { tree.live()? }
            }
        };
        if let (Some(s), 0) = (status, live) {
            return Ok(s);
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "processes of the command were still running {:.1} s after termination",
                started.elapsed().as_secs_f32()
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Runs `exe args…` (no shell) with `input` on stdin and returns stdout. Cancellation and the
/// timeout kill the whole process tree.
pub fn run(
    exe: &Path,
    args: &[&str],
    cwd: Option<&Path>,
    input: &[u8],
    timeout: Duration,
    control: &JobControl,
) -> Result<Vec<u8>, ProcError> {
    run_opts(exe, args, cwd, input, &RunOptions::new(timeout), control)
}

/// [`run`] with explicit [`RunOptions`].
///
/// States: *running* (until exit, cancellation or timeout) → *collecting* output (bounded) →
/// *shutdown* (always; terminate the tree, wait with one deadline) → result. Success is
/// returned only when the command succeeded, the export was not cancelled and no process of
/// the tree is alive any more; a shutdown failure is [`ProcError::Shutdown`].
pub fn run_opts(
    exe: &Path,
    args: &[&str],
    cwd: Option<&Path>,
    input: &[u8],
    opts: &RunOptions,
    control: &JobControl,
) -> Result<Vec<u8>, ProcError> {
    control.check().map_err(|_| ProcError::Cancelled)?;
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(d) = cwd {
        cmd.current_dir(d);
    }
    configure(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => ProcError::NotFound(exe.display().to_string()),
        _ => ProcError::Io(e.to_string()),
    })?;
    let tree = match Tree::attach(&child) {
        Ok(t) => t,
        Err(e) => {
            // not controllable: kill the direct child and wait for it (bounded)
            let _ = child.kill();
            let end = Instant::now() + opts.shutdown;
            while !matches!(child.try_wait(), Ok(Some(_)) | Err(_)) && Instant::now() < end {
                std::thread::sleep(Duration::from_millis(10));
            }
            return Err(ProcError::Io(format!("cannot control the started process: {e}")));
        }
    };
    let stdin = child.stdin.take();
    let input = input.to_vec();
    std::thread::spawn(move || {
        if let Some(mut s) = stdin {
            let _ = s.write_all(&input);
        }
    });
    let out_rx = child.stdout.take().map(|s| reader(s, MAX_STDOUT));
    let err_rx = child.stderr.take().map(|s| reader(s, MAX_STDERR));
    let start = Instant::now();
    // running
    let mut failure: Option<ProcError> = None;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(e) => {
                failure = Some(ProcError::Io(e.to_string()));
                break;
            }
        }
        if control.is_cancelled() {
            failure = Some(ProcError::Cancelled);
            break;
        }
        if start.elapsed() > opts.timeout {
            failure = Some(ProcError::Timeout(opts.timeout));
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    // collecting: the child exited; a descendant may still hold the pipes open — wait only
    // until the deadline (at least a short grace period)
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    if failure.is_none() {
        let deadline = (start + opts.timeout).max(Instant::now() + Duration::from_millis(500));
        let collect = |rx: Option<Receiver<Vec<u8>>>| -> Result<Vec<u8>, ProcError> {
            let Some(rx) = rx else { return Ok(Vec::new()) };
            loop {
                match rx.recv_timeout(Duration::from_millis(20)) {
                    Ok(v) => return Ok(v),
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Ok(Vec::new()),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                }
                if control.is_cancelled() {
                    return Err(ProcError::Cancelled);
                }
                if Instant::now() > deadline {
                    return Err(ProcError::Timeout(opts.timeout));
                }
            }
        };
        match collect(out_rx).and_then(|o| collect(err_rx).map(|e| (o, e))) {
            Ok((o, e)) => (stdout, stderr) = (o, e),
            Err(e) => failure = Some(e),
        }
    }
    // shutdown: always, with one deadline
    let status = shutdown(&tree, &mut child, opts, Instant::now() + opts.shutdown);
    drop(tree);
    let status = match (status, failure) {
        (Err(s), Some(f)) => return Err(ProcError::Shutdown(format!("{f}; {s}"))),
        (Err(s), None) => return Err(ProcError::Shutdown(s)),
        (Ok(_), Some(f)) => return Err(f),
        (Ok(st), None) => st,
    };
    if control.is_cancelled() {
        return Err(ProcError::Cancelled); // cancelled while the tree was being shut down
    }
    if status.success() {
        Ok(stdout)
    } else {
        Err(ProcError::Failed {
            code: status.code(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
        })
    }
}

/// Runs a command template: split into arguments, variables substituted per argument, the
/// program started directly (no shell). Returns stdout (stderr appended).
pub fn run_template(
    template: &str,
    vars: &Vars,
    cwd: Option<&Path>,
    timeout: Duration,
    control: &JobControl,
) -> Result<String, ProcError> {
    let argv = expand_argv(template, vars).map_err(ProcError::Io)?;
    let (exe, rest) = argv
        .split_first()
        .ok_or_else(|| ProcError::Io("empty command".into()))?;
    let rest: Vec<&str> = rest.iter().map(String::as_str).collect();
    let out = run(Path::new(exe), &rest, cwd, b"", timeout, control)?;
    Ok(String::from_utf8_lossy(&out).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitting() {
        assert_eq!(split_command("a b  c").unwrap(), ["a", "b", "c"]);
        assert_eq!(
            split_command("cp '${a} x' \"y \\\"z\\\"\"").unwrap(),
            ["cp", "${a} x", "y \"z\""]
        );
        assert_eq!(split_command("a ''").unwrap(), ["a", ""]);
        assert!(split_command("a 'b").is_err());
        assert!(split_command("   ").is_err());
    }

    fn hostile() -> Vars {
        let mut v = Vars::new();
        v.set(
            "title",
            "$(printf INJECTED) `printf INJECTED` \"; printf INJECTED; ' %PATH% & echo X",
        );
        v
    }

    #[test]
    fn values_stay_one_argument() {
        let v = hostile();
        let argv = expand_argv("echo \"${title}\" '${title}' ${title}", &v).unwrap();
        assert_eq!(argv.len(), 4);
        for a in &argv[1..] {
            assert_eq!(a, &v.get("title").unwrap());
        }
    }

    #[cfg(unix)]
    #[test]
    fn no_shell_interpretation() {
        let v = hostile();
        for tpl in ["echo ${title}", "echo \"${title}\"", "echo '${title}'"] {
            let out = run_template(tpl, &v, None, Duration::from_secs(10), &JobControl::new()).unwrap();
            assert_eq!(out.trim_end(), v.get("title").unwrap(), "{tpl}");
        }
        // a template that names a shell runs it only on the user's literal script; the value
        // is a positional argument, not script text
        let out = run_template(
            "/bin/sh -c 'printf \"%s\" \"$1\"' sh ${title}",
            &v,
            None,
            Duration::from_secs(10),
            &JobControl::new(),
        )
        .unwrap();
        assert_eq!(out, v.get("title").unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn cancel_and_timeout_kill_the_tree() {
        // the shell starts a background sleeper that keeps the pipes open
        let c = JobControl::new();
        let c2 = c.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            c2.cancel();
        });
        let t = Instant::now();
        let e = run(
            Path::new("/bin/sh"),
            &["-c", "sleep 30 & sleep 30"],
            None,
            b"",
            Duration::from_secs(60),
            &c,
        )
        .unwrap_err();
        assert_eq!(e, ProcError::Cancelled);
        assert!(t.elapsed() < Duration::from_secs(5));
        // the parent exits at once but a descendant holds stdout: bounded by the timeout
        let t = Instant::now();
        let e = run(
            Path::new("/bin/sh"),
            &["-c", "sleep 30 &"],
            None,
            b"",
            Duration::from_millis(300),
            &JobControl::new(),
        )
        .unwrap_err();
        assert!(matches!(e, ProcError::Timeout(_)), "{e:?}");
        assert!(t.elapsed() < Duration::from_secs(5));
    }

    fn quick() -> (&'static str, Vec<&'static str>) {
        if cfg!(windows) {
            ("cmd", vec!["/C", "exit 0"])
        } else {
            ("/bin/sh", vec!["-c", "exit 0"])
        }
    }

    // Codex round 5: one bounded deadline for the whole shutdown; a tree that does not empty
    // is a distinct error, never success.
    #[test]
    fn stuck_tree_is_a_shutdown_error_within_the_deadline() {
        let (exe, args) = quick();
        let opts = RunOptions {
            timeout: Duration::from_secs(30),
            shutdown: Duration::from_millis(300),
            simulate_stuck_tree: Some(Duration::from_secs(60)),
        };
        let t = Instant::now();
        let e = run_opts(Path::new(exe), &args, None, b"", &opts, &JobControl::new()).unwrap_err();
        assert!(matches!(e, ProcError::Shutdown(_)), "{e:?}");
        assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
        // a cancelled run whose tree does not empty reports both
        let c = JobControl::new();
        c.cancel();
        let e = run_opts(Path::new(exe), &args, None, b"", &opts, &c).unwrap_err();
        assert_eq!(e, ProcError::Cancelled, "cancelled before start: nothing ran");
    }

    #[test]
    fn cancel_during_successful_exit_drain_is_cancelled() {
        let (exe, args) = quick();
        let opts = RunOptions {
            timeout: Duration::from_secs(30),
            shutdown: Duration::from_secs(10),
            simulate_stuck_tree: Some(Duration::from_millis(800)),
        };
        let c = JobControl::new();
        let c2 = c.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            c2.cancel();
        });
        let t = Instant::now();
        let e = run_opts(Path::new(exe), &args, None, b"", &opts, &c).unwrap_err();
        assert_eq!(e, ProcError::Cancelled);
        assert!(
            t.elapsed() >= Duration::from_millis(700),
            "waited for the tree to empty"
        );
    }

    /// Size of the heartbeat file a descendant appends to, after it should have been killed.
    fn heartbeat_stops(hb: &Path) -> bool {
        std::thread::sleep(Duration::from_millis(500));
        let a = std::fs::metadata(hb).map(|m| m.len()).unwrap_or(0);
        std::thread::sleep(Duration::from_millis(2500));
        let b = std::fs::metadata(hb).map(|m| m.len()).unwrap_or(0);
        a == b
    }

    // Codex round 3: descendants are really terminated (they stop writing a heartbeat).
    #[cfg(unix)]
    #[test]
    fn unix_descendants_die_on_cancel() {
        let d = tempfile::tempdir().unwrap();
        let hb = d.path().join("hb");
        let script = format!("(while :; do echo x >> '{}'; sleep 0.1; done) & sleep 30", hb.display());
        let c = JobControl::new();
        let c2 = c.clone();
        let hb2 = hb.clone();
        std::thread::spawn(move || {
            // cancel once the descendant demonstrably runs (robust under machine load)
            let t = Instant::now();
            while std::fs::metadata(&hb2).map(|m| m.len()).unwrap_or(0) == 0 && t.elapsed() < Duration::from_secs(20) {
                std::thread::sleep(Duration::from_millis(20));
            }
            c2.cancel();
        });
        let e = run(
            Path::new("/bin/sh"),
            &["-c", &script],
            None,
            b"",
            Duration::from_secs(60),
            &c,
        )
        .unwrap_err();
        assert_eq!(e, ProcError::Cancelled);
        assert!(
            std::fs::metadata(&hb).map(|m| m.len()).unwrap_or(0) > 0,
            "descendant ran"
        );
        assert!(heartbeat_stops(&hb), "descendant survived cancellation");
    }

    #[cfg(windows)]
    #[test]
    fn windows_cancel_and_timeout_kill_the_tree() {
        let d = tempfile::tempdir().unwrap();
        let hb = d.path().join("hb.txt");
        std::fs::write(
            d.path().join("loop.cmd"),
            format!(
                "@echo off\r\n:a\r\necho x>>\"{}\"\r\nping -n 2 127.0.0.1 >nul\r\ngoto a\r\n",
                hb.display()
            ),
        )
        .unwrap();
        std::fs::write(
            d.path().join("main.cmd"),
            "@echo off\r\nstart /b cmd /c \"%~dp0loop.cmd\"\r\nping -n 30 127.0.0.1 >nul\r\n",
        )
        .unwrap();
        let main = d.path().join("main.cmd");
        let main = main.to_string_lossy().into_owned();
        let c = JobControl::new();
        let c2 = c.clone();
        let hb2 = hb.clone();
        std::thread::spawn(move || {
            let t = Instant::now();
            while std::fs::metadata(&hb2).map(|m| m.len()).unwrap_or(0) == 0 && t.elapsed() < Duration::from_secs(20) {
                std::thread::sleep(Duration::from_millis(20));
            }
            std::thread::sleep(Duration::from_millis(200));
            c2.cancel();
        });
        let t = Instant::now();
        let e = run(Path::new("cmd"), &["/C", &main], None, b"", Duration::from_secs(60), &c).unwrap_err();
        assert_eq!(e, ProcError::Cancelled);
        assert!(t.elapsed() < Duration::from_secs(30));
        assert!(
            std::fs::metadata(&hb).map(|m| m.len()).unwrap_or(0) > 0,
            "descendant ran"
        );
        assert!(heartbeat_stops(&hb), "descendant survived cancellation");
        // the launcher exits at once; a background descendant keeps stdout open
        let t = Instant::now();
        let e = run(
            Path::new("cmd"),
            &["/C", "start /b ping -n 30 127.0.0.1"],
            None,
            b"",
            Duration::from_millis(500),
            &JobControl::new(),
        )
        .unwrap_err();
        assert!(matches!(e, ProcError::Timeout(_)), "{e:?}");
        assert!(t.elapsed() < Duration::from_secs(10));
    }

    #[cfg(unix)]
    #[test]
    fn stderr_is_bounded() {
        let e = run(
            Path::new("/bin/sh"),
            &["-c", "head -c 1000000 /dev/zero | tr '\\0' x >&2; exit 2"],
            None,
            b"",
            Duration::from_secs(20),
            &JobControl::new(),
        )
        .unwrap_err();
        match e {
            ProcError::Failed { code, stderr } => {
                assert_eq!(code, Some(2));
                assert_eq!(stderr.len(), MAX_STDERR);
            }
            other => panic!("{other:?}"),
        }
    }
}

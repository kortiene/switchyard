//! Trusted supervisor and in-sandbox file helper. The supervisor is a separate
//! process so host SIGKILL closes its control pipe and `cgroup.kill` still runs.
use std::fs::{self, File};
use std::io::{BufRead, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use sha2::{Digest, Sha256};

use super::cgroup::{self, CgroupIdentity};
use super::{EXEC_OUTPUT_CAP, MAX_FILE_BYTES, VERIFIER_OUTPUT_CAP};

pub fn helper_main() {
    if let Err(error) = dispatch() {
        eprintln!("switchyard-execution-helper: {error}");
        std::process::exit(1);
    }
}

fn dispatch() -> Result<(), String> {
    if std::env::var_os("SWITCHYARD_SANDBOX").is_some() {
        let mode = std::env::args().nth(1).unwrap_or_default();
        if mode == "relay-wait" {
            return relay_wait();
        }
        if mode != "file" && mode != "probe" {
            return Err("sandbox helper refuses supervise and join".into());
        }
    }
    match std::env::args().nth(1).as_deref() {
        Some("supervise") => supervise(),
        Some("join") => join(),
        Some("file") => file_op(),
        Some("probe") => probe(),
        Some("relay-wait") => relay_wait(),
        _ => Err("usage: switchyard-execution-helper supervise|join|file|probe|relay-wait".into()),
    }
}

/// Run the script and write its wait status to an `O_CLOEXEC` fd.
/// The script does not inherit that fd, and stderr is not a wait channel.
fn relay_wait() -> Result<(), String> {
    use std::os::fd::AsFd;
    use std::os::unix::process::ExitStatusExt;
    rustix::process::set_dumpable_behavior(rustix::process::DumpableBehavior::NotDumpable)
        .map_err(|error| format!("relay-wait dumpable: {error}"))?;
    let mut args = std::env::args().skip(2);
    let wait_fd = match (args.next().as_deref(), args.next()) {
        (Some("--wait-fd"), Some(value)) => value
            .parse::<i32>()
            .map_err(|_| "relay-wait fd is not an integer".to_string())?,
        _ => return Err("relay-wait requires --wait-fd".into()),
    };
    if args.next().as_deref() != Some("--") {
        return Err("relay-wait requires --".into());
    }
    let argv: Vec<String> = args.collect();
    let Some(program) = argv.first() else {
        return Err("relay-wait command is empty".into());
    };
    let mut record = std::fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_CLOEXEC)
        .open(format!("/proc/self/fd/{wait_fd}"))
        .map_err(|error| format!("relay-wait status fd: {error}"))?;
    rustix::io::fcntl_setfd(record.as_fd(), rustix::io::FdFlags::CLOEXEC)
        .map_err(|error| format!("relay-wait cloexec: {error}"))?;
    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg(format!("exec {wait_fd}>&-; exec \"$@\""))
        .arg("relay-script")
        .arg(program)
        .args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("relay-wait spawn: {error}"))?;
    let mut child_out = child.stdout.take().ok_or("relay-wait missing stdout")?;
    let mut child_err = child.stderr.take().ok_or("relay-wait missing stderr")?;
    let out_thread = thread::spawn(move || {
        let mut stdout = std::io::stdout();
        let _ = std::io::copy(&mut child_out, &mut stdout);
        let _ = stdout.flush();
    });
    let err_thread = thread::spawn(move || {
        let mut stderr = std::io::stderr();
        let _ = std::io::copy(&mut child_err, &mut stderr);
        let _ = stderr.flush();
    });
    let status = child
        .wait()
        .map_err(|error| format!("relay-wait wait: {error}"))?;
    let _ = out_thread.join();
    let _ = err_thread.join();
    let signaled = status.signal().is_some();
    let signal = status.signal().unwrap_or(0);
    let exited = status.code().is_some();
    let code = status.code().unwrap_or(0);
    writeln!(
        record,
        "signaled={} signal={:03} exited={} code={:04}",
        u8::from(signaled),
        signal,
        u8::from(exited),
        code
    )
    .map_err(|error| format!("relay-wait record: {error}"))?;
    record
        .flush()
        .map_err(|error| format!("relay-wait record: {error}"))?;
    Ok(())
}

fn join() -> Result<(), String> {
    // The supervisor places this pid in the leaf. This process must not migrate
    // itself: a late self-join could enter after an empty-cgroup proof.
    let _ = rustix::process::set_parent_process_death_signal(Some(rustix::process::Signal::KILL));
    let mut args = std::env::args().skip(2);
    let cgroup = match (args.next().as_deref(), args.next()) {
        (Some("--cgroup"), Some(path)) => PathBuf::from(path),
        _ => return Err("join requires --cgroup".into()),
    };
    if args.next().as_deref() != Some("--") {
        return Err("join requires -- before the command".into());
    }
    let argv: Vec<String> = args.collect();
    if argv.is_empty() {
        return Err("join command is empty".into());
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let inside = process_in_cgroup(&cgroup)?;
        if inside {
            break;
        }
        if parent_is_gone() {
            return Err("supervisor exited before the launcher entered the leaf".into());
        }
        if Instant::now() >= deadline {
            return Err("launcher was not placed in the recorded cgroup".into());
        }
        thread::sleep(Duration::from_millis(5));
    }
    use std::os::unix::process::CommandExt;
    let error = Command::new(&argv[0]).args(&argv[1..]).exec();
    Err(format!("exec {} failed: {error}", argv[0]))
}

fn parent_is_gone() -> bool {
    rustix::process::getppid()
        .map(|pid| pid.as_raw_nonzero().get() == 1)
        .unwrap_or(true)
}

fn process_in_cgroup(cgroup: &Path) -> Result<bool, String> {
    let procs = fs::read_to_string(cgroup.join("cgroup.procs"))
        .map_err(|error| format!("cannot read cgroup.procs: {error}"))?;
    let pid = std::process::id().to_string();
    Ok(procs.split_whitespace().any(|entry| entry == pid))
}

fn place_launcher(child: &std::process::Child, cgroup: &Path) -> Result<(), String> {
    let pid = child.id().to_string();
    fs::write(cgroup.join("cgroup.procs"), &pid)
        .map_err(|error| format!("cannot place launcher in cgroup: {error}"))?;
    let procs = fs::read_to_string(cgroup.join("cgroup.procs"))
        .map_err(|error| format!("cannot read cgroup.procs: {error}"))?;
    if !procs.split_whitespace().any(|entry| entry == pid) {
        return Err("launcher did not land in the recorded cgroup".into());
    }
    Ok(())
}

/// Kill and reap the launcher before anyone trusts `populated 0`. A dead
/// launcher cannot join afterwards, and one already in the leaf is then
/// covered by `cgroup.kill`.
fn reap_launcher(child: &mut std::process::Child) -> Result<std::process::ExitStatus, String> {
    if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
        return Ok(status);
    }
    let _ = child.kill();
    child.wait().map_err(|error| error.to_string())
}

struct SuperviseOpts {
    cgroup: PathBuf,
    bwrap: PathBuf,
    workspace: PathBuf,
    marker: PathBuf,
    status: PathBuf,
    mode: String,
    deadline_ms: i64,
    helper: PathBuf,
    request: PathBuf,
    result: PathBuf,
    argv: Vec<String>,
    fail_launch: bool,
    cargo_home: Option<PathBuf>,
    rustup_home: Option<PathBuf>,
    toolchain: Option<PathBuf>,
    target_dir: Option<PathBuf>,
    cpu_seconds: Option<u64>,
}

fn supervise() -> Result<(), String> {
    let opts = parse_supervise()?;
    require_bwrap(&opts.bwrap)?;
    let identity = cgroup_identity(&opts.cgroup)?;
    println!("READY");
    let _ = std::io::stdout().flush();
    let go = read_control(deadline_instant(opts.deadline_ms)?)?;
    if go != "GO" {
        let quiescence = cgroup::kill_and_prove(&identity, Duration::from_secs(2))
            .map_err(|error| error.to_string())?;
        write_result(&opts.result, &cancelled_result("no-go", &quiescence.events))?;
        println!("DONE");
        return Ok(());
    }
    let (mut child, mut host_binds) = match spawn_confined(&opts) {
        Ok(spawned) => spawned,
        Err(error) => {
            let quiescence = cgroup::kill_and_prove(&identity, Duration::from_secs(2))
                .map_err(|error| error.to_string())?;
            write_result(
                &opts.result,
                &launch_result("not_applied", false, false, &error, &quiescence),
            )?;
            println!("DONE");
            return Ok(());
        }
    };
    if let Err(error) = place_launcher(&child, &opts.cgroup) {
        let _ = reap_launcher(&mut child);
        let quiescence = cgroup::kill_and_prove(&identity, Duration::from_secs(2))
            .map_err(|error| error.to_string())?;
        write_result(
            &opts.result,
            &launch_result("not_applied", false, false, &error, &quiescence),
        )?;
        println!("DONE");
        return Ok(());
    }
    let stdout = child.stdout.take().ok_or("missing stdout pipe")?;
    let stderr = child.stderr.take().ok_or("missing stderr pipe")?;
    let stdout_truncated = Arc::new(AtomicBool::new(false));
    let stderr_truncated = Arc::new(AtomicBool::new(false));
    let stdout_buf = Arc::new(Mutex::new(Vec::new()));
    let stderr_buf = Arc::new(Mutex::new(Vec::new()));
    let cap = match opts.mode.as_str() {
        "exec" => EXEC_OUTPUT_CAP,
        "verifier" => VERIFIER_OUTPUT_CAP,
        // serde_json turns one byte into `\u00XX` (6 bytes), plus the envelope.
        _ => MAX_FILE_BYTES * 6 + 8 * 1024,
    };
    let stdout_hash = Arc::new(Mutex::new(String::new()));
    let stderr_hash = Arc::new(Mutex::new(String::new()));
    let stdout_flag = Arc::clone(&stdout_truncated);
    let stdout_out = Arc::clone(&stdout_buf);
    let stdout_digest = Arc::clone(&stdout_hash);
    let verifier_stdout = opts.mode == "verifier";
    let stdout_thread = thread::spawn(move || {
        if verifier_stdout {
            drain_hashed(stdout, cap, &stdout_flag, &stdout_out, &stdout_digest);
        } else {
            drain(stdout, cap, &stdout_flag, &stdout_out);
        }
    });
    let stderr_flag = Arc::clone(&stderr_truncated);
    let stderr_out = Arc::clone(&stderr_buf);
    let stderr_digest = Arc::clone(&stderr_hash);
    let verifier_stderr = opts.mode == "verifier";
    let stderr_thread = thread::spawn(move || {
        if verifier_stderr {
            drain_hashed(stderr, cap, &stderr_flag, &stderr_out, &stderr_digest);
        } else {
            drain(stderr, cap, &stderr_flag, &stderr_out);
        }
    });
    let parent_dead = Arc::new(AtomicBool::new(false));
    let dead_flag = Arc::clone(&parent_dead);
    thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 16];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) | Err(_) => {
                    dead_flag.store(true, Ordering::Relaxed);
                    break;
                }
                Ok(_) => {}
            }
        }
    });
    let mut disposition = "exited".to_string();
    let deadline = deadline_instant(opts.deadline_ms)?;
    let mut announced_entry = false;
    let mut reaped = None;
    loop {
        if !announced_entry && child_pid_present(&opts.status) {
            println!("ENTERED");
            let _ = std::io::stdout().flush();
            announced_entry = true;
        }
        if stdout_truncated.load(Ordering::Relaxed) || stderr_truncated.load(Ordering::Relaxed) {
            disposition = "truncated".into();
        } else if limit_reason(&opts.cgroup).is_some() {
            disposition = "limit".into();
        } else if parent_dead.load(Ordering::Relaxed) {
            disposition = "cancelled".into();
        } else if Instant::now() >= deadline {
            disposition = "deadline".into();
        }
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            reaped = Some(status);
            break;
        }
        if stopping(&disposition) {
            // Reap before cgroup.kill so a launcher that has not joined yet
            // cannot enter after the empty proof.
            reaped = Some(reap_launcher(&mut child)?);
            break;
        }
        // STARTED means Bubblewrap has forked the sandbox (child-pid), not
        // merely that the launcher pid sits in the leaf.
        if opts.mode == "exec" && announced_entry {
            println!("STARTED");
            let _ = std::io::stdout().flush();
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    if opts.mode == "exec" && disposition == "exited" && reaped.is_none() {
        while reaped.is_none() {
            if stdout_truncated.load(Ordering::Relaxed) || stderr_truncated.load(Ordering::Relaxed)
            {
                disposition = "truncated".into();
            } else if limit_reason(&opts.cgroup).is_some() {
                disposition = "limit".into();
            } else if parent_dead.load(Ordering::Relaxed) {
                disposition = "cancelled".into();
            } else if Instant::now() >= deadline {
                disposition = "deadline".into();
            }
            if stopping(&disposition) {
                reaped = Some(reap_launcher(&mut child)?);
                break;
            }
            if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
                reaped = Some(status);
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
    let status = match reaped {
        Some(status) => status,
        None => reap_launcher(&mut child)?,
    };
    stdout_thread.join().map_err(|_| "stdout reader failed")?;
    stderr_thread.join().map_err(|_| "stderr reader failed")?;
    let quiescence = cgroup::kill_and_prove(&identity, Duration::from_secs(5))
        .map_err(|error| error.to_string())?;
    let stdout = String::from_utf8_lossy(&stdout_buf.lock().expect("stdout")).into_owned();
    let stderr = String::from_utf8_lossy(&stderr_buf.lock().expect("stderr")).into_owned();
    // Verifier stdout is the script stream. Parsing it into `helper` let a
    // failing script store a second copy of that stream and blow the result cap.
    let helper = if opts.mode == "exec" || opts.mode == "verifier" {
        Value::Null
    } else {
        serde_json::from_str(&stdout).unwrap_or(Value::Null)
    };
    let workload_entered = child_pid_present(&opts.status);
    if stdout_truncated.load(Ordering::Relaxed) || stderr_truncated.load(Ordering::Relaxed) {
        disposition = "truncated".into();
    } else if let Some(kind) = helper.get("disposition").and_then(Value::as_str) {
        if helper.get("ok").and_then(Value::as_bool) == Some(false) {
            disposition = match kind {
                "denied" => "denied",
                "conflict" => "conflict",
                _ => "runtime_failure",
            }
            .to_string();
        }
    } else if disposition == "exited" && !status.success() {
        disposition = "runtime_failure".into();
    }
    let settle = if opts.mode == "exec" || opts.mode == "verifier" {
        true
    } else {
        match helper.get("ok").and_then(Value::as_bool) {
            Some(true) => true,
            Some(false) => matches!(disposition.as_str(), "denied" | "conflict"),
            None => false,
        }
    };
    let mut result = json!({
        "disposition": disposition,
        "exitCode": status.code(),
        "stdout": stdout,
        "stderr": stderr,
        "stdoutTruncated": stdout_truncated.load(Ordering::Relaxed),
        "stderrTruncated": stderr_truncated.load(Ordering::Relaxed),
        "helper": helper,
        "workloadEntered": workload_entered,
        "settle": settle,
        "pidsMaxEvents": quiescence.pids_max,
        "oomKills": quiescence.oom_kill,
        "quiescenceProven": quiescence.proven,
        "quiescenceReason": quiescence.reason,
    });
    if opts.mode == "verifier" {
        let stdout_sha = stdout_hash
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default();
        let stderr_sha = stderr_hash
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default();
        if let Some(object) = result.as_object_mut() {
            let stdout_bytes = stdout_buf
                .lock()
                .map(|guard| guard.clone())
                .unwrap_or_default();
            let stderr_bytes = stderr_buf
                .lock()
                .map(|guard| guard.clone())
                .unwrap_or_default();
            object.insert("stdout".to_string(), json!(""));
            object.insert("stderr".to_string(), json!(""));
            object.insert(
                "stdoutB64".to_string(),
                json!(super::b64_encode(&stdout_bytes)),
            );
            object.insert(
                "stderrB64".to_string(),
                json!(super::b64_encode(&stderr_bytes)),
            );
            object.insert("stdoutSha256".to_string(), json!(stdout_sha));
            object.insert("stderrSha256".to_string(), json!(stderr_sha));
            object.insert("helper".to_string(), Value::Null);
            object.insert("etcBinds".to_string(), json!(host_binds.etc_present));
            if let Some(path) = host_binds.linker_missing.take() {
                object.insert("linkerMissing".to_string(), json!(path));
            }
            // The launcher's own exit must be clean. A signalled relay, or a
            // Bubblewrap status other than 0, drops the record.
            let wait = read_script_wait(status.code() == Some(0), host_binds.wait_read.take());
            if let Some(path) = host_binds.wait_path.take() {
                let _ = fs::remove_file(path);
            }
            if let Some(wait) = wait {
                object.insert("scriptStatus".to_string(), json!(true));
                object.insert("scriptSignaled".to_string(), json!(wait.signaled));
                object.insert("scriptSignal".to_string(), json!(wait.signal));
                object.insert("scriptExited".to_string(), json!(wait.exited));
                object.insert(
                    "scriptExit".to_string(),
                    if wait.exited {
                        json!(wait.code)
                    } else {
                        Value::Null
                    },
                );
            } else {
                object.insert("scriptStatus".to_string(), json!(false));
            }
        }
    }
    write_result(&opts.result, &result)?;
    if let Some(path) = host_binds.cargo_home_scratch.take() {
        let _ = fs::remove_dir_all(path);
    }
    println!("DONE");
    let _ = std::io::stdout().flush();
    Ok(())
}

fn stopping(disposition: &str) -> bool {
    matches!(
        disposition,
        "truncated" | "cancelled" | "deadline" | "limit"
    )
}

fn child_pid_present(path: &Path) -> bool {
    let Ok(text) = fs::read_to_string(path) else {
        return false;
    };
    // Bubblewrap appends a second object, `{ "exit-code": N }`, after the
    // child-pid record. Parse each line.
    text.lines().any(|line| {
        serde_json::from_str::<Value>(line.trim())
            .ok()
            .and_then(|value| value.get("child-pid").and_then(Value::as_u64))
            .is_some_and(|pid| pid > 0)
    })
}

fn launch_result(
    disposition: &str,
    workload_entered: bool,
    settle: bool,
    detail: &str,
    quiescence: &cgroup::Quiescence,
) -> Value {
    json!({
        "disposition": disposition,
        "exitCode": null,
        "stdout": "",
        "stderr": detail,
        "stdoutTruncated": false,
        "stderrTruncated": false,
        "helper": Value::Null,
        "workloadEntered": workload_entered,
        "settle": settle,
        "pidsMaxEvents": quiescence.pids_max,
        "oomKills": quiescence.oom_kill,
        "quiescenceProven": quiescence.proven,
        "quiescenceReason": quiescence.reason,
    })
}

fn spawn_sandbox(opts: &SuperviseOpts) -> Result<std::process::Child, String> {
    let mut command = Command::new(&opts.helper);
    command
        .arg("join")
        .arg("--cgroup")
        .arg(&opts.cgroup)
        .arg("--");
    command
        .arg("/bin/sh")
        .arg("-c")
        .arg("exec 3>\"$1\"; shift; exec \"$@\"");
    command
        .arg("sandbox")
        .arg(&opts.status)
        .arg("/usr/bin/prlimit");
    command.args([
        "--as=536870912:536870912",
        "--cpu=10:10",
        "--fsize=1048576:1048576",
        "--nofile=128:128",
        "--core=0:0",
        "--",
    ]);
    command.arg(&opts.bwrap);
    if opts.fail_launch {
        // Unknown flags make Bubblewrap exit before it writes child-pid.
        command.arg("--switchyard-fail-launch");
    }
    command.args([
        "--unshare-user",
        "--disable-userns",
        "--assert-userns-disabled",
        "--unshare-pid",
        "--unshare-net",
        "--unshare-ipc",
        "--unshare-uts",
        "--unshare-cgroup",
        "--cap-drop",
        "ALL",
        "--die-with-parent",
        "--new-session",
        "--json-status-fd",
        "3",
        "--clearenv",
        "--setenv",
        "PATH",
        "/usr/bin:/bin",
        "--setenv",
        "LANG",
        "C",
        "--setenv",
        "SWITCHYARD_SANDBOX",
        "1",
        "--ro-bind",
        "/usr",
        "/usr",
        "--symlink",
        "usr/bin",
        "/bin",
        "--symlink",
        "usr/lib",
        "/lib",
        "--symlink",
        "usr/lib64",
        "/lib64",
        "--proc",
        "/proc",
        "--dev",
        "/dev",
        "--size",
        "8388608",
        "--tmpfs",
        "/tmp",
        "--bind",
    ]);
    command.arg(&opts.workspace).arg("/workspace");
    command
        .arg("--ro-bind")
        .arg(&opts.marker)
        .arg("/workspace/.git");
    if opts.mode != "exec" {
        command.arg("--ro-bind").arg(&opts.helper).arg("/helper");
        command
            .arg("--ro-bind")
            .arg(&opts.request)
            .arg("/request.json");
    }
    command.args(["--chdir", "/workspace", "--remount-ro", "/", "--"]);
    match opts.mode.as_str() {
        "file" => command.args(["/helper", "file"]),
        "probe" => command.args(["/helper", "probe"]),
        "exec" => command.args(&opts.argv),
        _ => return Err("unknown sandbox mode".into()),
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot spawn sandbox launcher: {error}"))
}

/// Allowlisted `/etc` entries the verifier may bind. No wildcards.
pub(crate) const VERIFIER_ETC_ALLOWLIST: &[&str] = &[
    ETC_ALTERNATIVES,
    ETC_LD_SO_CACHE,
    ETC_LD_SO_CONF,
    ETC_LD_SO_CONF_D,
];

const ETC_ALTERNATIVES: &str = "/etc/alternatives";
const ETC_LD_SO_CACHE: &str = "/etc/ld.so.cache";
const ETC_LD_SO_CONF: &str = "/etc/ld.so.conf";
const ETC_LD_SO_CONF_D: &str = "/etc/ld.so.conf.d";

struct VerifierHostBinds {
    etc_present: Vec<String>,
    linker_missing: Option<String>,
    /// Writable empty `CARGO_HOME`. Removed after the sandbox exits.
    cargo_home_scratch: Option<PathBuf>,
    /// Read end of the wait fifo. `O_CLOEXEC`, so the sandbox never inherits it.
    wait_read: Option<File>,
    wait_path: Option<PathBuf>,
}

impl VerifierHostBinds {
    fn empty() -> Self {
        Self {
            etc_present: Vec::new(),
            linker_missing: None,
            cargo_home_scratch: None,
            wait_read: None,
            wait_path: None,
        }
    }
}

/// Allowlisted `/etc` paths that exist on `root` now. `--ro-bind-try` skips a
/// missing source without saying so, so the evidence records this list.
pub(crate) fn present_etc_binds(root: &Path) -> Vec<String> {
    VERIFIER_ETC_ALLOWLIST
        .iter()
        .filter(|path| root.join(path.trim_start_matches('/')).exists())
        .map(|path| (*path).to_string())
        .collect()
}

/// First sandbox path in the `cc` symlink chain that this profile cannot see.
///
/// `root` is the host directory that corresponds to the sandbox `/`. Production
/// passes `/`. A target outside the read-only `/usr` bind and the present
/// allowlist is returned as that path; the profile is not widened to cover it.
pub(crate) fn unresolved_sandbox_cc(
    root: &Path,
    toolchain: &Path,
    present_etc: &[String],
) -> Option<String> {
    let candidates = [
        PathBuf::from("/toolchain/bin/cc"),
        PathBuf::from("/usr/bin/cc"),
        PathBuf::from("/bin/cc"),
    ];
    let mut current = None;
    for candidate in &candidates {
        if fs::symlink_metadata(host_path(root, candidate, toolchain)).is_ok() {
            current = Some(candidate.clone());
            break;
        }
    }
    let mut current = current.unwrap_or_else(|| PathBuf::from("/usr/bin/cc"));
    if fs::symlink_metadata(host_path(root, &current, toolchain)).is_err() {
        return Some(current.display().to_string());
    }
    for _ in 0..32 {
        if !sandbox_visible(&current, present_etc) {
            return Some(current.display().to_string());
        }
        let host = host_path(root, &current, toolchain);
        let meta = match fs::symlink_metadata(&host) {
            Ok(meta) => meta,
            Err(_) => return Some(current.display().to_string()),
        };
        if !meta.file_type().is_symlink() {
            return None;
        }
        let target = match fs::read_link(&host) {
            Ok(target) => target,
            Err(_) => return Some(current.display().to_string()),
        };
        let next = if target.is_absolute() {
            target
        } else {
            current.parent().unwrap_or(Path::new("/")).join(target)
        };
        current = normalize_abs(&next);
        if fs::symlink_metadata(host_path(root, &current, toolchain)).is_err() {
            return Some(current.display().to_string());
        }
    }
    Some(current.display().to_string())
}

fn sandbox_visible(path: &Path, present_etc: &[String]) -> bool {
    let text = path.to_string_lossy();
    const ROOTS: &[&str] = &[
        "/usr",
        "/bin",
        "/lib",
        "/lib64",
        "/toolchain",
        "/tmp",
        "/proc",
        "/dev",
        "/workspace",
        "/cargo-home",
        "/rustup",
        "/cargo-target",
    ];
    if ROOTS.iter().any(|root| covers(root, &text)) {
        return true;
    }
    present_etc.iter().any(|root| covers(root, &text))
}

fn covers(root: &str, path: &str) -> bool {
    path == root || path.starts_with(&format!("{root}/"))
}

fn host_path(root: &Path, sandbox: &Path, toolchain: &Path) -> PathBuf {
    let logical = normalize_abs(sandbox);
    let text = logical.to_string_lossy();
    if let Some(rest) = text.strip_prefix("/toolchain") {
        let rel = rest.trim_start_matches('/');
        return if rel.is_empty() {
            toolchain.to_path_buf()
        } else {
            toolchain.join(rel)
        };
    }
    let mapped = if let Some(rest) = strip_sandbox_prefix(&text, "/bin") {
        PathBuf::from("/usr/bin").join(rest)
    } else if let Some(rest) = strip_sandbox_prefix(&text, "/lib64") {
        PathBuf::from("/usr/lib64").join(rest)
    } else if let Some(rest) = strip_sandbox_prefix(&text, "/lib") {
        PathBuf::from("/usr/lib").join(rest)
    } else {
        logical
    };
    if root == Path::new("/") {
        mapped
    } else {
        root.join(mapped.strip_prefix("/").unwrap_or(mapped.as_path()))
    }
}

fn strip_sandbox_prefix<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    path.strip_prefix(prefix)
        .map(|rest| rest.trim_start_matches('/'))
}

fn normalize_abs(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::RootDir => out.push("/"),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::Normal(part) => out.push(part),
            std::path::Component::Prefix(_) => {}
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from("/")
    } else {
        out
    }
}

fn spawn_confined(
    opts: &SuperviseOpts,
) -> Result<(std::process::Child, VerifierHostBinds), String> {
    if opts.mode == "verifier" {
        spawn_verifier_sandbox(opts)
    } else {
        spawn_sandbox(opts).map(|child| (child, VerifierHostBinds::empty()))
    }
}

/// Second profile, used only by the host verifier. The exec profile above is unchanged.
///
/// The worktree directory is bound read-only, so a tracked symlink stays a symlink.
/// The toolchain, `CARGO_HOME`, and `RUSTUP_HOME` are read-only. `CARGO_TARGET_DIR`
/// is a host directory outside the tree and is the only writable bind. `/usr` is
/// read-only. The linker paths below are the only `/etc` entries, each with
/// `--ro-bind-try`.
fn spawn_verifier_sandbox(
    opts: &SuperviseOpts,
) -> Result<(std::process::Child, VerifierHostBinds), String> {
    let cargo_home = opts
        .cargo_home
        .as_ref()
        .ok_or("verifier sandbox requires --cargo-home")?;
    let rustup_home = opts
        .rustup_home
        .as_ref()
        .ok_or("verifier sandbox requires --rustup-home")?;
    let toolchain = opts
        .toolchain
        .as_ref()
        .ok_or("verifier sandbox requires --toolchain")?;
    let target_dir = opts
        .target_dir
        .as_ref()
        .ok_or("verifier sandbox requires --target-dir")?;
    let cpu_seconds = opts
        .cpu_seconds
        .ok_or("verifier sandbox requires --cpu-seconds")?;
    let cpu = format!("--cpu={cpu_seconds}:{cpu_seconds}");
    let (wait_path, wait_read) = open_wait_fifo(&opts.status)?;
    let mut command = Command::new(&opts.helper);
    command
        .arg("join")
        .arg("--cgroup")
        .arg(&opts.cgroup)
        .arg("--");
    command
        .arg("/bin/sh")
        .arg("-c")
        .arg("exec 3>\"$1\"; exec 4>\"$2\"; shift 2; exec \"$@\"");
    command
        .arg("sandbox")
        .arg(&opts.status)
        .arg(&wait_path)
        .arg("/usr/bin/prlimit");
    command.args([
        "--fsize=2147483648:2147483648",
        cpu.as_str(),
        "--nofile=4096:4096",
        "--core=0:0",
        "--",
    ]);
    command.arg(&opts.bwrap);
    command.args([
        "--unshare-user",
        "--disable-userns",
        "--assert-userns-disabled",
        "--unshare-pid",
        "--unshare-net",
        "--unshare-ipc",
        "--unshare-uts",
        "--unshare-cgroup",
        "--cap-drop",
        "ALL",
        "--die-with-parent",
        "--new-session",
        "--json-status-fd",
        "3",
        "--clearenv",
        "--setenv",
        "PATH",
        "/toolchain/bin:/usr/bin:/bin",
        "--setenv",
        "LANG",
        "C",
        "--setenv",
        "CARGO_HOME",
        "/cargo-home",
        "--setenv",
        "RUSTUP_HOME",
        "/rustup",
        "--setenv",
        "CARGO_TARGET_DIR",
        "/cargo-target",
        "--setenv",
        "CARGO_NET_OFFLINE",
        "true",
        "--setenv",
        "SWITCHYARD_SANDBOX",
        "1",
        "--ro-bind",
        "/usr",
        "/usr",
        "--symlink",
        "usr/bin",
        "/bin",
        "--symlink",
        "usr/lib",
        "/lib",
        "--symlink",
        "usr/lib64",
        "/lib64",
        "--proc",
        "/proc",
        "--dev",
        "/dev",
        "--size",
        "1073741824",
        "--tmpfs",
        "/tmp",
    ]);
    let mut etc_present = present_etc_binds(Path::new("/"));
    if std::env::var_os("SWITCHYARD_OMIT_LINKER").is_some() {
        etc_present.retain(|path| path != ETC_ALTERNATIVES);
    }
    let linker_missing = unresolved_sandbox_cc(Path::new("/"), toolchain, &etc_present);
    // /etc/alternatives: /usr/bin/cc is an alternatives symlink.
    if etc_present.iter().any(|path| path == ETC_ALTERNATIVES) {
        command.args(["--ro-bind-try", ETC_ALTERNATIVES, ETC_ALTERNATIVES]);
    }
    // /etc/ld.so.cache: the dynamic linker reads this cache.
    command.args(["--ro-bind-try", ETC_LD_SO_CACHE, ETC_LD_SO_CACHE]);
    // /etc/ld.so.conf: names the directories the linker searches.
    command.args(["--ro-bind-try", ETC_LD_SO_CONF, ETC_LD_SO_CONF]);
    // /etc/ld.so.conf.d: holds the linker configuration fragments.
    command.args(["--ro-bind-try", ETC_LD_SO_CONF_D, ETC_LD_SO_CONF_D]);
    command.arg("--ro-bind").arg(toolchain).arg("/toolchain");
    // Empty CARGO_HOME. Host registry/ and git/ are the only cargo binds, read-only.
    let cargo_scratch = empty_cargo_home()?;
    command.arg("--bind").arg(&cargo_scratch).arg("/cargo-home");
    let registry = cargo_home.join("registry");
    if registry.is_dir() {
        command
            .arg("--ro-bind")
            .arg(&registry)
            .arg("/cargo-home/registry");
    }
    let git_cache = cargo_home.join("git");
    if git_cache.is_dir() {
        command
            .arg("--ro-bind")
            .arg(&git_cache)
            .arg("/cargo-home/git");
    }
    command.arg("--ro-bind").arg(rustup_home).arg("/rustup");
    command.arg("--bind").arg(target_dir).arg("/cargo-target");
    command.arg("--ro-bind").arg(&opts.helper).arg("/helper");
    command
        .arg("--ro-bind")
        .arg(&opts.workspace)
        .arg("/workspace");
    command
        .arg("--ro-bind")
        .arg(&opts.marker)
        .arg("/workspace/.git");
    command.args(["--chdir", "/workspace", "--remount-ro", "/", "--"]);
    if opts.argv.is_empty() {
        let _ = fs::remove_dir_all(&cargo_scratch);
        let _ = fs::remove_file(&wait_path);
        return Err("verifier sandbox command is empty".into());
    }
    command
        .arg("/helper")
        .arg("relay-wait")
        .arg("--wait-fd")
        .arg("4")
        .arg("--")
        .args(&opts.argv);
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            let _ = fs::remove_dir_all(&cargo_scratch);
            let _ = fs::remove_file(&wait_path);
            format!("cannot spawn verifier sandbox: {error}")
        })?;
    Ok((
        child,
        VerifierHostBinds {
            etc_present,
            linker_missing,
            cargo_home_scratch: Some(cargo_scratch),
            wait_read: Some(wait_read),
            wait_path: Some(wait_path),
        },
    ))
}

/// Host-side read end of the wait fifo. Opened before spawn so the launcher's
/// write open does not block, and `O_CLOEXEC` so the sandbox cannot inherit it.
fn open_wait_fifo(status: &Path) -> Result<(PathBuf, File), String> {
    let path = status.with_extension("wait");
    rustix::fs::mkfifoat(
        rustix::fs::CWD,
        &path,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )
    .map_err(|error| format!("wait fifo: {error}"))?;
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(&path)
        .map_err(|error| {
            let _ = fs::remove_file(&path);
            format!("wait fifo read: {error}")
        })?;
    use std::os::fd::AsFd;
    let flags = rustix::io::fcntl_getfd(file.as_fd()).map_err(|error| {
        let _ = fs::remove_file(&path);
        format!("wait fifo flags: {error}")
    })?;
    if !flags.contains(rustix::io::FdFlags::CLOEXEC) {
        let _ = fs::remove_file(&path);
        return Err("wait fifo read is inheritable".into());
    }
    Ok((path, file))
}

fn empty_cargo_home() -> Result<PathBuf, String> {
    let path = std::env::temp_dir().join(format!(
        "switchyard-cargo-home-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&path)
        .map_err(|error| format!("cannot create an empty CARGO_HOME: {error}"))?;
    Ok(path)
}

fn file_op() -> Result<(), String> {
    let request: Value = serde_json::from_reader(
        File::open("/request.json").map_err(|error| format!("request: {error}"))?,
    )
    .map_err(|error| format!("request json: {error}"))?;
    let workspace = open_workspace()?;
    let op = request["op"].as_str().unwrap_or("");
    let path = request["path"].as_str().unwrap_or(".");
    let result = match op {
        "read" => read_regular(&workspace, path, None),
        "range" => {
            let offset = request["offset"].as_u64().unwrap_or(0);
            let length = request["length"].as_u64().unwrap_or(0);
            read_regular(&workspace, path, Some((offset, length)))
        }
        "list" => list_directory(&workspace, path),
        "write" => write_regular(&workspace, path, request["content"].as_str().unwrap_or("")),
        "edit" => edit_regular(
            &workspace,
            path,
            request["old"].as_str().unwrap_or(""),
            request["new"].as_str().unwrap_or(""),
        ),
        _ => json!({"ok": false, "disposition": "denied", "reason": "unsupported"}),
    };
    println!(
        "{}",
        serde_json::to_string(&result).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn probe() -> Result<(), String> {
    let request: Value = serde_json::from_reader(
        File::open("/request.json").map_err(|error| format!("request: {error}"))?,
    )
    .map_err(|error| format!("request json: {error}"))?;
    let mut reads = Vec::new();
    for path in request["paths"].as_array().into_iter().flatten() {
        let Some(path) = path.as_str() else { continue };
        let read = fs::read(path).is_ok();
        let write = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .is_ok();
        reads.push(json!({"path": path, "read": read, "write": write}));
    }
    let tcp = request["tcp"].as_str().and_then(|address| {
        address.parse().ok().map(|address| {
            std::net::TcpStream::connect_timeout(&address, Duration::from_millis(300)).is_ok()
        })
    });
    let socket = request["socket"]
        .as_str()
        .map(|path| std::os::unix::net::UnixStream::connect(path).is_ok());
    let leaked: Vec<&str> = [
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "ANTHROPIC_API_KEY",
        "SSH_AUTH_SOCK",
        "HOME",
    ]
    .into_iter()
    .filter(|key| std::env::var_os(key).is_some())
    .collect();
    println!(
        "{}",
        serde_json::to_string(&json!({
            "ok": true,
            "disposition": "probe",
            "paths": reads,
            "tcp": tcp.unwrap_or(false),
            "socket": socket.unwrap_or(false),
            "leaked": leaked,
        }))
        .map_err(|error| error.to_string())?
    );
    Ok(())
}

fn open_workspace() -> Result<std::os::fd::OwnedFd, String> {
    use rustix::fs::{Mode, OFlags};
    rustix::fs::open(
        "/workspace",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|error| format!("open /workspace: {error}"))
}

fn resolve_flags() -> rustix::fs::ResolveFlags {
    use rustix::fs::ResolveFlags;
    ResolveFlags::BENEATH
        | ResolveFlags::NO_SYMLINKS
        | ResolveFlags::NO_MAGICLINKS
        | ResolveFlags::NO_XDEV
}

fn open_relative(
    workspace: &std::os::fd::OwnedFd,
    path: &str,
    flags: rustix::fs::OFlags,
    directory: bool,
) -> Result<File, Value> {
    use rustix::fs::{Mode, OFlags};
    if let Err(error) = super::validate_workspace_path(path, directory && path == ".") {
        return Err(
            json!({"ok": false, "disposition": "denied", "reason": "path", "detail": error.to_string()}),
        );
    }
    let mut flags = flags | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK;
    if directory {
        flags |= OFlags::DIRECTORY;
    }
    match rustix::fs::openat2(workspace, path, flags, Mode::empty(), resolve_flags()) {
        Ok(fd) => Ok(File::from(fd)),
        Err(error) => Err(json!({
            "ok": false,
            "disposition": "denied",
            "reason": "openat2",
            "detail": error.to_string()
        })),
    }
}

fn read_regular(workspace: &std::os::fd::OwnedFd, path: &str, range: Option<(u64, u64)>) -> Value {
    use rustix::fs::OFlags;
    let mut file = match open_relative(workspace, path, OFlags::RDONLY, false) {
        Ok(file) => file,
        Err(denied) => return denied,
    };
    if let Err(denied) = require_regular(&file) {
        return denied;
    }
    if let Some((offset, _)) = range
        && let Err(error) = std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(offset))
    {
        return json!({"ok": false, "disposition": "denied", "reason": "seek", "detail": error.to_string()});
    }
    let limit = range
        .map(|(_, length)| length as usize)
        .unwrap_or(MAX_FILE_BYTES + 1);
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        if buf.len() >= limit {
            break;
        }
        let want = (limit - buf.len()).min(chunk.len());
        match file.read(&mut chunk[..want]) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                return json!({"ok": false, "disposition": "runtime_failure", "detail": error.to_string()});
            }
        }
    }
    if range.is_none() && buf.len() > MAX_FILE_BYTES {
        return json!({"ok": false, "disposition": "denied", "reason": "too-large"});
    }
    let text = match String::from_utf8(buf) {
        Ok(text) => text,
        Err(_) => return json!({"ok": false, "disposition": "denied", "reason": "not-utf8"}),
    };
    json!({"ok": true, "disposition": "completed", "content": text, "bytes": text.len()})
}

fn write_regular(workspace: &std::os::fd::OwnedFd, path: &str, content: &str) -> Value {
    use rustix::fs::{AtFlags, FileType, Mode, OFlags};
    use std::os::fd::AsFd;
    if content.len() > MAX_FILE_BYTES {
        return json!({"ok": false, "disposition": "denied", "reason": "too-large"});
    }
    let (parent, name) = parent_and_name(path);
    let directory = match open_relative(workspace, parent, OFlags::RDONLY, true) {
        Ok(directory) => directory,
        Err(denied) => return denied,
    };
    let mode = match rustix::fs::statat(directory.as_fd(), name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => {
            if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
                return json!({"ok": false, "disposition": "denied", "reason": "not-regular"});
            }
            Mode::from_bits_truncate(stat.st_mode & 0o777)
        }
        Err(error) if error == rustix::io::Errno::NOENT => mode_644(),
        Err(error) => {
            return json!({"ok": false, "disposition": "denied", "reason": "stat", "detail": error.to_string()});
        }
    };
    let temp = format!(
        ".sy-tmp-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    );
    let flags = OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW;
    let mut file = match rustix::fs::openat2(directory.as_fd(), &temp, flags, mode, resolve_flags())
    {
        Ok(fd) => File::from(fd),
        Err(error) => {
            return json!({"ok": false, "disposition": "runtime_failure", "reason": "temp", "detail": error.to_string()});
        }
    };
    let cleanup = |directory: &File, temp: &str| {
        let _ = rustix::fs::unlinkat(directory.as_fd(), temp, AtFlags::empty());
    };
    if let Err(error) = file.write_all(content.as_bytes()) {
        cleanup(&directory, &temp);
        return json!({"ok": false, "disposition": "runtime_failure", "detail": error.to_string()});
    }
    if let Err(error) = rustix::fs::fchmod(file.as_fd(), mode) {
        cleanup(&directory, &temp);
        return json!({"ok": false, "disposition": "runtime_failure", "reason": "fchmod", "detail": error.to_string()});
    }
    if let Err(error) = rustix::fs::fsync(file.as_fd()) {
        cleanup(&directory, &temp);
        return json!({"ok": false, "disposition": "runtime_failure", "reason": "fsync", "detail": error.to_string()});
    }
    drop(file);
    if let Err(error) = rustix::fs::renameat(directory.as_fd(), &temp, directory.as_fd(), name) {
        cleanup(&directory, &temp);
        return json!({"ok": false, "disposition": "runtime_failure", "reason": "rename", "detail": error.to_string()});
    }
    if let Err(error) = rustix::fs::fsync(directory.as_fd()) {
        return json!({"ok": false, "disposition": "runtime_failure", "reason": "dirsync", "detail": error.to_string()});
    }
    json!({"ok": true, "disposition": "completed", "bytes": content.len(), "contentHash": hash(content.as_bytes())})
}

fn parent_and_name(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(index) => (&path[..index], &path[index + 1..]),
        None => (".", path),
    }
}

fn edit_regular(workspace: &std::os::fd::OwnedFd, path: &str, old: &str, new: &str) -> Value {
    let current = read_regular(workspace, path, None);
    if current.get("ok").and_then(Value::as_bool) != Some(true) {
        return current;
    }
    let text = current["content"].as_str().unwrap_or("");
    let count = text.matches(old).count();
    if count != 1 {
        return json!({
            "ok": false,
            "disposition": "conflict",
            "detail": format!("edit old string matched {count} times")
        });
    }
    write_regular(workspace, path, &text.replacen(old, new, 1))
}

fn list_directory(workspace: &std::os::fd::OwnedFd, path: &str) -> Value {
    use rustix::fs::OFlags;
    use std::os::fd::AsRawFd;
    let file = match open_relative(workspace, path, OFlags::RDONLY, true) {
        Ok(file) => file,
        Err(denied) => return denied,
    };
    let proc = format!("/proc/self/fd/{}", file.as_raw_fd());
    let entries = match fs::read_dir(&proc) {
        Ok(entries) => entries,
        Err(error) => {
            return json!({"ok": false, "disposition": "denied", "reason": "readdir", "detail": error.to_string()});
        }
    };
    let mut names = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else {
            return json!({"ok": false, "disposition": "runtime_failure", "reason": "readdir"});
        };
        names.push(entry.file_name().to_string_lossy().into_owned());
        if names.len() > 1024 {
            return json!({"ok": false, "disposition": "denied", "reason": "too-many-entries"});
        }
    }
    names.sort();
    json!({"ok": true, "disposition": "completed", "entries": names})
}

fn mode_644() -> rustix::fs::Mode {
    use rustix::fs::Mode;
    Mode::RUSR | Mode::WUSR | Mode::RGRP | Mode::ROTH
}

fn require_regular(file: &File) -> Result<(), Value> {
    use rustix::fs::FileType;
    use std::os::fd::AsFd;
    let stat = rustix::fs::fstat(file.as_fd()).map_err(|error| {
        json!({"ok": false, "disposition": "denied", "reason": "fstat", "detail": error.to_string()})
    })?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return Err(json!({"ok": false, "disposition": "denied", "reason": "not-regular"}));
    }
    Ok(())
}

fn hash(bytes: &[u8]) -> String {
    format!("sha256:{}", crate::canonical::sha256_hex(bytes))
}

struct ScriptWait {
    signaled: bool,
    signal: i32,
    exited: bool,
    code: i32,
}

/// One relay line, and only when the launcher itself exited 0.
fn read_script_wait(launcher_clean: bool, wait_read: Option<File>) -> Option<ScriptWait> {
    if !launcher_clean {
        return None;
    }
    let mut file = wait_read?;
    let mut bytes = Vec::new();
    let mut buf = [0u8; 256];
    loop {
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                bytes.extend_from_slice(&buf[..n]);
                if bytes.len() > 128 {
                    return None;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(_) => return None,
        }
    }
    parse_wait_record(&bytes)
}

fn parse_wait_record(bytes: &[u8]) -> Option<ScriptWait> {
    let text = std::str::from_utf8(bytes).ok()?;
    let text = text.strip_suffix('\n')?;
    if text.is_empty() || text.contains('\n') || text.contains("SYWAIT1") {
        return None;
    }
    let mut signaled = None;
    let mut signal = None;
    let mut exited = None;
    let mut code = None;
    for field in text.split_whitespace() {
        let (key, value) = field.split_once('=')?;
        match key {
            "signaled" => signaled = value.parse::<u8>().ok(),
            "signal" => signal = value.parse::<i32>().ok(),
            "exited" => exited = value.parse::<u8>().ok(),
            "code" => code = value.parse::<i32>().ok(),
            _ => return None,
        }
    }
    Some(ScriptWait {
        signaled: signaled? == 1,
        signal: signal?,
        exited: exited? == 1,
        code: code?,
    })
}

fn drain_hashed(
    mut reader: impl Read,
    cap: usize,
    truncated: &AtomicBool,
    out: &Mutex<Vec<u8>>,
    hash_out: &Mutex<String>,
) {
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    let mut total = 0usize;
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                hasher.update(&buf[..n]);
                push_tail(out, &buf[..n]);
                total += n;
                if total > cap {
                    truncated.store(true, Ordering::Relaxed);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => {
                truncated.store(true, Ordering::Relaxed);
                break;
            }
        }
    }
    if let Ok(mut slot) = hash_out.lock() {
        *slot = sha256_prefixed(hasher.finalize().as_slice());
    } else {
        truncated.store(true, Ordering::Relaxed);
    }
}

fn push_tail(out: &Mutex<Vec<u8>>, bytes: &[u8]) {
    let Ok(mut guard) = out.lock() else {
        return;
    };
    guard.extend_from_slice(bytes);
    if guard.len() > super::VERIFIER_TAIL_BYTES {
        let drop_n = guard.len() - super::VERIFIER_TAIL_BYTES;
        guard.drain(..drop_n);
    }
}

fn sha256_prefixed(digest: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::from("sha256:");
    for byte in digest {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    out
}

fn drain(mut reader: impl Read, cap: usize, truncated: &AtomicBool, out: &Mutex<Vec<u8>>) {
    let mut buf = [0u8; 8192];
    let mut total = 0usize;
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if total < cap {
                    let take = n.min(cap - total);
                    if let Ok(mut guard) = out.lock() {
                        guard.extend_from_slice(&buf[..take]);
                    }
                }
                total += n;
                if total > cap {
                    truncated.store(true, Ordering::Relaxed);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
}

fn parse_supervise() -> Result<SuperviseOpts, String> {
    let mut args = std::env::args().skip(2);
    let mut cgroup = None;
    let mut bwrap = None;
    let mut workspace = None;
    let mut marker = None;
    let mut status = None;
    let mut mode = None;
    let mut deadline_ms = None;
    let mut helper = None;
    let mut request = None;
    let mut result = None;
    let mut fail_launch = false;
    let mut cargo_home = None;
    let mut rustup_home = None;
    let mut toolchain = None;
    let mut target_dir = None;
    let mut cpu_seconds = None;
    while let Some(flag) = args.next() {
        if flag == "--" {
            break;
        }
        let value = args
            .next()
            .ok_or_else(|| format!("missing value for {flag}"))?;
        match flag.as_str() {
            "--cgroup" => cgroup = Some(PathBuf::from(value)),
            "--bwrap" => bwrap = Some(PathBuf::from(value)),
            "--workspace" => workspace = Some(PathBuf::from(value)),
            "--marker" => marker = Some(PathBuf::from(value)),
            "--status" => status = Some(PathBuf::from(value)),
            "--mode" => mode = Some(value),
            "--deadline-ms" => {
                deadline_ms = Some(
                    value
                        .parse::<i64>()
                        .map_err(|_| "deadline is not an integer")?,
                );
            }
            "--helper" => helper = Some(PathBuf::from(value)),
            "--request" => request = Some(PathBuf::from(value)),
            "--result" => result = Some(PathBuf::from(value)),
            "--fail-launch" => {
                fail_launch = value == "1" || value == "true";
            }
            "--cargo-home" => cargo_home = Some(PathBuf::from(value)),
            "--rustup-home" => rustup_home = Some(PathBuf::from(value)),
            "--toolchain" => toolchain = Some(PathBuf::from(value)),
            "--target-dir" => target_dir = Some(PathBuf::from(value)),
            "--cpu-seconds" => {
                cpu_seconds = Some(
                    value
                        .parse::<u64>()
                        .map_err(|_| "cpu-seconds is not an integer")?,
                );
            }
            _ => return Err(format!("unknown supervise flag {flag}")),
        }
    }
    Ok(SuperviseOpts {
        cgroup: cgroup.ok_or("missing --cgroup")?,
        bwrap: bwrap.ok_or("missing --bwrap")?,
        workspace: workspace.ok_or("missing --workspace")?,
        marker: marker.ok_or("missing --marker")?,
        status: status.ok_or("missing --status")?,
        mode: mode.ok_or("missing --mode")?,
        deadline_ms: deadline_ms.ok_or("missing --deadline-ms")?,
        helper: helper.ok_or("missing --helper")?,
        request: request.ok_or("missing --request")?,
        result: result.ok_or("missing --result")?,
        argv: args.collect(),
        fail_launch,
        cargo_home,
        rustup_home,
        toolchain,
        target_dir,
        cpu_seconds,
    })
}

fn require_bwrap(path: &Path) -> Result<(), String> {
    let output = Command::new(path)
        .arg("--version")
        .env_clear()
        .output()
        .map_err(|error| format!("bubblewrap: {error}"))?;
    if !output.status.success() || output.stdout != super::bubblewrap_version_line().as_bytes() {
        return Err(format!(
            "reviewed backend requires bubblewrap {}",
            super::BUBBLEWRAP_VERSION
        ));
    }
    Ok(())
}

fn cgroup_identity(path: &Path) -> Result<CgroupIdentity, String> {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::metadata(path).map_err(|error| format!("cgroup stat: {error}"))?;
    let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map_err(|error| format!("boot_id: {error}"))?;
    Ok(CgroupIdentity {
        path: fs::canonicalize(path).map_err(|error| error.to_string())?,
        dev: meta.dev(),
        ino: meta.ino(),
        boot_id: boot.trim().to_string(),
    })
}

fn limit_reason(path: &Path) -> Option<&'static str> {
    if event_value(&path.join("pids.events"), "max") > 0 {
        return Some("pids");
    }
    let memory = path.join("memory.events");
    if event_value(&memory, "oom_kill") > 0
        || event_value(&memory, "oom_group_kill") > 0
        || event_value(&memory, "oom") > 0
    {
        return Some("memory");
    }
    None
}

fn event_value(path: &Path, key: &str) -> u64 {
    let text = fs::read_to_string(path).unwrap_or_default();
    if key == "oom_kill" {
        return super::memory_events_oom_kill(&text);
    }
    text.lines()
        .find_map(|line| {
            let (name, value) = line.split_once(' ')?;
            (name == key).then(|| value.parse().ok()).flatten()
        })
        .unwrap_or(0)
}

fn read_control(deadline: Instant) -> Result<String, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let mut line = String::new();
        let result = std::io::stdin().lock().read_line(&mut line);
        let _ = tx.send(result.map(|_| line.trim().to_string()));
    });
    match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(Ok(line)) => Ok(line),
        Ok(Err(error)) => Err(format!("control pipe: {error}")),
        Err(_) => Ok(String::new()),
    }
}

fn deadline_instant(deadline_ms: i64) -> Result<Instant, String> {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_millis() as i64;
    let left = deadline_ms.saturating_sub(now_ms).max(0) as u64;
    Ok(Instant::now() + Duration::from_millis(left))
}

fn write_result(path: &Path, value: &Value) -> Result<(), String> {
    let text = serde_json::to_string(value).map_err(|error| error.to_string())?;
    fs::write(path, text).map_err(|error| format!("result file: {error}"))
}

fn cancelled_result(reason: &str, events: &str) -> Value {
    json!({
        "disposition": "cancelled",
        "reason": reason,
        "exitCode": null,
        "stdout": "",
        "stderr": "",
        "stdoutTruncated": false,
        "stderrTruncated": false,
        "helper": Value::Null,
        "quiescenceReason": events,
    })
}

#[cfg(all(test, unix))]
mod etc_bind_tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn scratch() -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "switchyard-etc-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn touch(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"x").unwrap();
    }

    #[test]
    fn allowlist_is_the_four_linker_paths_without_wildcards() {
        assert_eq!(
            VERIFIER_ETC_ALLOWLIST,
            [
                "/etc/alternatives",
                "/etc/ld.so.cache",
                "/etc/ld.so.conf",
                "/etc/ld.so.conf.d",
            ]
        );
        assert!(
            VERIFIER_ETC_ALLOWLIST
                .iter()
                .all(|path| !path.contains('*'))
        );
    }

    #[test]
    fn present_etc_binds_records_only_paths_that_exist() {
        let root = scratch();
        fs::create_dir_all(root.join("etc/alternatives")).unwrap();
        touch(&root.join("etc/ld.so.conf"));
        touch(&root.join("etc/passwd"));
        let present = present_etc_binds(&root);
        assert_eq!(
            present,
            vec![
                "/etc/alternatives".to_string(),
                "/etc/ld.so.conf".to_string()
            ]
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn unresolved_cc_names_the_path_the_profile_does_not_bind() {
        let root = scratch();
        let toolchain = root.join("toolchains/stable");
        fs::create_dir_all(toolchain.join("bin")).unwrap();
        fs::create_dir_all(root.join("usr/bin")).unwrap();
        fs::create_dir_all(root.join("etc/alternatives")).unwrap();
        symlink("/etc/alternatives/cc", root.join("usr/bin/cc")).unwrap();
        symlink("/usr/bin/gcc", root.join("etc/alternatives/cc")).unwrap();
        touch(&root.join("usr/bin/gcc"));
        assert_eq!(
            unresolved_sandbox_cc(&root, &toolchain, &[]).as_deref(),
            Some("/etc/alternatives/cc")
        );
        let present = vec!["/etc/alternatives".into()];
        assert_eq!(unresolved_sandbox_cc(&root, &toolchain, &present), None);
        fs::remove_file(root.join("etc/alternatives/cc")).unwrap();
        symlink("/opt/cc", root.join("etc/alternatives/cc")).unwrap();
        touch(&root.join("opt/cc"));
        assert_eq!(
            unresolved_sandbox_cc(&root, &toolchain, &present).as_deref(),
            Some("/opt/cc")
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn unresolved_cc_reports_usr_bin_cc_when_nothing_provides_it() {
        let root = scratch();
        let toolchain = root.join("toolchains/stable");
        fs::create_dir_all(&toolchain).unwrap();
        assert_eq!(
            unresolved_sandbox_cc(&root, &toolchain, &[]).as_deref(),
            Some("/usr/bin/cc")
        );
        let _ = fs::remove_dir_all(&root);
    }
}

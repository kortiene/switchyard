//! Trusted supervisor and in-sandbox file helper. The supervisor is a separate
//! process so host SIGKILL closes its control pipe and `cgroup.kill` still runs.
use std::fs::{self, File};
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use super::cgroup::{self, CgroupIdentity};
use super::{EXEC_OUTPUT_CAP, MAX_FILE_BYTES};

pub fn helper_main() {
    if let Err(error) = dispatch() {
        eprintln!("switchyard-execution-helper: {error}");
        std::process::exit(1);
    }
}

fn dispatch() -> Result<(), String> {
    if std::env::var_os("SWITCHYARD_SANDBOX").is_some() {
        let mode = std::env::args().nth(1).unwrap_or_default();
        if mode != "file" && mode != "probe" {
            return Err("sandbox helper refuses supervise and join".into());
        }
    }
    match std::env::args().nth(1).as_deref() {
        Some("supervise") => supervise(),
        Some("join") => join(),
        Some("file") => file_op(),
        Some("probe") => probe(),
        _ => Err("usage: switchyard-execution-helper supervise|join|file|probe".into()),
    }
}

fn join() -> Result<(), String> {
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
    fs::write(cgroup.join("cgroup.procs"), std::process::id().to_string())
        .map_err(|error| format!("cannot join cgroup: {error}"))?;
    let procs = fs::read_to_string(cgroup.join("cgroup.procs"))
        .map_err(|error| format!("cannot read cgroup.procs: {error}"))?;
    if !procs
        .split_whitespace()
        .any(|pid| pid == std::process::id().to_string())
    {
        return Err("process did not land in the recorded cgroup".into());
    }
    use std::os::unix::process::CommandExt;
    let error = Command::new(&argv[0]).args(&argv[1..]).exec();
    Err(format!("exec {} failed: {error}", argv[0]))
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
    let mut child = spawn_sandbox(&opts)?;
    let stdout = child.stdout.take().ok_or("missing stdout pipe")?;
    let stderr = child.stderr.take().ok_or("missing stderr pipe")?;
    let stdout_truncated = Arc::new(AtomicBool::new(false));
    let stderr_truncated = Arc::new(AtomicBool::new(false));
    let stdout_buf = Arc::new(Mutex::new(Vec::new()));
    let stderr_buf = Arc::new(Mutex::new(Vec::new()));
    let cap = if opts.mode == "exec" {
        EXEC_OUTPUT_CAP
    } else {
        MAX_FILE_BYTES * 4
    };
    let stdout_flag = Arc::clone(&stdout_truncated);
    let stdout_out = Arc::clone(&stdout_buf);
    let stdout_thread = thread::spawn(move || drain(stdout, cap, &stdout_flag, &stdout_out));
    let stderr_flag = Arc::clone(&stderr_truncated);
    let stderr_out = Arc::clone(&stderr_buf);
    let stderr_thread = thread::spawn(move || drain(stderr, cap, &stderr_flag, &stderr_out));
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
    let mut disposition = "exited";
    let deadline = deadline_instant(opts.deadline_ms)?;
    loop {
        if stdout_truncated.load(Ordering::Relaxed) || stderr_truncated.load(Ordering::Relaxed) {
            disposition = "truncated";
            let _ = cgroup::kill_and_prove(&identity, Duration::from_secs(2));
        }
        if limit_reason(&opts.cgroup).is_some() {
            disposition = "limit";
            let _ = cgroup::kill_and_prove(&identity, Duration::from_secs(2));
        }
        if parent_dead.load(Ordering::Relaxed) {
            disposition = "cancelled";
            let _ = cgroup::kill_and_prove(&identity, Duration::from_secs(2));
        }
        if Instant::now() >= deadline {
            disposition = "deadline";
            let _ = cgroup::kill_and_prove(&identity, Duration::from_secs(2));
        }
        if child
            .try_wait()
            .map_err(|error| error.to_string())?
            .is_some()
        {
            break;
        }
        if matches!(
            disposition,
            "truncated" | "cancelled" | "deadline" | "limit"
        ) {
            let _ = child.kill();
            break;
        }
        // Publish STARTED once the leaf contains the launcher. Exec callers
        // return a running handle only after this line.
        if opts.mode == "exec" && cgroup_populated(&opts.cgroup) {
            println!("STARTED");
            let _ = std::io::stdout().flush();
            // Only announce once.
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    if opts.mode == "exec" && disposition == "exited" {
        // Either STARTED was printed and we broke while the child still runs,
        // or the child already exited. Continue until it exits or a stop hits.
        while child
            .try_wait()
            .map_err(|error| error.to_string())?
            .is_none()
        {
            if stdout_truncated.load(Ordering::Relaxed) || stderr_truncated.load(Ordering::Relaxed)
            {
                disposition = "truncated";
                let _ = cgroup::kill_and_prove(&identity, Duration::from_secs(2));
                let _ = child.kill();
            }
            if limit_reason(&opts.cgroup).is_some() {
                disposition = "limit";
                let _ = cgroup::kill_and_prove(&identity, Duration::from_secs(2));
                let _ = child.kill();
            }
            if parent_dead.load(Ordering::Relaxed) {
                disposition = "cancelled";
                let _ = cgroup::kill_and_prove(&identity, Duration::from_secs(2));
                let _ = child.kill();
            }
            if Instant::now() >= deadline {
                disposition = "deadline";
                let _ = cgroup::kill_and_prove(&identity, Duration::from_secs(2));
                let _ = child.kill();
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
    let status = child.wait().map_err(|error| error.to_string())?;
    stdout_thread.join().map_err(|_| "stdout reader failed")?;
    stderr_thread.join().map_err(|_| "stderr reader failed")?;
    let quiescence = cgroup::kill_and_prove(&identity, Duration::from_secs(5))
        .map_err(|error| error.to_string())?;
    let stdout = String::from_utf8_lossy(&stdout_buf.lock().expect("stdout")).into_owned();
    let stderr = String::from_utf8_lossy(&stderr_buf.lock().expect("stderr")).into_owned();
    let helper = if opts.mode == "exec" {
        Value::Null
    } else {
        serde_json::from_str(&stdout).unwrap_or(Value::Null)
    };
    if helper.get("disposition").and_then(Value::as_str) == Some("denied") {
        disposition = "denied";
    } else if helper.get("disposition").and_then(Value::as_str) == Some("conflict") {
        disposition = "conflict";
    } else if disposition == "exited" && !status.success() {
        disposition = "runtime_failure";
    }
    let result = json!({
        "disposition": disposition,
        "exitCode": status.code(),
        "stdout": stdout,
        "stderr": stderr,
        "stdoutTruncated": stdout_truncated.load(Ordering::Relaxed),
        "stderrTruncated": stderr_truncated.load(Ordering::Relaxed),
        "helper": helper,
        "pidsMaxEvents": quiescence.pids_max,
        "oomKills": quiescence.oom_kill,
        "quiescenceProven": quiescence.proven,
        "quiescenceReason": quiescence.reason,
    });
    write_result(&opts.result, &result)?;
    println!("DONE");
    let _ = std::io::stdout().flush();
    Ok(())
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
    use rustix::fs::OFlags;
    if content.len() > MAX_FILE_BYTES {
        return json!({"ok": false, "disposition": "denied", "reason": "too-large"});
    }
    let flags = OFlags::WRONLY | OFlags::CREATE | OFlags::TRUNC;
    // openat2 mode is applied only when creating. 0644 via the mode argument.
    let mut file = match open_relative_mode(workspace, path, flags, mode_644()) {
        Ok(file) => file,
        Err(denied) => return denied,
    };
    if let Err(denied) = require_regular(&file) {
        return denied;
    }
    if let Err(error) = file.write_all(content.as_bytes()) {
        return json!({"ok": false, "disposition": "runtime_failure", "detail": error.to_string()});
    }
    json!({"ok": true, "disposition": "completed", "bytes": content.len(), "contentHash": hash(content.as_bytes())})
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

fn open_relative_mode(
    workspace: &std::os::fd::OwnedFd,
    path: &str,
    flags: rustix::fs::OFlags,
    mode: rustix::fs::Mode,
) -> Result<File, Value> {
    use rustix::fs::OFlags;
    if let Err(error) = super::validate_workspace_path(path, false) {
        return Err(
            json!({"ok": false, "disposition": "denied", "reason": "path", "detail": error.to_string()}),
        );
    }
    let flags = flags | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK;
    match rustix::fs::openat2(workspace, path, flags, mode, resolve_flags()) {
        Ok(fd) => Ok(File::from(fd)),
        Err(error) => Err(json!({
            "ok": false, "disposition": "denied", "reason": "openat2", "detail": error.to_string()
        })),
    }
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
    })
}

fn require_bwrap(path: &Path) -> Result<(), String> {
    let output = Command::new(path)
        .arg("--version")
        .env_clear()
        .output()
        .map_err(|error| format!("bubblewrap: {error}"))?;
    if !output.status.success() || output.stdout != b"bubblewrap 0.9.0\n" {
        return Err("reviewed backend requires bubblewrap 0.9.0".into());
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
    fs::read_to_string(path)
        .ok()
        .and_then(|text| {
            text.lines().find_map(|line| {
                let (name, value) = line.split_once(' ')?;
                (name == key).then(|| value.parse().ok()).flatten()
            })
        })
        .unwrap_or(0)
}

fn cgroup_populated(path: &Path) -> bool {
    fs::read_to_string(path.join("cgroup.procs"))
        .map(|text| text.split_whitespace().any(|pid| pid != "0"))
        .unwrap_or(false)
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

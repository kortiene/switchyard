//! Test-only separate bridge policy. No product dependency or selectable backend.
//! The generated launcher adapts PiBridge's existing Node argv to a fixed sandbox;
//! paths/policy come only from the trusted test harness, never protocol frames.
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct BridgeSandbox {
    pub launcher: PathBuf,
    pub database: PathBuf,
    pub status_root: PathBuf,
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// `root` is a fresh harness-owned directory, not an existing mission/workspace.
/// `policy` names the compiled, trusted library (an invalid file is used only by
/// negative tests). No configuration from inside the child can alter this launch.
pub fn prepare(root: &Path, policy: &Path, probe: bool) -> io::Result<BridgeSandbox> {
    let backend = super::probe::backend();
    if !backend.is_absolute() || fs::metadata(&backend)?.permissions().mode() & 0o6000 != 0 {
        return Err(io::Error::other("require absolute non-setuid Bubblewrap"));
    }
    let version = Command::new(&backend)
        .arg("--version")
        .env_clear()
        .output()?;
    if !version.status.success() || version.stdout != b"bubblewrap 0.9.0\n" {
        return Err(io::Error::other("require Bubblewrap 0.9.0"));
    }
    let node = PathBuf::from(
        std::env::var_os("SWITCHYARD_PI_NODE")
            .ok_or_else(|| io::Error::other("SWITCHYARD_PI_NODE is mandatory"))?,
    )
    .canonicalize()?;
    let version = Command::new(&node).arg("--version").env_clear().output()?;
    if !version.status.success() || version.stdout != b"v22.23.4\n" {
        return Err(io::Error::other("require pinned Node 22.23.4"));
    }
    let version = Command::new("/usr/bin/prlimit")
        .arg("--version")
        .env_clear()
        .output()?;
    if !version.status.success() || version.stdout != b"prlimit from util-linux 2.39.3\n" {
        return Err(io::Error::other(
            "require /usr/bin/prlimit from util-linux 2.39.3",
        ));
    }
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../runtime/pi-source")
        .canonicalize()?;
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("bridge")
        .canonicalize()?;
    let root = root.canonicalize()?;
    let state = root.join("state");
    fs::create_dir(&state)?; // refuse an existing directory; never mount a mission parent
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700))?;
    let database = state.join("session.sqlite");
    let status_root = root.join("supervisor");
    fs::create_dir(&status_root)?; // not mounted: child cannot forge PID evidence
    let mut args: Vec<String> = [
        "/usr/bin/prlimit",
        "--cpu=10:10",
        "--fsize=16777216:16777216",
        "--nofile=128:128",
        "--",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    args.push(backend.to_string_lossy().into());
    args.extend(
        [
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
            "LANG",
            "C",
            "--setenv",
            "LD_PRELOAD",
            "/lockdown.so",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--size",
            "8388608",
            "--tmpfs",
            "/tmp",
        ]
        .into_iter()
        .map(str::to_owned),
    );
    let mut mount = |flag: &str, from: &Path, to: &str| {
        args.extend([flag.into(), from.to_string_lossy().into(), to.into()]);
    };
    // Fixed Ubuntu runtime closure, not /usr, the checkout, or host /etc.
    for library in [
        "libc.so.6",
        "libm.so.6",
        "libdl.so.2",
        "libpthread.so.0",
        "librt.so.1",
        "libstdc++.so.6",
        "libgcc_s.so.1",
    ] {
        let path = format!("/lib/x86_64-linux-gnu/{library}");
        mount("--ro-bind", &Path::new(&path).canonicalize()?, &path);
    }
    mount(
        "--ro-bind",
        &Path::new("/lib64/ld-linux-x86-64.so.2").canonicalize()?,
        "/lib64/ld-linux-x86-64.so.2",
    );
    mount("--ro-bind", &node, "/node");
    for part in [
        "src",
        "test/bridge-child.ts",
        "node_modules",
        "pins.json",
        "package.json",
        "package-lock.json",
    ] {
        mount(
            "--ro-bind",
            &source.join(part).canonicalize()?,
            &format!("/runtime/{part}"),
        );
    }
    mount("--ro-bind", &fixtures.join("guard.cjs"), "/guard.cjs");
    mount("--ro-bind", &policy.canonicalize()?, "/lockdown.so");
    // Test-only read-write session bind. The bridge can plant symlinks here.
    // Product opens of bridge-writable paths must not follow them; see the S3 handoff.
    mount("--bind", &state, "/state");
    // PiBridge acquires this before starting the wrapper. Mask the actual inode:
    // bridge writes/unlink/rename cannot remove the host's ownership fence.
    mount(
        "--ro-bind",
        &state.join("session.sqlite.owner.lock"),
        "/state/session.sqlite.owner.lock",
    );
    if probe {
        mount("--ro-bind", &fixtures.join("probe.mjs"), "/probe.mjs");
        mount("--ro-bind", &root.join("case.json"), "/case.json");
    }
    args.extend(
        [
            "--chdir",
            "/state",
            "--remount-ro",
            "/",
            "--",
            "/node",
            "--max-old-space-size=128",
            "--require",
            "/guard.cjs",
            "--experimental-strip-types",
            if probe {
                "/probe.mjs"
            } else {
                "/runtime/test/bridge-child.ts"
            },
            "/state/session.sqlite",
        ]
        .into_iter()
        .map(str::to_owned),
    );
    let launcher = root.join("launch");
    // PiBridge argv: --experimental-strip-types launcher database binding mode.
    // Validate the first three, preserve the two opaque arguments without eval.
    let script = format!(
        "#!/bin/sh\nset -eu\n[ \"$#\" = 5 ]\n[ \"$1\" = --experimental-strip-types ]\n[ \"$2\" = bridge-fixture ]\n[ \"$3\" = {} ]\nexec 3>{}\nexec {} \"$4\" \"$5\"\n",
        quote(&database.to_string_lossy()),
        quote(&status_root.join("status.jsonl").to_string_lossy()),
        args.iter().map(|s| quote(s)).collect::<Vec<_>>().join(" ")
    );
    fs::write(&launcher, script)?;
    fs::set_permissions(&launcher, fs::Permissions::from_mode(0o700))?;
    Ok(BridgeSandbox {
        launcher,
        database,
        status_root,
    })
}

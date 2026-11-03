//! Fixed, credential-free B1 policy, now owned by the Rust session lifecycle.
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::ConfinedPiConfig;
use crate::{Error, Result, payload_hash};
use serde_json::{Value, json};

pub(super) fn source() -> Result<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../runtime/pi-source")
        .canonicalize()
        .map_err(Error::storage)
}

pub(super) fn check(config: &ConfinedPiConfig) -> Result<Value> {
    if !cfg!(target_arch = "x86_64") {
        return Err(Error::guard("bridge policy requires Linux x86_64"));
    }
    let mut files = Vec::new();
    for path in [
        &config.node,
        &config.bubblewrap,
        &config.launcher,
        &config.policy,
    ] {
        let meta = fs::symlink_metadata(path).map_err(Error::storage)?;
        if !path.is_absolute() || !meta.is_file() || meta.mode() & 0o6022 != 0 {
            return Err(Error::guard(
                "bridge runtime requires trusted absolute regular paths without setid/group/world writes",
            ));
        }
        files.push(json!({"path":path,"sha256":crate::canonical::sha256_hex(&fs::read(path).map_err(Error::storage)?)}));
    }
    for (path, expected) in [
        (&config.node, "v22.23.4\n"),
        (&config.bubblewrap, "bubblewrap 0.9.0\n"),
    ] {
        let output = Command::new(path)
            .arg("--version")
            .env_clear()
            .output()
            .map_err(Error::storage)?;
        if !output.status.success() || output.stdout != expected.as_bytes() {
            return Err(Error::guard("confined bridge runtime version mismatch"));
        }
    }
    let source = source()?;
    let provenance = fs::read(source.join(".artifacts/provenance.json")).map_err(Error::storage)?;
    let guard = fs::read(source.join("confinement/guard.cjs")).map_err(Error::storage)?;
    Ok(
        json!({"version":1,"runtime":files,"adapter":super::super::pi_bridge::adapter_fingerprint(),
        "provenance":crate::canonical::sha256_hex(&provenance),"guard":crate::canonical::sha256_hex(&guard),
        "limits":{"memory":268435456,"pids":64,"cpu":"50000 100000","wallSeconds":60},
        "identity":payload_hash(&json!(files))?}),
    )
}

pub(super) fn argv(config: &ConfinedPiConfig, state: &Path) -> Result<Vec<String>> {
    let source = source()?;
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
    args.push(config.bubblewrap.to_string_lossy().into());
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
        mount(
            "--ro-bind",
            &Path::new(&path).canonicalize().map_err(Error::storage)?,
            &path,
        );
    }
    mount(
        "--ro-bind",
        &Path::new("/lib64/ld-linux-x86-64.so.2")
            .canonicalize()
            .map_err(Error::storage)?,
        "/lib64/ld-linux-x86-64.so.2",
    );
    mount("--ro-bind", &config.node, "/node");
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
            &source.join(part).canonicalize().map_err(Error::storage)?,
            &format!("/runtime/{part}"),
        );
    }
    mount(
        "--ro-bind",
        &source.join("confinement/guard.cjs"),
        "/guard.cjs",
    );
    mount("--ro-bind", &config.policy, "/lockdown.so");
    // Only this private data directory is writable. Host control files are absent.
    mount("--bind", state, "/state");
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
            "/runtime/test/bridge-child.ts",
            "/state/session.sqlite",
        ]
        .into_iter()
        .map(str::to_owned),
    );
    Ok(args)
}

//! Fixed, credential-free B1 policy, now owned by the Rust session lifecycle.
//!
//! The runtime root is host configuration. Its expected provenance is rebuilt
//! from the reviewed `pins.json` compiled into this crate. There is no separate
//! compiled provenance hash.
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use super::ConfinedPiConfig;
use crate::{Error, Result, payload_hash};
use serde_json::{Value, json};

const PARTS: &[(&str, bool, &str)] = &[
    ("src", true, "/runtime/src"),
    (
        "test/bridge-child.ts",
        false,
        "/runtime/test/bridge-child.ts",
    ),
    ("node_modules", true, "/runtime/node_modules"),
    ("pins.json", false, "/runtime/pins.json"),
    ("package.json", false, "/runtime/package.json"),
    ("package-lock.json", false, "/runtime/package-lock.json"),
    ("confinement/guard.cjs", false, "/guard.cjs"),
    (".artifacts/provenance.json", false, ""),
];

#[derive(Debug)]
pub(super) struct RuntimeInstall {
    pub root: PathBuf,
    pub digest: String,
    provenance_sha256: String,
    guard_sha256: String,
    mounts: Vec<(PathBuf, String)>,
}

impl RuntimeInstall {
    /// Session-configuration identity. A swapped tree or path changes it, so
    /// the existing opened-ledger comparison refuses the replacement.
    pub(super) fn record(&self) -> Value {
        json!({"path": self.root, "digest": self.digest})
    }
}

fn reviewed_pins() -> &'static Value {
    static PINS: OnceLock<Value> = OnceLock::new();
    PINS.get_or_init(|| {
        serde_json::from_str(include_str!("../../../../../runtime/pi-source/pins.json"))
            .expect("reviewed runtime pins")
    })
}

/// Rebuild the provenance object the source build writes. Every field except
/// the build's fixed decision label is taken from the reviewed pins.
fn expected_provenance(pins: &Value) -> Value {
    json!({
        "decision": "NEEDS-FULL-S2",
        "sourceCommit": pins["commit"],
        "upstreamLockSha256": pins["lockSha256"],
        "catalogRevision": pins["catalogRevision"],
        "node": pins["node"],
        "npm": pins["npm"],
        "packages": pins["packages"],
    })
}

pub(super) fn inspect_runtime(path: &Path) -> Result<RuntimeInstall> {
    let root = validate_root(path)?;
    let mut mounts = Vec::new();
    let mut entries = Vec::new();
    for (rel, directory, mount) in PARTS {
        let canonical = resolve_part(&root, rel, *directory)?;
        collect(&root, rel, &mut entries)?;
        if !mount.is_empty() {
            mounts.push((canonical, (*mount).to_owned()));
        }
    }
    let pins = read_json(&root.join("pins.json"))?;
    if &pins != reviewed_pins() {
        return Err(Error::guard(
            "bridge runtime pins do not match the reviewed pins",
        ));
    }
    let provenance = read_json(&root.join(".artifacts/provenance.json"))?;
    if provenance != expected_provenance(reviewed_pins()) {
        return Err(Error::guard(
            "bridge runtime provenance does not match the reviewed pins",
        ));
    }
    entries.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
    let mut manifest = String::new();
    for (_, line) in &entries {
        manifest.push_str(line);
        manifest.push('\n');
    }
    Ok(RuntimeInstall {
        root,
        digest: format!(
            "sha256:{}",
            crate::canonical::sha256_hex(manifest.as_bytes())
        ),
        provenance_sha256: file_sha(&entries, ".artifacts/provenance.json")?,
        guard_sha256: file_sha(&entries, "confinement/guard.cjs")?,
        mounts,
    })
}

fn file_sha(entries: &[(String, String)], rel: &str) -> Result<String> {
    let prefix = format!("f {rel} ");
    entries
        .iter()
        .find(|(path, _)| path == rel)
        .and_then(|(_, line)| line.strip_prefix(&prefix))
        .map(str::to_owned)
        .ok_or_else(|| Error::guard(format!("bridge runtime root is missing {rel}")))
}

fn read_json(path: &Path) -> Result<Value> {
    let bytes = fs::read(path).map_err(Error::storage)?;
    serde_json::from_slice(&bytes).map_err(|_| Error::guard("bridge runtime JSON is invalid"))
}

fn validate_root(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() || path.to_str().is_none() {
        return Err(Error::guard("bridge runtime root must be absolute UTF-8"));
    }
    for component in path.components() {
        match component {
            Component::RootDir | Component::Normal(_) => {}
            _ => {
                return Err(Error::guard(
                    "bridge runtime root contains a non-normal component",
                ));
            }
        }
    }
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        if matches!(component, Component::RootDir) {
            continue;
        }
        let meta = fs::symlink_metadata(&current).map_err(Error::storage)?;
        if meta.file_type().is_symlink() {
            return Err(Error::guard("bridge runtime root contains a symlink"));
        }
    }
    let meta = fs::symlink_metadata(path).map_err(Error::storage)?;
    if !meta.is_dir()
        || meta.uid() != rustix::process::geteuid().as_raw()
        || meta.mode() & 0o077 != 0
        || meta.mode() & 0o6022 != 0
    {
        return Err(Error::guard(
            "bridge runtime root must be an owner-private directory",
        ));
    }
    let canonical = path.canonicalize().map_err(Error::storage)?;
    if canonical != path {
        return Err(Error::guard("bridge runtime root must be canonical"));
    }
    Ok(canonical)
}

fn resolve_part(root: &Path, rel: &str, directory: bool) -> Result<PathBuf> {
    let mut current = root.to_path_buf();
    let parts: Vec<&str> = rel.split('/').collect();
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() || *part == "." || *part == ".." {
            return Err(Error::guard(
                "bridge runtime root contains a non-normal component",
            ));
        }
        current.push(part);
        let meta = match fs::symlink_metadata(&current) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(Error::guard(format!(
                    "bridge runtime root is missing {rel}"
                )));
            }
            Err(error) => return Err(Error::storage(error)),
        };
        if meta.file_type().is_symlink() {
            return Err(Error::guard(format!(
                "bridge runtime component {rel} is a symlink"
            )));
        }
        if meta.mode() & 0o6022 != 0 {
            return Err(Error::guard(format!(
                "bridge runtime component {rel} is setid or group/world writable"
            )));
        }
        let last = index + 1 == parts.len();
        let expect_dir = !last || directory;
        if expect_dir && !meta.is_dir() {
            return Err(Error::guard(format!(
                "bridge runtime component {rel} is not a directory"
            )));
        }
        if last && !directory && !meta.is_file() {
            return Err(Error::guard(format!(
                "bridge runtime component {rel} is not a regular file"
            )));
        }
    }
    let canonical = current.canonicalize().map_err(Error::storage)?;
    if canonical != current {
        return Err(Error::guard(format!(
            "bridge runtime component {rel} is not canonical"
        )));
    }
    Ok(canonical)
}

/// Digest one required part. Interior symlinks are recorded and not followed,
/// because npm's `node_modules/.bin` entries are links. A symlink that *is*
/// the required part was already refused.
fn collect(root: &Path, rel: &str, entries: &mut Vec<(String, String)>) -> Result<()> {
    let path = root.join(rel);
    let meta = fs::symlink_metadata(&path).map_err(Error::storage)?;
    if meta.file_type().is_symlink() {
        let target = fs::read_link(&path).map_err(Error::storage)?;
        let target = target
            .to_str()
            .filter(|target| !target.contains('\n') && !target.contains('\0'))
            .ok_or_else(|| Error::guard("bridge runtime symlink target is not a single line"))?;
        entries.push((rel.to_owned(), format!("l {rel} {target}")));
        return Ok(());
    }
    if meta.mode() & 0o6022 != 0 {
        return Err(Error::guard(format!(
            "bridge runtime component {rel} is setid or group/world writable"
        )));
    }
    if meta.is_dir() {
        let mut names = Vec::new();
        for entry in fs::read_dir(&path).map_err(Error::storage)? {
            let name = entry.map_err(Error::storage)?.file_name();
            let name = name
                .to_str()
                .filter(|name| !name.is_empty() && !name.contains(['/', '\n', '\0']))
                .ok_or_else(|| Error::guard("bridge runtime file name is not UTF-8"))?;
            names.push(name.to_owned());
        }
        names.sort();
        for name in names {
            collect(root, &format!("{rel}/{name}"), entries)?;
        }
        return Ok(());
    }
    if meta.is_file() {
        let bytes = fs::read(&path).map_err(Error::storage)?;
        entries.push((
            rel.to_owned(),
            format!("f {rel} {}", crate::canonical::sha256_hex(&bytes)),
        ));
        return Ok(());
    }
    Err(Error::guard(format!(
        "bridge runtime component {rel} is a special file"
    )))
}

pub(super) fn check(config: &ConfinedPiConfig) -> Result<Value> {
    if !cfg!(target_arch = "x86_64") {
        return Err(Error::guard("bridge policy requires Linux x86_64"));
    }
    // Pins and provenance fail before any runtime binary is executed.
    let install = inspect_runtime(&config.runtime)?;
    let prlimit = PathBuf::from("/usr/bin/prlimit");
    let mut files = Vec::new();
    for path in [
        &config.node,
        &config.bubblewrap,
        &config.launcher,
        &config.policy,
        &prlimit,
    ] {
        let meta = fs::symlink_metadata(path).map_err(Error::storage)?;
        if !path.is_absolute()
            || path.to_str().is_none()
            || !meta.is_file()
            || meta.mode() & 0o6022 != 0
        {
            return Err(Error::guard(format!(
                "untrusted bridge runtime path {} (mode {:o}); requires an absolute regular file without setid/group/world writes",
                path.display(),
                meta.mode()
            )));
        }
        files.push(json!({"path":path,"sha256":crate::canonical::sha256_hex(&fs::read(path).map_err(Error::storage)?)}));
    }
    for (path, expected) in [
        (&config.node, "v22.23.4\n"),
        (&config.bubblewrap, "bubblewrap 0.9.0\n"),
        (&prlimit, "prlimit from util-linux 2.39.3\n"),
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
    let runtime_root = install.record();
    Ok(
        json!({"version":1,"runtime":files,"adapter":super::super::pi_bridge::adapter_fingerprint(),
        "provenance":install.provenance_sha256,"guard":install.guard_sha256,
        "runtimeRoot":runtime_root,
        "limits":{"memory":268435456,"pids":64,"cpu":"50000 100000","wallSeconds":super::BRIDGE_WALL.as_secs()},
        "identity":payload_hash(&json!(files))?,
        "egress":super::egress::record(config.model_endpoint.as_ref())}),
    )
}

pub(super) fn argv(
    config: &ConfinedPiConfig,
    state: &Path,
    proxy_dir: Option<&Path>,
    installed: &Value,
) -> Result<Vec<String>> {
    // Re-hash at launch and bind the canonical paths from this second look.
    let install = inspect_runtime(&config.runtime)?;
    if installed["runtimeRoot"] != install.record() {
        return Err(Error::guard(
            "runtime root changed between validation and launch",
        ));
    }
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
            // Thread-directed PR_SET_PDEATHSIG. The session supervisor is that parent.
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
    for (from, to) in &install.mounts {
        mount("--ro-bind", from, to);
    }
    mount("--ro-bind", &config.policy, "/lockdown.so");
    // Only this private data directory is writable. Host control files are absent.
    mount("--bind", state, "/state");
    match (&config.model_endpoint, proxy_dir) {
        (None, None) => {}
        (Some(_), Some(directory)) => args.extend(super::egress::mount_args(directory)?),
        _ => {
            return Err(Error::guard(
                "model endpoint proxy does not match host configuration",
            ));
        }
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
            "/runtime/test/bridge-child.ts",
            "/state/session.sqlite",
        ]
        .into_iter()
        .map(str::to_owned),
    );
    Ok(args)
}

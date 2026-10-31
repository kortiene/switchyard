use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use crate::canonical::sha256_hex;
use crate::domain::RepoIdentity;
use crate::error::{Error, Result};

/// Read repository identity without writing the checkout.
///
/// Hooks, fsmonitor, and optional locks are disabled. The git process receives
/// an allowlisted environment. S1 does not require a clean tree; the porcelain
/// hash is recorded so later slices can see drift. A repository without a
/// commit cannot supply a base SHA and is rejected.
pub fn inspect_repo(path: &Path) -> Result<RepoIdentity> {
    let inside = git(path, &["rev-parse", "--is-inside-work-tree"])?;
    if inside.trim() != "true" {
        return Err(Error::GitFailed {
            detail: format!("{} is not a git work tree", path.display()),
        });
    }
    let toplevel = git(path, &["rev-parse", "--show-toplevel"])?;
    let head = git(path, &["rev-parse", "HEAD"]).map_err(|_| Error::GitFailed {
        detail: "repository has no commits, so no base SHA can be frozen".into(),
    })?;
    let branch = git(path, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let porcelain = git_bytes(path, &["status", "--porcelain=v1", "-z"])?;
    let origin = git(path, &["remote", "get-url", "origin"]).ok();
    Ok(RepoIdentity {
        toplevel: toplevel.trim().to_string(),
        head_sha: head.trim().to_string(),
        branch: branch.trim().to_string(),
        porcelain_hash: format!("sha256:{}", sha256_hex(&porcelain)),
        origin_url: origin
            .map(|value| strip_userinfo(value.trim()))
            .filter(|value| !value.is_empty()),
    })
}

/// Drop URL userinfo such as `user:token@` before the identity is stored.
pub fn strip_userinfo(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        return url.to_string();
    };
    let rest = &url[scheme_end + 3..];
    let Some(at) = rest.find('@') else {
        return url.to_string();
    };
    if rest[..at].contains('/') {
        return url.to_string();
    }
    format!("{}://{}", &url[..scheme_end], &rest[at + 1..])
}

pub(super) fn git(path: &Path, args: &[&str]) -> Result<String> {
    let bytes = git_bytes(path, args)?;
    String::from_utf8(bytes).map_err(|error| Error::GitFailed {
        detail: format!("git output was not utf-8: {error}"),
    })
}

pub(super) fn git_bytes(path: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = command(path)?.args(args).output().map_err(Error::storage)?;
    checked_output(args, output)
}

pub(super) fn checked_output(args: &[&str], output: std::process::Output) -> Result<Vec<u8>> {
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::GitFailed {
            detail: format!("git {} failed: {}", args.join(" "), stderr.trim()),
        });
    }
    Ok(output.stdout)
}

fn base_command(path: &Path) -> Command {
    let mut command = Command::new("git");
    command.env_clear();
    if let Some(path_var) = std::env::var_os("PATH") {
        command.env("PATH", path_var);
    }
    command.env("GIT_OPTIONAL_LOCKS", "0");
    command.env("GIT_CONFIG_NOSYSTEM", "1");
    command.env("GIT_CONFIG_GLOBAL", "/dev/null");
    command.env("GIT_CONFIG_SYSTEM", "/dev/null");
    command.env("GIT_TERMINAL_PROMPT", "0");
    command.env("GIT_NO_REPLACE_OBJECTS", "1");
    command.env("GIT_NO_LAZY_FETCH", "1");
    command.env("GIT_ATTR_NOSYSTEM", "1");
    for setting in [
        "core.fsmonitor=false",
        "core.hooksPath=/dev/null",
        "core.attributesFile=/dev/null",
        "submodule.recurse=false",
        "core.sparseCheckout=false",
        "core.sparseCheckoutCone=false",
        "core.autocrlf=false",
        "core.eol=lf",
        "protocol.allow=never",
        "gc.auto=0",
        "maintenance.auto=false",
        "checkout.workers=1",
    ] {
        command.args(["-c", setting]);
    }
    command.arg("-C").arg(path);
    command
}

/// Git checkout/status can execute configured filters, even without hooks.
/// Disable every discovered driver at command scope. Configuration discovery
/// itself does not run a driver, and inherited Git config/environment is cleared.
///
/// Drivers are cached per checkout for this process. The operator's Git
/// configuration must not change while a host is using that checkout, and a
/// worktree path is cached separately from its source so `includeIf` still
/// applies there on first use.
pub(super) fn command(path: &Path) -> Result<Command> {
    let mut command = base_command(path);
    for setting in filter_overrides(path)? {
        command.arg("-c").arg(setting);
    }
    Ok(command)
}

fn filter_overrides(path: &Path) -> Result<Vec<String>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Vec<String>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let key = path.to_path_buf();
    if let Some(cached) = cache
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .get(&key)
    {
        return Ok(cached.clone());
    }
    let discovered = discover_filter_overrides(path)?;
    let mut guard = cache.lock().unwrap_or_else(|poison| poison.into_inner());
    Ok(guard.entry(key).or_insert(discovered).clone())
}

fn discover_filter_overrides(path: &Path) -> Result<Vec<String>> {
    let args = [
        "config",
        "--null",
        "--name-only",
        "--get-regexp",
        "^filter\\..*\\.(clean|smudge|process|required)$",
    ];
    let output = base_command(path)
        .args(args)
        .output()
        .map_err(Error::storage)?;
    if output.status.code() == Some(1) {
        return Ok(Vec::new());
    }
    let keys = String::from_utf8(checked_output(&args, output)?).map_err(Error::storage)?;
    let mut drivers = std::collections::BTreeSet::new();
    for key in keys.split('\0').filter(|key| !key.is_empty()) {
        let (driver, _) = key
            .rsplit_once('.')
            .ok_or_else(|| Error::guard("invalid filter key"))?;
        drivers.insert(driver.to_string());
    }
    let mut settings = Vec::new();
    for driver in drivers {
        for suffix in ["clean=", "smudge=", "process=", "required=false"] {
            settings.push(format!("{driver}.{suffix}"));
        }
    }
    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::strip_userinfo;

    #[test]
    fn origin_userinfo_is_removed() {
        assert_eq!(
            strip_userinfo("https://user:supersecret@github.com/kortiene/switchyard.git"),
            "https://github.com/kortiene/switchyard.git"
        );
        assert_eq!(
            strip_userinfo("ssh://git@github.com/kortiene/switchyard.git"),
            "ssh://github.com/kortiene/switchyard.git"
        );
        assert_eq!(
            strip_userinfo("https://github.com/kortiene/switchyard.git"),
            "https://github.com/kortiene/switchyard.git"
        );
    }
}

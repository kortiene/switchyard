use std::path::Path;
use std::process::Command;

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

fn git(path: &Path, args: &[&str]) -> Result<String> {
    let bytes = git_bytes(path, args)?;
    String::from_utf8(bytes).map_err(|error| Error::GitFailed {
        detail: format!("git output was not utf-8: {error}"),
    })
}

fn git_bytes(path: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let mut command = Command::new("git");
    command.env_clear();
    if let Some(path_var) = std::env::var_os("PATH") {
        command.env("PATH", path_var);
    }
    command.env("GIT_OPTIONAL_LOCKS", "0");
    command.env("GIT_CONFIG_NOSYSTEM", "1");
    command.env("GIT_TERMINAL_PROMPT", "0");
    command
        .arg("-c")
        .arg("core.fsmonitor=")
        .arg("-c")
        .arg("core.hooksPath=/dev/null")
        .arg("-C")
        .arg(path);
    command.args(args);
    let output = command.output().map_err(|error| Error::GitFailed {
        detail: format!("failed to run git: {error}"),
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::GitFailed {
            detail: format!("git {} failed: {}", args.join(" "), stderr.trim()),
        });
    }
    Ok(output.stdout)
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

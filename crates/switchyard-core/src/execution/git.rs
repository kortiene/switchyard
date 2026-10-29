use std::path::Path;
use std::process::Command;

use crate::canonical::sha256_hex;
use crate::domain::RepoIdentity;
use crate::error::{Error, Result};

/// Read repository identity without writing the checkout.
///
/// Hooks and fsmonitor are disabled. Credential-bearing environment variables
/// are removed from the git process. S1 does not require a clean tree; the
/// porcelain hash is recorded so later slices can see drift. A repository
/// without a commit cannot supply a base SHA and is rejected.
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
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
    })
}

fn git(path: &Path, args: &[&str]) -> Result<String> {
    let bytes = git_bytes(path, args)?;
    String::from_utf8(bytes).map_err(|error| Error::GitFailed {
        detail: format!("git output was not utf-8: {error}"),
    })
}

fn git_bytes(path: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let mut command = Command::new("git");
    command
        .arg("-c")
        .arg("core.fsmonitor=")
        .arg("-c")
        .arg("core.hooksPath=/dev/null")
        .arg("-C")
        .arg(path);
    command.args(args);
    for name in [
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "GH_ENTERPRISE_TOKEN",
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "AWS_SECRET_ACCESS_KEY",
        "AWS_SESSION_TOKEN",
    ] {
        command.env_remove(name);
    }
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

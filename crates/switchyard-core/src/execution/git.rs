use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
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
        return Err(Error::git(
            format!("{} is not a git work tree", path.display()),
            None,
        ));
    }
    let toplevel = git(path, &["rev-parse", "--show-toplevel"])?;
    let head = git(path, &["rev-parse", "HEAD"]).map_err(|_| {
        Error::git(
            "repository has no commits, so no base SHA can be frozen",
            None,
        )
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
    String::from_utf8(bytes)
        .map_err(|error| Error::git(format!("git output was not utf-8: {error}"), None))
}

pub(super) fn git_bytes(path: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = command(path)?.args(args).output().map_err(Error::storage)?;
    checked_output(args, output)
}

pub(super) fn checked_output(args: &[&str], output: std::process::Output) -> Result<Vec<u8>> {
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // `code()` is `None` when the child was killed by a signal. That stays
        // a retryable GitFailed; only a completed non-zero exit is evidence.
        return Err(Error::git(
            format!("git {} failed: {}", args.join(" "), stderr.trim()),
            output.status.code(),
        ));
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
/// Discovery is cached only inside a [`FilterCacheScope`]. The scope is one
/// prepare or verify, so a driver added afterwards — including through
/// `includeIf` — is seen on the next call. Outside a scope every command
/// rediscovers.
pub(super) fn command(path: &Path) -> Result<Command> {
    let mut command = base_command(path);
    for setting in filter_overrides(path)? {
        command.arg("-c").arg(setting);
    }
    Ok(command)
}

/// Cache of disabled filter drivers for the current prepare/verify.
/// Nested scopes share one map. The outermost drop clears it.
#[must_use]
pub(crate) struct FilterCacheScope;

thread_local! {
    static FILTER_SCOPE_DEPTH: Cell<usize> = const { Cell::new(0) };
    static FILTER_CACHE: RefCell<HashMap<PathBuf, Vec<String>>> = RefCell::new(HashMap::new());
}

impl FilterCacheScope {
    pub(crate) fn enter() -> Self {
        FILTER_SCOPE_DEPTH.with(|depth| depth.set(depth.get().saturating_add(1)));
        Self
    }
}

impl Drop for FilterCacheScope {
    fn drop(&mut self) {
        FILTER_SCOPE_DEPTH.with(|depth| {
            let next = depth.get().saturating_sub(1);
            depth.set(next);
            if next == 0 {
                FILTER_CACHE.with(|cache| cache.borrow_mut().clear());
            }
        });
    }
}

fn filter_overrides(path: &Path) -> Result<Vec<String>> {
    let in_scope = FILTER_SCOPE_DEPTH.with(|depth| depth.get() > 0);
    if in_scope {
        let cached = FILTER_CACHE.with(|cache| cache.borrow().get(path).cloned());
        if let Some(cached) = cached {
            return Ok(cached);
        }
    }
    let discovered = discover_filter_overrides(path)?;
    if in_scope {
        FILTER_CACHE.with(|cache| {
            cache
                .borrow_mut()
                .insert(path.to_path_buf(), discovered.clone());
        });
    }
    Ok(discovered)
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

    #[cfg(unix)]
    #[test]
    fn filter_driver_added_after_first_use_is_still_disabled() {
        use std::os::unix::fs::PermissionsExt;
        use std::process::Stdio;

        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "--initial-branch=main"]);
        git(&["config", "user.email", "filter@example.com"]);
        git(&["config", "user.name", "Filter"]);
        std::fs::write(repo.join("README.md"), "hello\n").unwrap();
        git(&["add", "README.md"]);
        git(&["commit", "-m", "init"]);

        {
            let _scope = super::FilterCacheScope::enter();
            let output = super::command(&repo)
                .unwrap()
                .args(["status", "--porcelain=v1"])
                .output()
                .unwrap();
            assert!(output.status.success());
        }

        let sentinel = dir.path().join("executed");
        let hook = dir.path().join("hook");
        std::fs::write(
            &hook,
            format!("#!/bin/sh\ntouch '{}'\ncat\n", sentinel.display()),
        )
        .unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        let included = dir.path().join("late.gitconfig");
        std::fs::write(
            &included,
            format!(
                "[filter \"late\"]\n\tclean = {hook}\n\tsmudge = {hook}\n\tprocess = {hook}\n\trequired = true\n",
                hook = hook.display()
            ),
        )
        .unwrap();
        let git_dir =
            String::from_utf8(git_out(&repo, &["rev-parse", "--absolute-git-dir"])).unwrap();
        // `gitdir:<exact>/.path` is parsed as a trailing-slash pattern and does
        // not match the repository. The variable is `.path` on the exact gitdir.
        let pattern = format!("includeIf.gitdir:{}.path", git_dir.trim());
        git(&["config", &pattern, included.to_str().unwrap()]);
        std::fs::write(repo.join(".gitattributes"), "*.md filter=late\n").unwrap();
        std::fs::write(repo.join("README.md"), "changed\n").unwrap();
        let configured =
            String::from_utf8(git_out(&repo, &["config", "--get", "filter.late.clean"])).unwrap();
        assert!(
            configured.contains(&hook.display().to_string()),
            "includeIf filter was not visible to git: {configured}"
        );
        let attr = String::from_utf8(git_out(&repo, &["check-attr", "filter", "--", "README.md"]))
            .unwrap();
        assert!(
            attr.contains("filter: late"),
            "attribute did not select the new driver: {attr}"
        );

        {
            let _scope = super::FilterCacheScope::enter();
            let output = super::command(&repo)
                .unwrap()
                .args(["add", "--", "README.md"])
                .stderr(Stdio::piped())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        assert!(
            !sentinel.exists(),
            "a filter driver added after the first git call still ran"
        );
    }

    #[cfg(unix)]
    fn git_out(repo: &std::path::Path, args: &[&str]) -> Vec<u8> {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success());
        output.stdout
    }
}

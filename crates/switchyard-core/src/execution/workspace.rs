use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;

use serde_json::Value;

use super::git::{command, git, git_bytes};
use crate::domain::MissionRecord;
use crate::error::{Error, Result};
use crate::workspace::{WorkspaceRecord, WorkspaceView};

/// Dropping the host's copy closes it; never explicitly unlock it. Git inherits
/// a duplicate as stdin so a surviving creation process retains the flock.
pub(crate) struct WorkspaceLease(File);

pub(crate) fn canonical_directory(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(Error::guard(
            "workspace paths must be absolute without dot components",
        ));
    }
    let canonical = fs::canonicalize(path).map_err(Error::storage)?;
    if canonical.as_os_str() != path.as_os_str()
        || !fs::metadata(&canonical).map_err(Error::storage)?.is_dir()
    {
        return Err(Error::guard(
            "workspace paths cannot use symlinks or aliases",
        ));
    }
    path_text(&canonical)?;
    Ok(canonical)
}

pub(crate) fn path_text(path: &Path) -> Result<String> {
    let text = path
        .to_str()
        .ok_or_else(|| Error::guard("workspace paths must be UTF-8"))?;
    if text.chars().any(char::is_control) {
        return Err(Error::guard("control characters in workspace path"));
    }
    Ok(text.into())
}

#[cfg(unix)]
pub(crate) fn identity(path: &Path) -> Result<String> {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::symlink_metadata(path).map_err(Error::storage)?;
    if metadata.file_type().is_symlink() {
        return Err(Error::guard("symlink in workspace ownership metadata"));
    }
    Ok(format!("{}:{}", metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
pub(crate) fn identity(_path: &Path) -> Result<String> {
    Err(Error::guard(
        "workspace ownership requires Unix filesystem identities",
    ))
}

fn check_identity(path: &Path, expected: &str) -> Result<()> {
    if identity(path)? != expected {
        return Err(Error::guard(format!(
            "workspace filesystem identity changed: {}",
            path.display()
        )));
    }
    Ok(())
}

fn regular_text(path: &Path) -> Result<String> {
    let metadata = fs::symlink_metadata(path).map_err(Error::storage)?;
    if !metadata.is_file() || metadata.len() > 16 * 1024 {
        return Err(Error::guard(format!(
            "invalid workspace metadata: {}",
            path.display()
        )));
    }
    fs::read_to_string(path).map_err(Error::storage)
}

fn common_dir(source: &Path) -> Result<PathBuf> {
    let common = git(
        source,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    canonical_directory(Path::new(common.trim_end_matches('\n')))
}

pub(crate) fn reserve(
    mission: &MissionRecord,
    root: &Path,
    ledger_path: &Path,
    generation: u64,
) -> Result<WorkspaceRecord> {
    let root = canonical_directory(root)?;
    let source = canonical_directory(Path::new(&mission.repo.toplevel))?;
    let common = common_dir(&source)?;
    if !mission
        .attempt
        .id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err(Error::guard("invalid attempt workspace name"));
    }
    let slot = root.join(&mission.attempt.id);
    if slot.starts_with(&source) || source.starts_with(&slot) {
        return Err(Error::guard(
            "workspace must be outside the source checkout",
        ));
    }
    // Never nest under any registered checkout or Git administration directory.
    let listing = git_bytes(&source, &["worktree", "list", "--porcelain", "-z"])?;
    let listing = String::from_utf8(listing).map_err(Error::storage)?;
    for field in listing.split('\0') {
        if let Some(path) = field.strip_prefix("worktree ") {
            let path = fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path));
            if slot.starts_with(&path) || path.starts_with(&slot) {
                return Err(Error::guard("workspace must be outside existing checkouts"));
            }
        }
    }
    if slot.starts_with(&common) || common.starts_with(&slot) {
        return Err(Error::guard("workspace overlaps Git administration"));
    }
    require_absent(&slot)?;
    let base = &mission.attempt.base_sha;
    if ![40, 64].contains(&base.len()) || !base.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(Error::guard(
            "workspace requires an exact frozen commit SHA",
        ));
    }
    if git(&source, &["cat-file", "-t", base])?.trim() != "commit" {
        return Err(Error::guard("workspace base is not a commit"));
    }
    Ok(WorkspaceRecord {
        ledger_path: path_text(ledger_path)?,
        view: WorkspaceView {
            id: crate::domain::new_id("w"),
            mission_id: mission.id.clone(),
            attempt_id: mission.attempt.id.clone(),
            path: path_text(&slot.join("tree"))?,
            base_sha: base.clone(),
            state: "reserved".into(),
            created_generation: generation,
            verified_generation: None,
            detail: None,
        },
        root: path_text(&root)?,
        root_identity: identity(&root)?,
        source: path_text(&source)?,
        common_dir: path_text(&common)?,
        common_identity: identity(&common)?,
        slot_identity: None,
        lease_identity: None,
        tree_identity: None,
        git_dir: None,
    })
}

pub(crate) fn validate_source(w: &WorkspaceRecord) -> Result<()> {
    canonical_directory(Path::new(&w.root))?;
    check_identity(Path::new(&w.root), &w.root_identity)?;
    canonical_directory(Path::new(&w.source))?;
    let common = common_dir(Path::new(&w.source))?;
    if common != Path::new(&w.common_dir) {
        return Err(Error::guard("source repository changed"));
    }
    check_identity(&common, &w.common_identity)?;
    if Path::new(&w.view.path) != w.slot().join("tree") {
        return Err(Error::guard("workspace path does not match its owned slot"));
    }
    Ok(())
}

pub(crate) fn require_absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(Error::storage(error)),
        Ok(_) => Err(Error::guard(format!(
            "workspace path already exists: {}",
            path.display()
        ))),
    }
}

pub(crate) fn initialize_slot(w: &WorkspaceRecord) -> Result<(WorkspaceLease, String, String)> {
    validate_source(w)?;
    require_absent(&w.slot())?;
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(w.slot()).map_err(Error::storage)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let lease_path = w.slot().join("owner.lock");
    let lease = options.open(&lease_path).map_err(Error::storage)?;
    lease.try_lock().map_err(Error::storage)?;
    lease.sync_all().map_err(Error::storage)?;
    let mut marker = options
        .open(w.slot().join("owner.json"))
        .map_err(Error::storage)?;
    marker
        .write_all(w.marker().to_string().as_bytes())
        .map_err(Error::storage)?;
    marker.sync_all().map_err(Error::storage)?;
    sync_directory(&w.slot())?;
    sync_directory(Path::new(&w.root))?;
    Ok((
        WorkspaceLease(lease),
        identity(&w.slot())?,
        identity(&lease_path)?,
    ))
}

/// `None` means an earlier creator still holds the lease. Do not quarantine or
/// replace its workspace; retry after that process releases the descriptor.
pub(crate) fn acquire(w: &WorkspaceRecord) -> Result<Option<WorkspaceLease>> {
    validate_source(w)?;
    canonical_directory(&w.slot())?;
    check_identity(
        &w.slot(),
        w.slot_identity
            .as_deref()
            .ok_or_else(|| Error::guard("missing slot identity"))?,
    )?;
    let marker: Value = serde_json::from_str(&regular_text(&w.slot().join("owner.json"))?)
        .map_err(Error::storage)?;
    if marker != w.marker() {
        return Err(Error::guard("workspace ownership marker mismatch"));
    }
    let path = w.slot().join("owner.lock");
    let expected = w
        .lease_identity
        .as_deref()
        .ok_or_else(|| Error::guard("missing lease identity"))?;
    check_identity(&path, expected)?;
    if !fs::symlink_metadata(&path)
        .map_err(Error::storage)?
        .is_file()
    {
        return Err(Error::guard("invalid workspace lease"));
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .map_err(Error::storage)?;
    match file.try_lock() {
        Ok(()) => {
            check_identity(&path, expected)?;
            Ok(Some(WorkspaceLease(file)))
        }
        Err(fs::TryLockError::WouldBlock) => Ok(None),
        Err(fs::TryLockError::Error(error)) => Err(Error::storage(error)),
    }
}

pub(crate) fn create(w: &WorkspaceRecord, lease: &WorkspaceLease) -> Result<()> {
    validate_source(w)?;
    require_absent(Path::new(&w.view.path))?;
    let reason = w.reason();
    let args = [
        "worktree",
        "add",
        "--quiet",
        "--no-checkout",
        "--detach",
        "--lock",
        "--reason",
        &reason,
        "--",
        &w.view.path,
        &w.view.base_sha,
    ];
    // Durable diagnostics avoid a broken stdout/stderr pipe killing Git when
    // the host disappears. The child keeps both its log and creation lease.
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let log = options
        .open(w.slot().join("git-create.log"))
        .map_err(Error::storage)?;
    creation_git(w, lease, &log, Path::new(&w.source), &args)?;
    // Discover filters again in the new worktree: includeIf/worktree config
    // can activate drivers that were absent in the source checkout.
    let tree = Path::new(&w.view.path);
    verify_binding(w)?;
    for entry in fs::read_dir(tree).map_err(Error::storage)? {
        if entry.map_err(Error::storage)?.file_name() != ".git" {
            return Err(Error::guard(
                "new workspace contains unexpected files before checkout",
            ));
        }
    }
    creation_git(w, lease, &log, tree, &["reset", "--hard", &w.view.base_sha])?;
    Ok(())
}

fn creation_git(
    w: &WorkspaceRecord,
    lease: &WorkspaceLease,
    log: &File,
    cwd: &Path,
    args: &[&str],
) -> Result<()> {
    let status = command(cwd)?
        .args(args)
        .stdin(Stdio::from(lease.0.try_clone().map_err(Error::storage)?))
        .stdout(Stdio::from(log.try_clone().map_err(Error::storage)?))
        .stderr(Stdio::from(log.try_clone().map_err(Error::storage)?))
        .status()
        .map_err(Error::storage)?;
    log.sync_all().map_err(Error::storage)?;
    if !status.success() {
        return Err(Error::GitFailed {
            detail: format!(
                "worktree creation failed ({status}); retained diagnostics in {}",
                w.slot().join("git-create.log").display()
            ),
        });
    }
    Ok(())
}

fn verify_binding(w: &WorkspaceRecord) -> Result<(String, PathBuf)> {
    validate_source(w)?;
    let tree = canonical_directory(Path::new(&w.view.path))?;
    let tree_identity = identity(&tree)?;
    if w.tree_identity
        .as_ref()
        .is_some_and(|expected| expected != &tree_identity)
    {
        return Err(Error::guard("workspace directory was replaced"));
    }
    let dot_git = regular_text(&tree.join(".git"))?;
    let admin = dot_git
        .trim_end_matches('\n')
        .strip_prefix("gitdir: ")
        .ok_or_else(|| Error::guard("missing worktree gitdir link"))?;
    let admin = canonical_directory(Path::new(admin))?;
    if admin.parent() != Some(Path::new(&w.common_dir).join("worktrees").as_path())
        || w.git_dir
            .as_ref()
            .is_some_and(|expected| Path::new(expected) != admin)
    {
        return Err(Error::guard("workspace Git administration mismatch"));
    }
    if regular_text(&admin.join("gitdir"))?.trim_end_matches('\n') != path_text(&tree.join(".git"))?
        || regular_text(&admin.join("locked"))?.trim_end_matches('\n') != w.reason()
        || regular_text(&admin.join("HEAD"))?.trim_end_matches('\n') != w.view.base_sha
    {
        return Err(Error::guard(
            "workspace binding, lock reason or frozen HEAD mismatch",
        ));
    }
    let relative_common = regular_text(&admin.join("commondir"))?;
    if fs::canonicalize(admin.join(relative_common.trim_end_matches('\n')))
        .map_err(Error::storage)?
        != Path::new(&w.common_dir)
        || common_dir(&tree)? != Path::new(&w.common_dir)
        || git(&tree, &["rev-parse", "--show-toplevel"])?.trim_end_matches('\n') != w.view.path
        || git(&tree, &["rev-parse", "--absolute-git-dir"])?.trim_end_matches('\n')
            != path_text(&admin)?
    {
        return Err(Error::guard("workspace repository backlink mismatch"));
    }
    Ok((tree_identity, admin))
}

pub(crate) fn verify(w: &WorkspaceRecord) -> Result<(String, String)> {
    let (tree_identity, admin) = verify_binding(w)?;
    let tree = Path::new(&w.view.path);
    require_absent(&admin.join("index.lock"))?;
    if !fs::symlink_metadata(admin.join("index"))
        .map_err(Error::storage)?
        .is_file()
    {
        return Err(Error::guard("missing worktree index"));
    }
    // Validate index readability while retaining legitimate staged/dirty edits.
    git_bytes(tree, &["ls-files", "--stage", "-z"])?;
    if git(
        tree,
        &["cat-file", "-t", &format!("{}^{{tree}}", w.view.base_sha)],
    )?
    .trim()
        != "tree"
    {
        return Err(Error::guard("frozen workspace tree is unavailable"));
    }
    if w.view.state == "creating"
        && !git_bytes(
            tree,
            &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        )?
        .is_empty()
    {
        return Err(Error::guard(
            "interrupted workspace checkout is incomplete or changed",
        ));
    }
    Ok((tree_identity, path_text(&admin)?))
}

fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(Error::storage)
}

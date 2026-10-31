use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;

use serde_json::Value;

pub(crate) use super::git::FilterCacheScope;
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
    // `components` drops a trailing slash and duplicate separators. The
    // canonical path must still name the same directory, so symlinks stay
    // rejected while `/workspaces/` matches `/workspaces`.
    let normalized: PathBuf = path.components().collect();
    let canonical = fs::canonicalize(&normalized).map_err(evidence_io)?;
    if canonical != normalized || !fs::metadata(&canonical).map_err(evidence_io)?.is_dir() {
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
    let metadata = fs::symlink_metadata(path).map_err(evidence_io)?;
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

fn evidence_io(error: std::io::Error) -> Error {
    if error.kind() == std::io::ErrorKind::NotFound {
        Error::guard(format!("workspace evidence is missing: {error}"))
    } else {
        Error::storage(error)
    }
}

fn regular_text(path: &Path) -> Result<String> {
    let metadata = fs::symlink_metadata(path).map_err(evidence_io)?;
    if !metadata.is_file() || metadata.len() > 16 * 1024 {
        return Err(Error::guard(format!(
            "invalid workspace metadata: {}",
            path.display()
        )));
    }
    fs::read_to_string(path).map_err(evidence_io)
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
    match fs::symlink_metadata(w.slot()) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => create_slot(w),
        Err(error) => Err(Error::storage(error)),
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err(Error::guard(format!(
                "workspace path already exists: {}",
                w.slot().display()
            )))
        }
        Ok(_) => adopt_reserved_slot(w),
    }
}

fn create_slot(w: &WorkspaceRecord) -> Result<(WorkspaceLease, String, String)> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(w.slot()).map_err(Error::storage)?;
    // This call created the directory. A later error must not leave a slot
    // without owner.json, or the retry would treat it as foreign and become
    // permanently uncertain.
    match finish_created_slot(w) {
        Ok(value) => Ok(value),
        Err(error) => {
            remove_created_slot(&w.slot())?;
            Err(error)
        }
    }
}

fn finish_created_slot(w: &WorkspaceRecord) -> Result<(WorkspaceLease, String, String)> {
    #[cfg(test)]
    if std::env::var_os("SW_WORKSPACE_FAIL_AFTER_MKDIR").is_some() {
        return Err(Error::storage(
            "injected failure after creating the workspace slot",
        ));
    }
    let lease_path = w.slot().join("owner.lock");
    let lease = create_lock(&lease_path)?;
    write_marker(w)?;
    sync_directory(&w.slot())?;
    sync_directory(Path::new(&w.root))?;
    Ok((
        WorkspaceLease(lease),
        identity(&w.slot())?,
        identity(&lease_path)?,
    ))
}

fn remove_created_slot(slot: &Path) -> Result<()> {
    match fs::remove_dir_all(slot) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(Error::storage(error)),
    }
}

/// A crash after the slot and `owner.json` are durable, but before `creating`
/// is committed, still has the marker the host just wrote. Reclaim that slot
/// instead of treating it as a foreign directory. Anything else stays evidence.
fn adopt_reserved_slot(w: &WorkspaceRecord) -> Result<(WorkspaceLease, String, String)> {
    let marker_path = w.slot().join("owner.json");
    let marker = read_marker(&marker_path)?;
    if marker != w.marker() {
        return Err(Error::guard(format!(
            "workspace path already exists: {}",
            w.slot().display()
        )));
    }
    for entry in fs::read_dir(w.slot()).map_err(Error::storage)? {
        let name = entry.map_err(Error::storage)?.file_name();
        if name != "owner.json" && name != "owner.lock" {
            return Err(Error::guard(
                "reserved workspace slot holds unexpected evidence",
            ));
        }
    }
    let lease_path = w.slot().join("owner.lock");
    let lease = match fs::symlink_metadata(&lease_path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => create_lock(&lease_path)?,
        Err(error) => return Err(Error::storage(error)),
        Ok(metadata) if metadata.is_file() => open_lock(&lease_path, &w.view.id)?,
        Ok(_) => return Err(Error::guard("invalid workspace lease")),
    };
    if read_marker(&marker_path)? != w.marker() {
        return Err(Error::guard("workspace ownership marker mismatch"));
    }
    sync_directory(&w.slot())?;
    Ok((
        WorkspaceLease(lease),
        identity(&w.slot())?,
        identity(&lease_path)?,
    ))
}

fn exclusive_file_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

fn create_lock(path: &Path) -> Result<File> {
    let lease = exclusive_file_options()
        .open(path)
        .map_err(Error::storage)?;
    match lease.try_lock() {
        Ok(()) => {}
        Err(fs::TryLockError::WouldBlock) => {
            return Err(Error::guard("workspace lease is already held"));
        }
        Err(fs::TryLockError::Error(error)) => return Err(Error::storage(error)),
    }
    lease.sync_all().map_err(Error::storage)?;
    Ok(lease)
}

fn open_lock(path: &Path, workspace_id: &str) -> Result<File> {
    let lease = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(Error::storage)?;
    match lease.try_lock() {
        Ok(()) => Ok(lease),
        Err(fs::TryLockError::WouldBlock) => Err(Error::WorkspaceBusy {
            workspace_id: workspace_id.to_string(),
        }),
        Err(fs::TryLockError::Error(error)) => Err(Error::storage(error)),
    }
}

fn write_marker(w: &WorkspaceRecord) -> Result<()> {
    let mut marker = exclusive_file_options()
        .open(w.slot().join("owner.json"))
        .map_err(Error::storage)?;
    marker
        .write_all(w.marker().to_string().as_bytes())
        .map_err(Error::storage)?;
    marker.sync_all().map_err(Error::storage)?;
    Ok(())
}

fn read_marker(path: &Path) -> Result<Value> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::guard(
                "workspace path already exists: missing ownership marker",
            ));
        }
        Err(error) => return Err(Error::storage(error)),
        Ok(_) => {}
    }
    let text = regular_text(path)?;
    serde_json::from_str(&text).map_err(|_| Error::guard("workspace ownership marker mismatch"))
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
    let marker = read_marker(&w.slot().join("owner.json"))?;
    if marker != w.marker() {
        return Err(Error::guard("workspace ownership marker mismatch"));
    }
    let path = w.slot().join("owner.lock");
    let expected = w
        .lease_identity
        .as_deref()
        .ok_or_else(|| Error::guard("missing lease identity"))?;
    check_identity(&path, expected)?;
    if !fs::symlink_metadata(&path).map_err(evidence_io)?.is_file() {
        return Err(Error::guard("invalid workspace lease"));
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .map_err(evidence_io)?;
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
    let log = open_creation_log(&w.slot().join("git-create.log"))?;
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

fn open_creation_log(path: &Path) -> Result<File> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => exclusive_file_options()
            .write(true)
            .open(path)
            .map_err(Error::storage),
        Err(error) => Err(Error::storage(error)),
        Ok(metadata) if metadata.is_file() => OpenOptions::new()
            .append(true)
            .open(path)
            .map_err(Error::storage),
        Ok(_) => Err(Error::guard("invalid workspace creation log")),
    }
}

/// True when Git registered this workspace or left a tree behind. A failed
/// `worktree add` that did neither can be repeated; partial evidence cannot.
pub(crate) fn creation_left_evidence(w: &WorkspaceRecord) -> Result<bool> {
    if fs::symlink_metadata(Path::new(&w.view.path)).is_ok() {
        return Ok(true);
    }
    let listing = git_bytes(
        Path::new(&w.source),
        &["worktree", "list", "--porcelain", "-z"],
    )?;
    let listing = String::from_utf8(listing).map_err(Error::storage)?;
    Ok(listing.split('\0').any(|field| {
        field
            .strip_prefix("worktree ")
            .is_some_and(|path| path == w.view.path)
    }))
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
    {
        return Err(Error::guard(
            "workspace binding, lock reason or frozen HEAD mismatch",
        ));
    }
    let relative_common = regular_text(&admin.join("commondir"))?;
    let linked_common = fs::canonicalize(admin.join(relative_common.trim_end_matches('\n')))
        .map_err(evidence_io)?;
    let tree_common = evidence_path(common_dir(&tree))?;
    let toplevel = evidence_git(&tree, &["rev-parse", "--show-toplevel"])?;
    let git_dir = evidence_git(&tree, &["rev-parse", "--absolute-git-dir"])?;
    if linked_common != Path::new(&w.common_dir)
        || tree_common != Path::new(&w.common_dir)
        || toplevel.trim_end_matches('\n') != w.view.path
        || git_dir.trim_end_matches('\n') != path_text(&admin)?
    {
        return Err(Error::guard("workspace repository backlink mismatch"));
    }
    // Ancestry runs only after the worktree is bound to this object database.
    // A missing commit or a foreign Git directory is evidence, not a retry.
    require_head(w, &admin, &tree)?;
    Ok((tree_identity, admin))
}

fn evidence_path(result: Result<PathBuf>) -> Result<PathBuf> {
    match result {
        Ok(path) => Ok(path),
        Err(Error::GitFailed { detail }) => Err(Error::guard(format!(
            "workspace evidence rejected: {detail}"
        ))),
        Err(error) => Err(error),
    }
}

fn evidence_git(path: &Path, args: &[&str]) -> Result<String> {
    match git(path, args) {
        Ok(text) => Ok(text),
        Err(Error::GitFailed { detail }) => Err(Error::guard(format!(
            "workspace evidence rejected: {detail}"
        ))),
        Err(error) => Err(error),
    }
}

fn evidence_git_bytes(path: &Path, args: &[&str]) -> Result<Vec<u8>> {
    match git_bytes(path, args) {
        Ok(bytes) => Ok(bytes),
        Err(Error::GitFailed { detail }) => Err(Error::guard(format!(
            "workspace evidence rejected: {detail}"
        ))),
        Err(error) => Err(error),
    }
}

/// `creating` still requires the admin HEAD to be the frozen base. Once
/// `ready`, later commits are ownership-preserving when HEAD stays detached
/// and descends from that base.
fn require_head(w: &WorkspaceRecord, admin: &Path, tree: &Path) -> Result<()> {
    let head = regular_text(&admin.join("HEAD"))?;
    let head = head.trim_end_matches('\n');
    if head == w.view.base_sha {
        return Ok(());
    }
    if w.view.state == "creating" || !is_commit_sha(head) {
        return Err(Error::guard(
            "workspace binding, lock reason or frozen HEAD mismatch",
        ));
    }
    // `cat-file -e <head>^{commit}` fails when HEAD is not a commit in this
    // object database. That is a binding mismatch, including merge-base's
    // exit 128 for a missing object.
    match git(tree, &["cat-file", "-e", &format!("{head}^{{commit}}")]) {
        Ok(_) => {}
        Err(Error::GitFailed { .. }) => {
            return Err(Error::guard(
                "workspace HEAD is not a commit in this repository",
            ));
        }
        Err(error) => return Err(error),
    }
    if base_is_ancestor(tree, &w.view.base_sha, head)? {
        Ok(())
    } else {
        Err(Error::guard(
            "workspace HEAD does not descend from the frozen base",
        ))
    }
}

fn is_commit_sha(value: &str) -> bool {
    [40, 64].contains(&value.len()) && value.bytes().all(|c| c.is_ascii_hexdigit())
}

fn base_is_ancestor(repo: &Path, base: &str, head: &str) -> Result<bool> {
    let status = command(repo)?
        .args(["merge-base", "--is-ancestor", base, head])
        .status()
        .map_err(Error::storage)?;
    match status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(Error::guard(format!(
            "workspace HEAD is not a commit in this repository ({status})"
        ))),
    }
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
    // A corrupt index or missing frozen tree is evidence, not a transient
    // Git failure that would be retried forever.
    evidence_git_bytes(tree, &["ls-files", "--stage", "-z"])?;
    if evidence_git(
        tree,
        &["cat-file", "-t", &format!("{}^{{tree}}", w.view.base_sha)],
    )?
    .trim()
        != "tree"
    {
        return Err(Error::guard("frozen workspace tree is unavailable"));
    }
    if w.view.state == "creating"
        && !evidence_git_bytes(
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

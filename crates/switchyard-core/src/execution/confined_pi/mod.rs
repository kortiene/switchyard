//! Non-default, credential-free bridge lifecycle. The caller supplies trusted
//! Host configuration; neither protocol frames nor model text select paths.
mod egress;
mod launcher;
mod policy;
mod retention;
mod storage;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::owner::try_acquire_file;
use super::{PiBridge, PiEffectOutcome, RuntimeBinding};
use crate::linux_execution::cgroup::{self, CgroupIdentity};
use crate::{Error, Result, payload_hash};
use storage::Directory;

pub use egress::ModelEndpoint;
pub use launcher::confined_pi_launcher_main;

pub(crate) fn runtime_digest(runtime: &Path) -> Result<String> {
    Ok(policy::inspect_runtime(runtime)?.digest)
}

/// Diagnostic network field. Unset stays the string `"denied"` so a host that
/// did not opt in does not look like it gained a route.
pub(crate) fn egress_network(config: &ConfinedPiConfig) -> Value {
    match &config.model_endpoint {
        None => json!("denied"),
        Some(endpoint) => egress::record(Some(endpoint)),
    }
}

/// Conformance wall for a direct session open, and the ceiling for one
/// forwarded effect (half of this, matching the 60s Linux ledger). Host
/// sessions that have a freeze use the frozen generation wall instead.
/// The effect deadline and policy `wallSeconds` stay on this constant.
pub(crate) const BRIDGE_WALL: Duration = Duration::from_secs(60);

/// Trusted installation paths. `root` must already be a private (0700) directory
/// outside workspaces; it is never mounted into the bridge. Linux x86_64 only.
#[derive(Clone, Debug)]
pub struct ConfinedPiConfig {
    pub root: PathBuf,
    pub cgroup_root: PathBuf,
    /// Absolute, owner-private runtime install. It must be canonical, sit
    /// outside workspaces, and is mounted read-only: the bridge never receives
    /// a writable path to it. Symlinked path components and setid or
    /// group/world-writable parts are refused before launch.
    pub runtime: PathBuf,
    pub node: PathBuf,
    pub bubblewrap: PathBuf,
    pub launcher: PathBuf,
    pub policy: PathBuf,
    /// Operator-selected credential-free HTTP model URL; HTTPS is not supported.
    /// `None` keeps the empty network namespace
    /// and the full socket deny. Repository files and protocol frames cannot
    /// set this.
    pub model_endpoint: Option<ModelEndpoint>,
}

pub struct ConfinedPiSession {
    bridge: PiBridge,
    control: Directory,
    record: Value,
    leaf: CgroupIdentity,
    watchdog: Option<(Sender<()>, JoinHandle<()>)>,
    /// Owns the thread that spawned the launcher. Parent-death signals follow
    /// that thread, so it stays alive until [`Self::stop`] has reaped the child.
    launch_supervisor: Option<LaunchSupervisor>,
    /// Host-side splice to the pinned model address. Absent when egress is denied.
    proxy: Option<egress::ModelProxy>,
    stopped: bool,
    opened_at: Instant,
    wall_exhausted: Arc<AtomicBool>,
    /// Frozen per-mission model-request cap. Held for `ModelProxy::start`.
    model_request_cap: u64,
}

/// Wall and the frozen per-mission model-request cap for one bridge open.
pub(crate) struct OpenLimits {
    pub wall: Duration,
    /// Owner: "40 requests is the frozen per-mission cap".
    pub model_request_cap: u64,
}

/// Construction site for the host splice. Does not count connections.
///
/// `d1b-splice-timeout-connection-cap` merges first and changes
/// `ModelProxy::start` to `start(endpoint, allowance: u32)`. That PR passes
/// `egress::UNWIRED_CONNECTION_ALLOWANCE` (40) and labels the record
/// `"scope":"generation"`. The proxy counts a connection once the upstream
/// dial succeeds, and sets `exhausted()` past the allowance.
///
/// TODO(d1b-splice-timeout-connection-cap): when #229 is on main, this
/// interim call is `ModelProxy::start(endpoint, egress::UNWIRED_CONNECTION_ALLOWANCE)`.
/// Scope is a constructor input; pass `"mission"`. Wiring that replaces the
/// constant, due before GA:
/// allowance = frozen cap minus every recorded connection across prior
/// generations, saturating at 0. `u64` to `u32` via `try_from`, and 0 on
/// overflow. Persist each generation's final `modelEgress` durably before the
/// next generation opens. `current.json` and the `pi_host_sessions` row are
/// overwritten, so they are not that record. Fail closed at allowance 0, not
/// 40, when a prior generation had egress but no recorded count, and when the
/// mission has no frozen budget. Old ledgers keep cancel and get no new model
/// egress. After wiring, `UNWIRED_CONNECTION_ALLOWANCE` has zero uses. Freeze
/// `maxRetries` 0. Do not edit `egress.rs` in this slice.
fn start_model_proxy(
    endpoint: &egress::ModelEndpoint,
    model_request_cap: u64,
) -> Result<egress::ModelProxy> {
    // Held for the post-merge call `ModelProxy::start(endpoint, allowance)`.
    let _frozen_per_mission_cap = model_request_cap;
    egress::ModelProxy::start(endpoint)
}

impl ConfinedPiSession {
    /// Open/recover one original Pi session inert. Generation must increase on
    /// every reopen. Recovery kills and proves the recorded leaf empty before
    /// inspecting bridge-writable state or launching a replacement.
    ///
    /// The launcher is spawned on a supervisor thread owned by this session.
    /// Linux `PR_SET_PDEATHSIG` and bubblewrap `--die-with-parent` follow the
    /// thread that created the child. The supervisor stays parked until
    /// [`Self::stop`], so `open` may run on a thread that ends first.
    pub fn open(config: &ConfinedPiConfig, binding: RuntimeBinding, fixture: &str) -> Result<Self> {
        Self::open_required(
            config,
            binding,
            fixture,
            false,
            None,
            cfg!(test),
            OpenLimits {
                wall: BRIDGE_WALL,
                model_request_cap: crate::budget::MAX_MODEL_REQUESTS,
            },
        )
    }

    pub(crate) fn open_required(
        config: &ConfinedPiConfig,
        binding: RuntimeBinding,
        fixture: &str,
        require_original: bool,
        // Digest recorded when the host ledger was opened. None for a direct
        // session open that has no attempt ledger yet.
        expected_digest: Option<&str>,
        test_fixtures: bool,
        limits: OpenLimits,
    ) -> Result<Self> {
        validate_fixture(&binding, fixture, test_fixtures || cfg!(test))?;
        let root = Directory::root(&config.root)?;
        let key = payload_hash(&identity(&binding))?;
        let control = root.child(&format!("session-{key}"))?;
        let owner = try_acquire_file(control.open_file("owner.lock", true)?)?;
        let state = control.child("state")?;
        let installed = policy::check(config)?;
        // Refuse a swapped install before recovery rewrites control state.
        if expected_digest.is_some_and(|expected| installed["runtimeRoot"]["digest"] != expected) {
            return Err(Error::guard(
                "runtime root does not match the host ledger pin",
            ));
        }
        let cgroup_root = cgroup::validate_delegation(&config.cgroup_root)?;
        let previous = control.read("current.json")?;
        if require_original && previous.is_none() {
            return Err(Error::guard(
                "original confined bridge control record is missing",
            ));
        }
        let mut require_existing = false;
        let mut recovered = Value::Null;
        if let Some(previous) = previous.as_ref() {
            let configuration_changed = previous["configuration"] != installed;
            if previous["version"] != 1
                || previous["identity"] != identity(&binding)
                // Emptiness is checked after the old cgroup is proven dead.
                || (configuration_changed && !proven_unopened_stop(previous))
                || previous["requiresExisting"].as_bool().is_none()
                || !matches!(
                    previous["phase"].as_str(),
                    Some("prepared" | "released" | "opened" | "stopped" | "uncertain")
                )
                || previous["generation"]
                    .as_u64()
                    .is_none_or(|g| g == 0 || g >= binding.generation)
            {
                return Err(Error::guard(
                    "incompatible or stale confined bridge generation",
                ));
            }
            let old = recorded_leaf(previous, &cgroup_root)?;
            // Only the dead Host could place the launch barrier into this leaf.
            // The child never self-joins, so populated 0 cannot precede a late join.
            let proof = cgroup::rekill_and_prove(&old, Duration::from_secs(5))?;
            if !proof.proven {
                return Err(Error::guard(format!(
                    "previous bridge is not quiescent: {}",
                    proof.reason
                )));
            }
            // Quiescence first, so a live bridge cannot create session.sqlite
            // after this look. A corrected runtime may replace the recorded one
            // only when that directory is still empty.
            if !allows_configuration(previous, &installed, state.is_empty()?) {
                return Err(Error::guard(
                    "incompatible or stale confined bridge generation",
                ));
            }
            // A second crash during preparation must retain the original
            // database requirement from an earlier generation. A proven stop
            // that never reached "opened" has no history to keep.
            require_existing = require_existing_database(previous);
            recovered = json!({"generation":previous["generation"],"cgroup":previous["cgroup"],
                "proven":true,"events":proof.events});
            if configuration_changed {
                recovered["replacedConfiguration"] = previous["configuration"].clone();
            }
            control.write(
                &format!("recovered-{}.json", previous["generation"]),
                &recovered,
            )?;
        } else if fs::read_dir(&state.path)
            .map_err(Error::storage)?
            .next()
            .is_some()
        {
            return Err(Error::guard("unowned Pi storage cannot be adopted"));
        }
        retention::recover(&control, &cgroup_root, &binding, previous.as_ref())?;
        state.inspect_state(require_existing)?;
        let proxy = match &config.model_endpoint {
            Some(endpoint) => Some(start_model_proxy(endpoint, limits.model_request_cap)?),
            None => None,
        };
        let argv = policy::argv(
            config,
            &state.path,
            proxy.as_ref().map(|proxy| proxy.directory()),
            &installed,
        )?;
        let leaf = retention::allocate(&control, &cgroup_root, &binding)?;
        let mut record = json!({"version":1,"identity":identity(&binding),"generation":binding.generation,
            "configuration":installed,"phase":"prepared","recovered":recovered,"requiresExisting":require_existing,
            "cgroup":{"path":leaf.path,"dev":leaf.dev,"ino":leaf.ino,"boot":leaf.boot_id}});
        control.write("current.json", &record)?;
        // Only this durable handoff makes the former current leaf obsolete.
        // Settle allocation/retirement journals before any launcher can start.
        retention::recover(&control, &cgroup_root, &binding, Some(&record))?;
        let mut command = Command::new(&config.launcher);
        command
            .arg(std::process::id().to_string())
            .arg(&leaf.path)
            .arg("--")
            .args(&argv)
            .arg(binding_value(&binding).to_string())
            .arg(fixture)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let (child, launch_supervisor) = spawn_supervised(command)?;
        let mut pending = PendingLaunch {
            child: Some(child),
            supervisor: Some(launch_supervisor),
        };
        let child = pending.child.as_mut().expect("spawned child");
        record["supervisor"] = json!({"pid":child.id(),"start":start_time(child.id())?});
        control.write("current.json", &record)?;
        // A direct unreaped child cannot have its PID recycled. Migration is
        // performed by this Host, never by the launcher after a restart.
        fs::write(leaf.path.join("cgroup.procs"), child.id().to_string())
            .map_err(Error::storage)?;
        record["phase"] = json!("released");
        control.write("current.json", &record)?;
        child
            .stdin
            .as_mut()
            .expect("piped input")
            .write_all(b"GO\n")
            .map_err(Error::storage)?;
        let bridge =
            PiBridge::from_child(pending.child.take().expect("spawned child"), binding, owner)?;
        let launch_supervisor = pending.supervisor.take().expect("launch supervisor");
        let (done, wait) = channel();
        let watched = leaf.clone();
        let wall_exhausted = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&wall_exhausted);
        let wall = limits.wall;
        let watchdog = std::thread::spawn(move || {
            if matches!(
                wait.recv_timeout(wall),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ) {
                flag.store(true, Ordering::SeqCst);
                // Same cgroup proof the stop path uses. Not a second teardown:
                // the generation wall already killed here, and `stop` proves again.
                let _ = cgroup::rekill_and_prove(&watched, Duration::from_secs(5));
            }
        });
        let mut session = Self {
            bridge,
            control,
            record,
            leaf,
            watchdog: Some((done, watchdog)),
            launch_supervisor: Some(launch_supervisor),
            proxy,
            stopped: false,
            opened_at: Instant::now(),
            wall_exhausted,
            model_request_cap: limits.model_request_cap,
        };
        session.bridge.hello()?;
        let opened = session.bridge.call(
            "runtime.open",
            None,
            json!({"requireExisting":require_existing}),
            &mut |_| Err(Error::guard("open cannot execute effects")),
        )?;
        if opened["opened"] != true || opened["scheduling"] != "paused" {
            return Err(Error::guard("confined bridge did not open inert"));
        }
        session.record["phase"] = json!("opened");
        session.record["requiresExisting"] = json!(true);
        session.control.write("current.json", &session.record)?;
        Ok(session)
    }

    pub fn binding(&self) -> &RuntimeBinding {
        self.bridge.binding()
    }

    /// Time since this generation was released. The frozen bridge wall is
    /// measured from here, not from `mission verify`.
    pub fn generation_elapsed(&self) -> Duration {
        self.opened_at.elapsed()
    }

    pub fn wall_exhausted(&self) -> bool {
        self.wall_exhausted.load(Ordering::SeqCst)
    }

    /// Frozen per-mission model-request cap for this generation.
    ///
    /// Owner: "40 requests is the frozen per-mission cap".
    /// [`start_model_proxy`] holds it until `d1b-splice-timeout-connection-cap`
    /// passes the remaining mission allowance into `ModelProxy::start`.
    pub fn model_request_cap(&self) -> u64 {
        self.model_request_cap
    }

    /// A protocol abort receipt is not a bridge termination proof. `stop` owns
    /// that proof. The trusted caller continues to own workspace effect receipts.
    pub fn call(
        &mut self,
        binding: &RuntimeBinding,
        method: &str,
        command: Option<&str>,
        params: Value,
        effect: &mut dyn FnMut(&Value) -> PiEffectOutcome,
    ) -> Result<Value> {
        if self.stopped || self.binding() != binding {
            return Err(Error::guard("stale or stopped confined bridge"));
        }
        if !matches!(
            method,
            "runtime.submit"
                | "runtime.steer"
                | "runtime.inspect"
                | "runtime.watch"
                | "runtime.abort"
        ) {
            return Err(Error::guard("bridge lifecycle method is owned by Rust"));
        }
        let result = self
            .bridge
            .call_with_effects(method, command, params, effect);
        // The bridge exit is recorded either way. A failed call keeps its own
        // error; only a call that succeeded surfaces a later stop failure.
        match self.bridge.exit_status() {
            Ok(Some(_)) => {
                let stopped = self.stop();
                if result.is_ok() {
                    stopped?;
                }
            }
            Ok(None) => {}
            Err(error) => {
                if result.is_ok() {
                    return Err(Error::storage(error));
                }
            }
        }
        result
    }

    /// The session has already run `stop`. This is the in-process fact.
    /// `termination.proven` is not: a later cancel re-proves when that flag
    /// is false.
    pub fn is_stopped(&self) -> bool {
        self.stopped
    }

    /// Record an unproven stop without `rekill_and_prove`. The confinement
    /// probe uses this so a later cancel in the same process has to re-prove.
    /// `cfg(test)` is not set when that probe builds this crate.
    #[cfg(feature = "test-fixtures")]
    pub fn settle_unproven_for_test(&mut self) -> Result<()> {
        if let Some((done, watchdog)) = self.watchdog.take() {
            let _ = done.send(());
            let _ = watchdog.join();
        }
        self.bridge.terminate();
        self.proxy.take();
        self.record["phase"] = json!("uncertain");
        self.record["termination"] = json!({
            "proven": false,
            "reason": "nonempty",
            "events": "",
            "generation": self.binding().generation,
            "cgroup": self.record["cgroup"]
        });
        self.control.write("current.json", &self.record)?;
        self.stopped = true;
        if let Some(mut supervisor) = self.launch_supervisor.take() {
            let _ = supervisor.shutdown();
        }
        Ok(())
    }

    /// The next `stop` re-proof cannot see this leaf, so it stays unproven.
    #[cfg(feature = "test-fixtures")]
    pub fn miss_leaf_for_test(&mut self) {
        self.leaf.path = std::path::PathBuf::from("/tmp/switchyard-missing-bridge-leaf");
        self.leaf.dev = 0;
        self.leaf.ino = 0;
        self.leaf.boot_id = "missing".into();
    }

    /// Reap the direct child and prove the recorded leaf empty.
    ///
    /// A later call always runs `rekill_and_prove` again. The fresh result
    /// decides; a stored `proven` flag is not reused.
    pub fn stop(&mut self) -> Result<Value> {
        if self.stopped {
            return self.reprove_stopped_leaf();
        }
        if let Some((done, watchdog)) = self.watchdog.take() {
            let _ = done.send(());
            watchdog
                .join()
                .map_err(|_| Error::guard("bridge watchdog failed"))?;
        }
        self.bridge.terminate();
        // TODO(d1b-splice-timeout-connection-cap): #229 writes `modelEgress`
        // after the accept thread joins and before this drop. Keep that copy.
        // Delete this TODO in the merge. Pass scope `"mission"`. Counts are
        // allowance, accepted, refused, scope, and reason
        // `runtime_budget_exhausted` when `exhausted()`. No new Error variant.
        // Exhaustion admits `detect_uncertainty` and does not admit
        // `request_verification`. The `modelEgress` object has to be stored
        // somewhere `current.json` and `pi_host_sessions` cannot overwrite
        // before the next generation opens.
        // Drop the proxy only after the bridge is dead so an in-flight splice
        // is not the thing that decides termination.
        self.proxy.take();
        let proof = cgroup::rekill_and_prove(&self.leaf, Duration::from_secs(5))?;
        self.record["phase"] = json!(if proof.proven { "stopped" } else { "uncertain" });
        let evidence = json!({"proven":proof.proven,"reason":proof.reason,"events":proof.events,
            "generation":self.binding().generation,"cgroup":self.record["cgroup"]});
        self.record["termination"] = evidence.clone();
        self.control.write("current.json", &self.record)?;
        // Set before joining the supervisor so a join failure cannot make Drop
        // re-kill and overwrite this record.
        self.stopped = true;
        if let Some(mut supervisor) = self.launch_supervisor.take() {
            let joined = supervisor.shutdown();
            if proof.proven {
                joined?;
            }
        }
        if !proof.proven {
            return Err(Error::guard("confined bridge termination remains unproven"));
        }
        Ok(evidence)
    }

    /// Fresh `rekill_and_prove` after an unproven stop in this process.
    /// Returns the new evidence even when it is still unproven, so cancel can
    /// refuse to confirm without treating the old flag as the decision.
    fn reprove_stopped_leaf(&mut self) -> Result<Value> {
        let proof = cgroup::rekill_and_prove(&self.leaf, Duration::from_secs(5))?;
        self.record["phase"] = json!(if proof.proven { "stopped" } else { "uncertain" });
        let evidence = json!({"proven":proof.proven,"reason":proof.reason,"events":proof.events,
            "generation":self.binding().generation,"cgroup":self.record["cgroup"]});
        self.record["termination"] = evidence.clone();
        self.control.write("current.json", &self.record)?;
        Ok(evidence)
    }

    pub fn stderr_tail(&self) -> String {
        self.bridge.stderr_tail()
    }

    /// Host-only observation, useful for diagnostics and independently checking
    /// cgroup limits. It does not certify that any workspace effect succeeded.
    pub fn lifecycle(&self) -> Value {
        self.record.clone()
    }
}

impl Drop for ConfinedPiSession {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

struct PendingLaunch {
    child: Option<Child>,
    supervisor: Option<LaunchSupervisor>,
}

impl Drop for PendingLaunch {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        // Join only after the child is dead so this thread's exit is not what
        // delivers parent-death SIGKILL.
        self.supervisor.take();
    }
}

/// Thread that forked the launcher. It parks until the session stops.
struct LaunchSupervisor {
    shutdown: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl LaunchSupervisor {
    fn shutdown(&mut self) -> Result<()> {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .map_err(|_| Error::guard("bridge supervisor failed"))?;
        }
        Ok(())
    }
}

impl Drop for LaunchSupervisor {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn spawn_supervised(mut command: Command) -> Result<(Child, LaunchSupervisor)> {
    let (started, ready) = channel();
    let (shutdown, stop) = channel();
    let thread = std::thread::Builder::new()
        .name("confined-pi-supervisor".into())
        .spawn(move || {
            let spawned = command.spawn();
            let failed = spawned.is_err();
            if let Err(std::sync::mpsc::SendError(Ok(mut child))) = started.send(spawned) {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
            if !failed {
                let _ = stop.recv();
            }
        })
        .map_err(Error::storage)?;
    match ready.recv() {
        Ok(Ok(child)) => Ok((
            child,
            LaunchSupervisor {
                shutdown: Some(shutdown),
                thread: Some(thread),
            },
        )),
        Ok(Err(error)) => {
            let _ = thread.join();
            Err(Error::storage(error))
        }
        Err(_) => {
            let _ = thread.join();
            Err(Error::guard("bridge supervisor exited before spawn"))
        }
    }
}

/// Proven stop whose record never reached `"opened"`.
fn proven_unopened_stop(previous: &Value) -> bool {
    previous["phase"] == "stopped"
        && previous["termination"]["proven"] == true
        && previous["requiresExisting"] == false
}

/// `true` when recovery must find the original database.
///
/// A proven stop whose record never reached `"opened"` accepted no command.
/// Crashes and unproven stops after release stay conservative, and an earlier
/// generation's requirement survives a crash while the next one is only
/// `"prepared"`.
fn require_existing_database(previous: &Value) -> bool {
    !proven_unopened_stop(previous)
        && (previous["requiresExisting"] == true || previous["phase"] != "prepared")
}

/// A different runtime is allowed only for a proven stop that never opened
/// while `state/` is still empty. The same configuration is always eligible
/// here; database retention is a separate check.
fn allows_configuration(previous: &Value, installed: &Value, state_empty: bool) -> bool {
    previous["configuration"] == *installed || (proven_unopened_stop(previous) && state_empty)
}

fn identity(binding: &RuntimeBinding) -> Value {
    json!([binding.mission_id, binding.attempt_id, binding.session_id])
}

fn binding_value(binding: &RuntimeBinding) -> Value {
    json!({"missionId":binding.mission_id,"attemptId":binding.attempt_id,
        "sessionId":binding.session_id,"generation":binding.generation})
}

fn production_session_fixture(fixture: &str) -> bool {
    matches!(
        fixture,
        "answer"
            | "recover"
            | "ignore-abort"
            | "file-write"
            | "process-fast"
            | "process-hold"
            | "process-cancel"
            | "tools"
            | "s3-acceptance"
    )
}

fn session_fixture_allowed(fixture: &str, test_fixtures: bool) -> bool {
    production_session_fixture(fixture)
        || (test_fixtures && crate::execution::is_busy_once_fixture(fixture))
}

#[cfg(test)]
fn validate(binding: &RuntimeBinding, fixture: &str) -> Result<()> {
    validate_fixture(binding, fixture, true)
}

fn validate_fixture(binding: &RuntimeBinding, fixture: &str, test_fixtures: bool) -> Result<()> {
    if !session_fixture_allowed(fixture, test_fixtures)
        || binding.generation == 0
        || binding.generation > 9_007_199_254_740_991
        || [
            &binding.mission_id,
            &binding.attempt_id,
            &binding.session_id,
        ]
        .iter()
        .any(|s| s.is_empty() || s.len() > 256)
    {
        return Err(Error::guard(
            "invalid confined bridge binding or credential-free fixture",
        ));
    }
    Ok(())
}

fn recorded_leaf(record: &Value, root: &Path) -> Result<CgroupIdentity> {
    let field = &record["cgroup"];
    let path = PathBuf::from(
        field["path"]
            .as_str()
            .ok_or_else(|| Error::guard("missing bridge cgroup"))?,
    );
    if path.parent() != Some(root)
        || !path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("bridge-"))
    {
        return Err(Error::guard(
            "recorded bridge cgroup is outside the delegated root",
        ));
    }
    Ok(CgroupIdentity {
        path,
        dev: field["dev"]
            .as_u64()
            .ok_or_else(|| Error::guard("missing cgroup device"))?,
        ino: field["ino"]
            .as_u64()
            .ok_or_else(|| Error::guard("missing cgroup inode"))?,
        boot_id: field["boot"]
            .as_str()
            .ok_or_else(|| Error::guard("missing cgroup boot"))?
            .into(),
    })
}

#[cfg(test)]
pub(crate) fn fixture_runtime(parent: &Path, marker: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    const PINS: &str = include_str!("../../../../../runtime/pi-source/pins.json");
    let root = parent.join(format!("pi-runtime-{marker}"));
    let _ = fs::remove_dir_all(&root);
    for directory in ["src", "test", "node_modules", "confinement", ".artifacts"] {
        fs::create_dir_all(root.join(directory)).unwrap();
    }
    for (rel, bytes) in policy::reviewed_files() {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, bytes).unwrap();
    }
    // `node_modules` is not compile-time pinned. The marker keeps two fixtures
    // distinct so a swapped tree changes the runtime-root digest.
    fs::write(root.join("node_modules/keep.txt"), marker).unwrap();
    fs::write(root.join("pins.json"), PINS).unwrap();
    let pins: Value = serde_json::from_str(PINS).unwrap();
    let provenance = json!({
        "decision": "NEEDS-FULL-S2",
        "sourceCommit": pins["commit"],
        "upstreamLockSha256": pins["lockSha256"],
        "catalogRevision": pins["catalogRevision"],
        "node": pins["node"],
        "npm": pins["npm"],
        "packages": pins["packages"],
    });
    fs::write(
        root.join(".artifacts/provenance.json"),
        serde_json::to_vec_pretty(&provenance).unwrap(),
    )
    .unwrap();
    fn tighten(path: &Path, root: &Path) {
        let meta = fs::symlink_metadata(path).unwrap();
        if meta.file_type().is_symlink() {
            return;
        }
        if meta.is_dir() {
            let mode = if path == root { 0o700 } else { 0o755 };
            fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
            for entry in fs::read_dir(path).unwrap() {
                tighten(&entry.unwrap().path(), root);
            }
        } else {
            fs::set_permissions(path, fs::Permissions::from_mode(0o644)).unwrap();
        }
    }
    tighten(&root, &root);
    root
}

fn start_time(pid: u32) -> Result<String> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).map_err(Error::storage)?;
    stat.rsplit_once(") ")
        .and_then(|(_, fields)| fields.split_whitespace().nth(19))
        .map(str::to_owned)
        .ok_or_else(|| Error::guard("invalid launcher process identity"))
}

#[cfg(test)]
mod tests;

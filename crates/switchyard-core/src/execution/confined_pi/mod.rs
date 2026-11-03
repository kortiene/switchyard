//! Non-default, credential-free bridge lifecycle. The caller supplies trusted
//! Host configuration; neither protocol frames nor model text select paths.
mod launcher;
mod policy;
mod storage;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Sender, channel};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::{Value, json};

use super::owner::try_acquire_file;
use super::{PiBridge, PiEffectOutcome, RuntimeBinding};
use crate::linux_execution::cgroup::{self, CgroupIdentity};
use crate::{Error, Result, payload_hash};
use storage::Directory;

pub use launcher::confined_pi_launcher_main;

/// Trusted installation paths. `root` must already be a private (0700) directory
/// outside workspaces; it is never mounted into the bridge. Linux x86_64 only.
#[derive(Clone, Debug)]
pub struct ConfinedPiConfig {
    pub root: PathBuf,
    pub cgroup_root: PathBuf,
    pub node: PathBuf,
    pub bubblewrap: PathBuf,
    pub launcher: PathBuf,
    pub policy: PathBuf,
}

pub struct ConfinedPiSession {
    bridge: PiBridge,
    control: Directory,
    record: Value,
    leaf: CgroupIdentity,
    watchdog: Option<(Sender<()>, JoinHandle<()>)>,
    stopped: bool,
}

impl ConfinedPiSession {
    /// Open/recover one original Pi session inert. Generation must increase on
    /// every reopen. Recovery kills and proves the recorded leaf empty before
    /// inspecting bridge-writable state or launching a replacement.
    pub fn open(config: &ConfinedPiConfig, binding: RuntimeBinding, fixture: &str) -> Result<Self> {
        validate(&binding, fixture)?;
        let root = Directory::root(&config.root)?;
        let key = payload_hash(&identity(&binding))?;
        let control = root.child(&format!("session-{key}"))?;
        let owner = try_acquire_file(control.open_file("owner.lock", true)?)?;
        let state = control.child("state")?;
        let installed = policy::check(config)?;
        let cgroup_root = cgroup::validate_delegation(&config.cgroup_root)?;
        let previous = control.read("current.json")?;
        let mut require_existing = false;
        let mut recovered = Value::Null;
        if let Some(previous) = previous {
            if previous["version"] != 1
                || previous["identity"] != identity(&binding)
                || previous["configuration"] != installed
                || previous["generation"]
                    .as_u64()
                    .is_none_or(|g| g >= binding.generation)
            {
                return Err(Error::guard(
                    "incompatible or stale confined bridge generation",
                ));
            }
            let old = recorded_leaf(&previous, &cgroup_root)?;
            // Only the dead Host could place the launch barrier into this leaf.
            // The child never self-joins, so populated 0 cannot precede a late join.
            let proof = cgroup::rekill_and_prove(&old, Duration::from_secs(5))?;
            if !proof.proven {
                return Err(Error::guard(format!(
                    "previous bridge is not quiescent: {}",
                    proof.reason
                )));
            }
            require_existing = previous["phase"] != "prepared";
            recovered = json!({"generation":previous["generation"],"cgroup":previous["cgroup"],
                "proven":true,"events":proof.events});
            control.write(
                &format!("recovered-{}.json", previous["generation"]),
                &recovered,
            )?;
        } else if state.exists("session.sqlite")? {
            return Err(Error::guard("unowned Pi storage cannot be adopted"));
        }
        state.inspect_state(require_existing)?;
        let argv = policy::argv(config, &state.path)?;
        let leaf =
            cgroup::create_bridge_leaf(&cgroup_root, &format!("bridge-{}", uuid::Uuid::new_v4()))?;
        let mut record = json!({"version":1,"identity":identity(&binding),"generation":binding.generation,
            "configuration":installed,"phase":"prepared","recovered":recovered,
            "cgroup":{"path":leaf.path,"dev":leaf.dev,"ino":leaf.ino,"boot":leaf.boot_id}});
        control.write("current.json", &record)?;
        let mut command = Command::new(&config.launcher);
        command
            .arg(&leaf.path)
            .arg("--")
            .args(&argv)
            .arg(binding_value(&binding).to_string())
            .arg(fixture)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut pending = PendingChild(Some(command.spawn().map_err(Error::storage)?));
        let child = pending.0.as_mut().expect("spawned child");
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
            PiBridge::from_child(pending.0.take().expect("spawned child"), binding, owner)?;
        let (done, wait) = channel();
        let watched = leaf.clone();
        let watchdog = std::thread::spawn(move || {
            if matches!(
                wait.recv_timeout(Duration::from_secs(60)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ) {
                let _ = cgroup::rekill_and_prove(&watched, Duration::from_secs(5));
            }
        });
        let mut session = Self {
            bridge,
            control,
            record,
            leaf,
            watchdog: Some((done, watchdog)),
            stopped: false,
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
        session.control.write("current.json", &session.record)?;
        Ok(session)
    }

    pub fn binding(&self) -> &RuntimeBinding {
        self.bridge.binding()
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
        if self.bridge.exit_status().map_err(Error::storage)?.is_some() {
            self.stop()?;
        }
        result
    }

    pub fn stop(&mut self) -> Result<Value> {
        if let Some((done, watchdog)) = self.watchdog.take() {
            let _ = done.send(());
            watchdog
                .join()
                .map_err(|_| Error::guard("bridge watchdog failed"))?;
        }
        self.bridge.terminate();
        let proof = cgroup::rekill_and_prove(&self.leaf, Duration::from_secs(5))?;
        self.stopped = true;
        self.record["phase"] = json!(if proof.proven { "stopped" } else { "uncertain" });
        let evidence = json!({"proven":proof.proven,"reason":proof.reason,"events":proof.events,
            "generation":self.binding().generation,"cgroup":self.record["cgroup"]});
        self.record["termination"] = evidence.clone();
        self.control.write("current.json", &self.record)?;
        if !proof.proven {
            return Err(Error::guard("confined bridge termination remains unproven"));
        }
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

struct PendingChild(Option<Child>);
impl Drop for PendingChild {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn identity(binding: &RuntimeBinding) -> Value {
    json!([binding.mission_id, binding.attempt_id, binding.session_id])
}

fn binding_value(binding: &RuntimeBinding) -> Value {
    json!({"missionId":binding.mission_id,"attemptId":binding.attempt_id,
        "sessionId":binding.session_id,"generation":binding.generation})
}

fn validate(binding: &RuntimeBinding, fixture: &str) -> Result<()> {
    if !matches!(fixture, "answer" | "recover" | "ignore-abort")
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

fn start_time(pid: u32) -> Result<String> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).map_err(Error::storage)?;
    stat.rsplit_once(") ")
        .and_then(|(_, fields)| fields.split_whitespace().nth(19))
        .map(str::to_owned)
        .ok_or_else(|| Error::guard("invalid launcher process identity"))
}

#[cfg(test)]
mod tests;

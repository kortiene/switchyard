//! Owner-lock-only cgroup retention. Lifecycle records remain the recovery
//! authority; retirement receipts authorize absence of obsolete leaves only.
use std::fs;
use std::path::Path;

use serde_json::{Value, json};

use super::storage::Directory;
use super::{RuntimeBinding, identity, recorded_leaf};
use crate::linux_execution::cgroup::{self, CgroupIdentity, Quiescence};
use crate::{Error, Result, payload_hash};

const ALLOCATION: &str = "leaf-allocation.json";
const RETIREMENT: &str = "leaf-retirement.json";
const CLEANUP_BATCH: usize = 8;

trait Leaves {
    fn exists(&self, path: &Path) -> Result<bool>;
    fn observe(&self, leaf: &CgroupIdentity) -> Result<Quiescence>;
    fn remove_empty(&self, leaf: &CgroupIdentity) -> Result<Quiescence>;
}

struct Kernel;

impl Leaves for Kernel {
    fn exists(&self, path: &Path) -> Result<bool> {
        match fs::symlink_metadata(path) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(Error::storage(error)),
        }
    }

    fn observe(&self, leaf: &CgroupIdentity) -> Result<Quiescence> {
        cgroup::observe(leaf)
    }

    fn remove_empty(&self, leaf: &CgroupIdentity) -> Result<Quiescence> {
        cgroup::remove_empty_leaf(leaf)
    }
}

/// Called with the session owner lock held, after proving the current leaf
/// empty. Never removes that leaf, even when its stop record is proven.
pub(super) fn recover(
    control: &Directory,
    root: &Path,
    binding: &RuntimeBinding,
    current: Option<&Value>,
) -> Result<()> {
    Cleanup {
        control,
        root,
        binding,
        current,
        leaves: &Kernel,
    }
    .recover()
}

/// An intent precedes mkdir. A crash before identity persistence blocks further
/// allocation if the path exists: its identity must not be guessed or adopted.
pub(super) fn allocate(
    control: &Directory,
    root: &Path,
    binding: &RuntimeBinding,
) -> Result<CgroupIdentity> {
    allocate_with(control, root, binding, cgroup::create_bridge_leaf)
}

fn allocate_with(
    control: &Directory,
    root: &Path,
    binding: &RuntimeBinding,
    create: impl FnOnce(&Path, &str) -> Result<CgroupIdentity>,
) -> Result<CgroupIdentity> {
    if control.exists(ALLOCATION)? || control.exists(RETIREMENT)? {
        return Err(Error::guard(
            "bridge leaf cleanup must settle before allocation",
        ));
    }
    let name = format!("bridge-{}", uuid::Uuid::new_v4());
    let mut intent = json!({"version":1,"identity":identity(binding),
        "generation":binding.generation,"path":root.join(&name),"cgroup":null});
    control.write(ALLOCATION, &intent)?;
    let leaf = create(root, &name)?;
    intent["cgroup"] = leaf_value(&leaf);
    control.write(ALLOCATION, &intent)?;
    Ok(leaf)
}

pub(super) fn leaf_value(leaf: &CgroupIdentity) -> Value {
    json!({"path":leaf.path,"dev":leaf.dev,"ino":leaf.ino,"boot":leaf.boot_id})
}

struct Cleanup<'a, L> {
    control: &'a Directory,
    root: &'a Path,
    binding: &'a RuntimeBinding,
    current: Option<&'a Value>,
    leaves: &'a L,
}

impl<L: Leaves> Cleanup<'_, L> {
    fn recover(&self) -> Result<()> {
        // A deletion may have committed before either journal was cleared.
        self.resume_retirement()?;
        self.recover_allocation()?;
        self.collect_history()
    }

    fn generation(&self, value: &Value) -> Result<u64> {
        value
            .as_u64()
            .filter(|generation| *generation > 0 && *generation <= self.binding.generation)
            .ok_or_else(|| Error::guard("invalid bridge leaf journal generation"))
    }

    fn journal(&self, value: &Value) -> Result<()> {
        if value["version"] != 1 || value["identity"] != identity(self.binding) {
            return Err(Error::guard("foreign or invalid bridge leaf journal"));
        }
        self.generation(&value["generation"])?;
        Ok(())
    }

    fn current_path(&self, path: &Path) -> bool {
        self.current
            .and_then(|record| record["cgroup"]["path"].as_str())
            .is_some_and(|current| Path::new(current) == path)
    }

    fn recover_allocation(&self) -> Result<()> {
        let Some(intent) = self.control.read(ALLOCATION)? else {
            return Ok(());
        };
        self.journal(&intent)?;
        let path = intent["path"]
            .as_str()
            .map(Path::new)
            .ok_or_else(|| Error::guard("missing bridge allocation path"))?;
        if path.parent() != Some(self.root)
            || !path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("bridge-"))
        {
            return Err(Error::guard(
                "bridge allocation is outside the delegated root",
            ));
        }
        if intent["cgroup"].is_null() {
            if self.current_path(path) || self.leaves.exists(path)? {
                return Err(Error::guard(
                    "bridge allocation identity is unknown; retain the leaf and block replacement",
                ));
            }
            // No current record ever published this allocation, and no launcher
            // can start before that publication. This is not a stop proof.
            let name = format!(
                "abandoned-{}.json",
                payload_hash(&intent)?.trim_start_matches("sha256:")
            );
            self.control.write(
                &name,
                &json!({"allocation":intent,"reason":"unreleased allocation absent"}),
            )?;
        } else {
            let leaf = recorded_leaf(&intent, self.root)?;
            if leaf.path != path {
                return Err(Error::guard("bridge allocation path changed"));
            }
            if self.current_path(path) {
                let current = self.current.expect("current path matched");
                if current["generation"] != intent["generation"]
                    || current["cgroup"] != intent["cgroup"]
                {
                    return Err(Error::guard(
                        "bridge allocation conflicts with the current record",
                    ));
                }
                // The durable current record now owns this exact leaf. Its
                // absence can never be excused by a retirement receipt.
            } else {
                self.retire("allocation", &intent["generation"], &intent["cgroup"])?;
            }
        }
        self.control.remove(ALLOCATION)
    }

    fn collect_history(&self) -> Result<()> {
        let mut names = fs::read_dir(&self.control.path)
            .map_err(Error::storage)?
            .map(|entry| entry.map(|entry| entry.file_name()).map_err(Error::storage))
            .collect::<Result<Vec<_>>>()?;
        names.sort();
        let mut cleaned = 0;
        for name in names {
            let Some(name) = name.to_str() else { continue };
            let Some(generation) = name
                .strip_prefix("recovered-")
                .and_then(|name| name.strip_suffix(".json"))
            else {
                continue;
            };
            let generation = generation
                .parse::<u64>()
                .map_err(|_| Error::guard("invalid bridge recovery record name"))?;
            if name != format!("recovered-{generation}.json") {
                return Err(Error::guard("invalid bridge recovery record name"));
            }
            let recovered = self
                .control
                .read(name)?
                .ok_or_else(|| Error::guard("bridge recovery record disappeared"))?;
            if recovered["generation"] != generation || recovered["proven"] != true {
                return Err(Error::guard("invalid bridge recovery evidence"));
            }
            self.generation(&recovered["generation"])?;
            let leaf = recorded_leaf(&recovered, self.root)?;
            let current = self
                .current
                .ok_or_else(|| Error::guard("recovery evidence has no current bridge record"))?;
            if self.current_path(&leaf.path) {
                if current["generation"] != generation || current["cgroup"] != recovered["cgroup"] {
                    return Err(Error::guard(
                        "recovery evidence conflicts with the current bridge leaf",
                    ));
                }
                continue;
            }
            if current["generation"]
                .as_u64()
                .is_none_or(|current| current <= generation)
            {
                return Err(Error::guard("bridge recovery evidence is not obsolete"));
            }
            if self.completed("recovered", &recovered["generation"], &recovered["cgroup"])? {
                continue;
            }
            if cleaned == CLEANUP_BATCH {
                return Err(Error::guard(
                    "bridge leaf cleanup backlog remains; reopen before allocating another leaf",
                ));
            }
            self.retire("recovered", &recovered["generation"], &recovered["cgroup"])?;
            cleaned += 1;
        }
        Ok(())
    }

    fn retirement(&self, source: &str, generation: &Value, cgroup: &Value) -> Value {
        json!({"version":1,"identity":identity(self.binding),"source":source,
            "generation":generation,"cgroup":cgroup,"authorization":null})
    }

    fn audit_name(&self, cgroup: &Value) -> Result<String> {
        Ok(format!(
            "retired-{}.json",
            payload_hash(cgroup)?.trim_start_matches("sha256:")
        ))
    }

    fn completed(&self, source: &str, generation: &Value, cgroup: &Value) -> Result<bool> {
        let Some(audit) = self.control.read(&self.audit_name(cgroup)?)? else {
            return Ok(false);
        };
        self.journal(&audit)?;
        if audit["source"] != source
            || audit["generation"] != *generation
            || audit["cgroup"] != *cgroup
            || audit["removed"] != true
            || !authorized(&audit)
        {
            return Err(Error::guard("invalid bridge leaf retirement receipt"));
        }
        let leaf = recorded_leaf(&audit, self.root)?;
        if self.current_path(&leaf.path) || self.leaves.exists(&leaf.path)? {
            return Err(Error::guard(
                "retired bridge leaf reappeared or became current",
            ));
        }
        Ok(true)
    }

    fn retire(&self, source: &str, generation: &Value, cgroup: &Value) -> Result<()> {
        if self.completed(source, generation, cgroup)? {
            return Ok(());
        }
        if self.control.exists(RETIREMENT)? {
            return Err(Error::guard("another bridge leaf retirement is pending"));
        }
        self.control
            .write(RETIREMENT, &self.retirement(source, generation, cgroup))?;
        self.resume_retirement()
    }

    fn validate_source(&self, retirement: &Value) -> Result<()> {
        let source = match retirement["source"].as_str() {
            Some("allocation") => self.control.read(ALLOCATION)?,
            Some("recovered") => self.control.read(&format!(
                "recovered-{}.json",
                self.generation(&retirement["generation"])?
            ))?,
            _ => return Err(Error::guard("unknown bridge leaf retirement source")),
        }
        .ok_or_else(|| Error::guard("bridge leaf retirement source is missing"))?;
        if source["generation"] != retirement["generation"]
            || source["cgroup"] != retirement["cgroup"]
        {
            return Err(Error::guard("bridge leaf retirement source changed"));
        }
        if retirement["source"] == "allocation" {
            self.journal(&source)?;
            if source["path"] != retirement["cgroup"]["path"] {
                return Err(Error::guard("bridge allocation path changed"));
            }
        } else if source["proven"] != true
            || self
                .current
                .and_then(|current| current["generation"].as_u64())
                .is_none_or(|current| {
                    current <= retirement["generation"].as_u64().unwrap_or(u64::MAX)
                })
        {
            return Err(Error::guard("bridge recovery evidence is not obsolete"));
        }
        Ok(())
    }

    fn resume_retirement(&self) -> Result<()> {
        let Some(mut retirement) = self.control.read(RETIREMENT)? else {
            return Ok(());
        };
        self.journal(&retirement)?;
        self.validate_source(&retirement)?;
        let leaf = recorded_leaf(&retirement, self.root)?;
        if self.current_path(&leaf.path) {
            return Err(Error::guard("cannot retire the current bridge leaf"));
        }
        let source = retirement["source"].as_str().expect("validated source");
        if self.completed(source, &retirement["generation"], &retirement["cgroup"])? {
            return self.control.remove(RETIREMENT);
        }
        if !retirement["authorization"].is_null() && !authorized(&retirement) {
            return Err(Error::guard("invalid bridge leaf deletion authorization"));
        }
        let observed = self.leaves.observe(&leaf)?;
        if observed.reason == "missing" && authorized(&retirement) {
            retirement["completion"] = json!("absent after durable deletion authorization");
        } else {
            if !observed.proven {
                return Err(Error::guard(format!(
                    "obsolete bridge leaf is not safely removable: {}",
                    observed.reason
                )));
            }
            if retirement["authorization"].is_null() {
                retirement["authorization"] =
                    json!({"proven":true,"cgroup":retirement["cgroup"],"events":observed.events});
                self.control.write(RETIREMENT, &retirement)?;
            }
            // Re-check identity and fresh emptiness at the removal boundary.
            // Never kill an obsolete/replaced leaf as a cleanup shortcut.
            let removed = self.leaves.remove_empty(&leaf)?;
            retirement["completion"] = json!({"events":removed.events,"reason":removed.reason});
        }
        retirement["removed"] = json!(true);
        self.control
            .write(&self.audit_name(&retirement["cgroup"])?, &retirement)?;
        self.control.remove(RETIREMENT)
    }
}

fn authorized(retirement: &Value) -> bool {
    retirement["authorization"]["proven"] == true
        && retirement["authorization"]["cgroup"] == retirement["cgroup"]
        && retirement["authorization"]["events"]
            .as_str()
            .is_some_and(|events| events.lines().any(|line| line == "populated 0"))
}

#[cfg(test)]
mod tests;

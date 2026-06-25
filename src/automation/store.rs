use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, Result};

use crate::automation::model::Automation;
use crate::session::atomic_write;

const LOCK_FILENAME: &str = ".automations.lock";

fn save_lock_for(path: &Path) -> Arc<Mutex<()>> {
    static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();
    let reg = REGISTRY.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = reg.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    map.entry(path.to_path_buf())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

pub struct AutomationStore {
    path: PathBuf,
    save_lock: Arc<Mutex<()>>,
}

impl AutomationStore {
    pub fn new(profile: &str) -> Result<Self> {
        let dir = crate::session::get_profile_dir(profile)?;
        Ok(Self::with_path(dir.join("automations.json")))
    }

    pub fn with_path(path: PathBuf) -> Self {
        let save_lock = save_lock_for(&path);
        AutomationStore { path, save_lock }
    }

    pub fn load(&self) -> Result<Vec<Automation>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let bytes = std::fs::read(&self.path)
            .with_context(|| format!("reading {}", self.path.display()))?;
        if bytes.is_empty() {
            return Ok(Vec::new());
        }
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub fn update<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&mut Vec<Automation>) -> Result<R>,
    {
        let _guard = self
            .save_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = self
            .path
            .parent()
            .context("automations path has no parent")?;
        std::fs::create_dir_all(dir)?;
        let _flock = acquire_flock(dir)?;

        let mut list = self.load()?;
        let result = f(&mut list)?;
        let json = serde_json::to_vec_pretty(&list)?;
        atomic_write(&self.path, &json)?;
        Ok(result)
    }
}

/// Cross-process exclusive lock on a sidecar file, mirroring
/// `acquire_storage_flock` in `src/session/storage.rs`.
fn acquire_flock(dir: &Path) -> Result<std::fs::File> {
    use fs2::FileExt;

    let lock_path = dir.join(LOCK_FILENAME);
    #[cfg(unix)]
    let file = {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&lock_path)?
    };
    #[cfg(not(unix))]
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)?;

    file.lock_exclusive()?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::model::{LaunchSpec, Trigger};

    fn spec() -> LaunchSpec {
        LaunchSpec {
            project_path: "/tmp/p".into(),
            group_path: String::new(),
            tool: Some("claude".into()),
            command: None,
            extra_args: String::new(),
            view: crate::session::View::Terminal,
            worktree_branch: None,
            sandbox: false,
            auto_approve: true,
            max_runtime_secs: 1800,
            initial_prompt: "hi".into(),
            agent_name: None,
            agent_model: None,
        }
    }

    #[test]
    fn load_missing_file_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AutomationStore::with_path(tmp.path().join("automations.json"));
        assert!(store.load().unwrap().is_empty());
    }

    #[test]
    fn update_then_load_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AutomationStore::with_path(tmp.path().join("automations.json"));
        store
            .update(|list| {
                list.push(crate::automation::model::Automation::new(
                    "x",
                    spec(),
                    Trigger::Cron {
                        expr: "* * * * *".into(),
                    },
                ));
                Ok(())
            })
            .unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "x");
    }
}

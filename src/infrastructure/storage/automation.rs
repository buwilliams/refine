//! Atomic Events/Skills documents. Collection revisions fence concurrent editors.
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use crate::error::{RefineError, RefineResult};
use crate::infrastructure::process::supervisor::coordination::{
    replace_file_durably, with_record_lock,
};
use crate::model::automation::AutomationConfig;

#[derive(Clone)]
pub struct AutomationStore {
    pub root: PathBuf,
}

#[derive(PartialEq)]
struct Fingerprint {
    modified: std::time::SystemTime,
    len: u64,
    identity: (u64, i64, i64),
}
fn fingerprint(metadata: &fs::Metadata) -> RefineResult<Fingerprint> {
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        (metadata.ino(), metadata.ctime(), metadata.ctime_nsec())
    };
    #[cfg(not(unix))]
    let identity = (0, 0, 0);
    Ok(Fingerprint {
        modified: metadata
            .modified()
            .map_err(|e| RefineError::Io(e.to_string()))?,
        len: metadata.len(),
        identity,
    })
}
type Cache = BTreeMap<PathBuf, (Fingerprint, Arc<AutomationConfig>)>;
static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();

impl AutomationStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
    pub fn path(&self) -> PathBuf {
        self.root.join("automation/config.json")
    }

    pub fn load(&self) -> RefineResult<Arc<AutomationConfig>> {
        let path = self.path();
        let metadata = fs::metadata(&path)
            .map_err(|e| RefineError::Io(format!("read {}: {e}", path.display())))?;
        if metadata.len() > crate::model::automation::MAX_CONFIG_BYTES as u64 {
            return Err(RefineError::InvalidInput(
                "Events/Skills configuration exceeds 16 MiB".into(),
            ));
        }
        let fingerprint = fingerprint(&metadata)?;
        if let Some((observed, config)) = CACHE
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .get(&path)
            && *observed == fingerprint
        {
            return Ok(config.clone());
        }
        let config: AutomationConfig = read_json(&path)?;
        config.validate().map_err(RefineError::InvalidInput)?;
        let config = Arc::new(config);
        let mut cache = CACHE.get_or_init(Default::default).lock().unwrap();
        if cache.len() >= 64 {
            cache.clear();
        }
        cache.insert(path, (fingerprint, config.clone()));
        Ok(config)
    }

    pub fn initialize(
        &self,
        create: impl FnOnce() -> RefineResult<AutomationConfig>,
    ) -> RefineResult<Arc<AutomationConfig>> {
        if self.path().exists() {
            return self.load();
        }
        with_record_lock(&self.root, "automation-config", || {
            if !self.path().exists() {
                self.write(&create()?)?;
            }
            self.load()
        })
    }

    pub fn update(
        &self,
        revision: u64,
        change: impl FnOnce(&mut AutomationConfig) -> RefineResult<()>,
    ) -> RefineResult<Arc<AutomationConfig>> {
        with_record_lock(&self.root, "automation-config", || {
            let mut config = (*self.load()?).clone();
            if config.revision != revision {
                return Err(RefineError::Conflict(format!(
                    "Events/Skills changed: expected revision {revision}, current revision {}. Refresh before saving.",
                    config.revision
                )));
            }
            change(&mut config)?;
            config.revision = config
                .revision
                .checked_add(1)
                .ok_or_else(|| RefineError::Conflict("configuration revision exhausted".into()))?;
            self.write(&config)?;
            self.load()
        })
    }

    fn write(&self, config: &AutomationConfig) -> RefineResult<()> {
        config.validate().map_err(RefineError::InvalidInput)?;
        write_json(&self.path(), config)?;
        CACHE
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .remove(&self.path());
        Ok(())
    }
}

pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> RefineResult<T> {
    let bytes =
        fs::read(path).map_err(|e| RefineError::Io(format!("read {}: {e}", path.display())))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| RefineError::Serialization(format!("decode {}: {e}", path.display())))
}

pub fn write_json(path: &Path, value: &impl serde::Serialize) -> RefineResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| RefineError::Io(e.to_string()))?;
    }
    let bytes =
        serde_json::to_vec_pretty(value).map_err(|e| RefineError::Serialization(e.to_string()))?;
    replace_file_durably(path, &bytes)
}

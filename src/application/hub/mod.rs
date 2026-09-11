//! Knowledge Hub capability shared by all surfaces. Durable JSON is authoritative.
use crate::error::{RefineError, RefineResult};
fn reject_symlinks(path: &Path) -> RefineResult<()> {
    for component in path.ancestors() {
        match fs::symlink_metadata(component) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(invalid("Hub paths cannot contain symlinks"));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io(error)),
        }
    }
    Ok(())
}
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> RefineResult<T> {
    reject_symlinks(path)?;
    use std::io::Read;
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(RefineError::NotFound(format!(
                "Hub item not found: {}",
                path.display()
            )));
        }
        Err(error) => return Err(io(error)),
    };
    let mut bytes = Vec::new();
    file.take(16 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(io)?;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err(invalid("Hub JSON file exceeds 16 MiB"));
    }
    serde_json::from_slice(&bytes).map_err(|error| RefineError::Serialization(error.to_string()))
}
fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> RefineResult<()> {
    reject_symlinks(path)?;
    if serde_json::to_vec_pretty(value)
        .map_err(|error| RefineError::Serialization(error.to_string()))?
        .len()
        > 16 * 1024 * 1024
    {
        return Err(invalid("Hub JSON file exceeds 16 MiB"));
    }
    crate::infrastructure::storage::automation::write_json(path, value)
}
use crate::infrastructure::process::supervisor::coordination::with_record_lock;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
mod assets;
pub mod query;
#[cfg(test)]
mod tests;

#[derive(Clone)]
pub struct Hub {
    pub root: PathBuf,
    pub runtime: PathBuf,
}
fn invalid(message: impl Into<String>) -> RefineError {
    RefineError::InvalidInput(message.into())
}
fn io(e: std::io::Error) -> RefineError {
    RefineError::Io(e.to_string())
}
fn id(value: &str) -> RefineResult<&str> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(invalid(
            "IDs must contain 1–128 letters, digits, hyphens or underscores",
        ));
    }
    Ok(value)
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn revision(value: &Value) -> String {
    digest(&serde_json::to_vec(value).unwrap())
}
fn checked(current: &Value, expected: Option<&str>) -> RefineResult<()> {
    if expected != Some(revision(current).as_str()) {
        return Err(RefineError::Conflict(
            "Hub item changed; refresh its revision".into(),
        ));
    }
    Ok(())
}
fn envelope(value: Value) -> Value {
    json!({"revision":revision(&value),"item":value})
}
impl Hub {
    pub fn new(root: impl Into<PathBuf>, runtime: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            runtime: runtime.into(),
        }
    }
    fn directory(&self) -> PathBuf {
        self.root.join("hub")
    }
    fn site(&self, site: &str) -> RefineResult<PathBuf> {
        Ok(self.directory().join("sites").join(id(site)?))
    }
    fn collection(&self, site: &str, collection: &str) -> RefineResult<PathBuf> {
        self.load_site(site)?;
        Ok(self.site(site)?.join("collections").join(id(collection)?))
    }
    fn record(&self, site: &str, collection: &str, record: &str) -> RefineResult<PathBuf> {
        let key = id(record)?;
        Ok(self
            .collection(site, collection)?
            .join("records")
            .join(&digest(key.as_bytes())[..2])
            .join(format!("{key}.json")))
    }
    fn load_site(&self, site: &str) -> RefineResult<Value> {
        let v: Value = read_json(&self.site(site)?.join("site.json"))?;
        if v["deleted"] == true {
            return Err(RefineError::NotFound("Site was deleted".into()));
        }
        Ok(v)
    }
    fn load_collection(&self, site: &str, collection: &str) -> RefineResult<Value> {
        let v: Value = read_json(&self.collection(site, collection)?.join("collection.json"))?;
        if v["deleted"] == true {
            return Err(RefineError::NotFound("Collection was deleted".into()));
        }
        Ok(v)
    }
    pub fn sites(&self) -> RefineResult<Value> {
        let mut rows = Vec::new();
        let dir = self.directory().join("sites");
        if dir.exists() {
            for entry in fs::read_dir(dir).map_err(io)? {
                let entry = entry.map_err(io)?;
                if entry.file_type().map_err(io)?.is_dir() {
                    let site = entry.file_name().to_string_lossy().into_owned();
                    if let Ok(v) = self.load_site(&site) {
                        rows.push(envelope(v));
                    }
                }
            }
        }
        rows.sort_by_key(|v| v["item"]["name"].as_str().unwrap_or("").to_lowercase());
        Ok(json!({"sites":rows}))
    }
    pub fn show(&self, site: &str) -> RefineResult<Value> {
        Ok(envelope(self.load_site(site)?))
    }
    pub fn save_site(&self, site: &str, body: &Value) -> RefineResult<Value> {
        let path = self.site(site)?.join("site.json");
        with_record_lock(&self.root, &format!("hub-site-{site}"), || {
            let name = body["name"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| invalid("Site name is required"))?;
            let mut v = if path.exists() {
                let v: Value = read_json(&path)?;
                checked(&v, body["revision"].as_str())?;
                v
            } else {
                json!({"id":site,"created":chrono::Utc::now().to_rfc3339(),"publication":null})
            };
            v["name"] = json!(name);
            v["description"] = body.get("description").cloned().unwrap_or(json!(""));
            v["deleted"] = json!(false);
            write_json(&path, &v)?;
            Ok(envelope(v))
        })
    }
    pub fn delete_site(&self, site: &str, expected: &str) -> RefineResult<Value> {
        with_record_lock(&self.root, &format!("hub-site-{site}"), || {
            let mut v = self.load_site(site)?;
            checked(&v, Some(expected))?;
            v["deleted"] = json!(true);
            v["publication"] = Value::Null;
            write_json(&self.site(site)?.join("site.json"), &v)?;
            query::invalidate(&self.root);
            Ok(envelope(v))
        })
    }
    pub fn collections(&self, site: &str) -> RefineResult<Value> {
        self.load_site(site)?;
        let mut rows = vec![];
        let dir = self.site(site)?.join("collections");
        if dir.exists() {
            for e in fs::read_dir(dir).map_err(io)? {
                let e = e.map_err(io)?;
                if e.file_type().map_err(io)?.is_dir() {
                    let c = e.file_name().to_string_lossy().into_owned();
                    if let Ok(v) = self.load_collection(site, &c) {
                        rows.push(envelope(v));
                    }
                }
            }
        }
        Ok(json!({"collections":rows}))
    }
    pub fn save_collection(
        &self,
        site: &str,
        collection: &str,
        body: &Value,
    ) -> RefineResult<Value> {
        let path = self.collection(site, collection)?.join("collection.json");
        with_record_lock(
            &self.root,
            &format!("hub-collection-{site}-{collection}"),
            || {
                let indexes: crate::model::hub::IndexDefinition =
                    serde_json::from_value(body.get("indexes").cloned().unwrap_or(json!({})))
                        .map_err(|e| invalid(e.to_string()))?;
                if indexes
                    .fields
                    .keys()
                    .any(|name| ["id", "updated", "__tokens"].contains(&name.as_str()))
                {
                    return Err(invalid(
                        "Index field names id, updated and __tokens are reserved",
                    ));
                }
                for kind in indexes.fields.values() {
                    if !["string", "number", "boolean", "timestamp"].contains(&kind.as_str()) {
                        return Err(invalid("Index types: string, number, boolean, timestamp"));
                    }
                }
                if indexes.fields.len() > 32 || indexes.search.len() > 16 {
                    return Err(invalid("Too many indexed fields"));
                }
                if path.exists() {
                    checked(&read_json(&path)?, body["revision"].as_str())?;
                }
                let v = json!({"id":collection,"indexes":indexes});
                write_json(&path, &v)?;
                query::invalidate(&self.root);
                Ok(envelope(v))
            },
        )
    }
    pub fn delete_collection(
        &self,
        site: &str,
        collection: &str,
        expected: &str,
    ) -> RefineResult<Value> {
        with_record_lock(
            &self.root,
            &format!("hub-collection-{site}-{collection}"),
            || {
                let mut v = self.load_collection(site, collection)?;
                checked(&v, Some(expected))?;
                v["deleted"] = json!(true);
                write_json(
                    &self.collection(site, collection)?.join("collection.json"),
                    &v,
                )?;
                query::invalidate(&self.root);
                Ok(envelope(v))
            },
        )
    }
    pub fn get(&self, site: &str, collection: &str, record: &str) -> RefineResult<Value> {
        self.load_collection(site, collection)?;
        let v: Value = read_json(&self.record(site, collection, record)?)?;
        if v["deleted"] == true {
            return Err(RefineError::NotFound("Record was deleted".into()));
        }
        Ok(envelope(v))
    }
    pub fn put(
        &self,
        site: &str,
        collection: &str,
        record: &str,
        body: &Value,
    ) -> RefineResult<Value> {
        self.load_collection(site, collection)?;
        let data = body
            .get("data")
            .filter(|v| v.is_object())
            .ok_or_else(|| invalid("Record data must be a JSON object"))?;
        if serde_json::to_vec(data).unwrap().len() > 1024 * 1024 {
            return Err(invalid("Record exceeds 1 MiB"));
        }
        let path = self.record(site, collection, record)?;
        with_record_lock(
            &self.root,
            &format!("hub-collection-{site}-{collection}"),
            || {
                let definition = serde_json::from_value(
                    self.load_collection(site, collection)?["indexes"].clone(),
                )
                .map_err(|e| invalid(format!("Invalid collection indexes: {e}")))?;
                query::validate_data(&definition, data)?;
                if path.exists() {
                    let old: Value = read_json(&path)?;
                    if old["request_id"] == body["request_id"] && body["request_id"].is_string() {
                        if old["data"] == *data {
                            return Ok(envelope(old));
                        }
                        return Err(RefineError::Conflict(
                            "request_id reused with different data".into(),
                        ));
                    }
                    checked(&old, body["revision"].as_str())?;
                }
                let v = json!({"id":record,"data":data,"updated":chrono::Utc::now().to_rfc3339(),"request_id":body.get("request_id").cloned().unwrap_or(Value::Null)});
                write_json(&path, &v)?;
                query::record_changed(self, site, collection, &v);
                Ok(envelope(v))
            },
        )
    }
    pub fn delete(
        &self,
        site: &str,
        collection: &str,
        record: &str,
        expected: &str,
    ) -> RefineResult<Value> {
        let path = self.record(site, collection, record)?;
        with_record_lock(
            &self.root,
            &format!("hub-collection-{site}-{collection}"),
            || {
                self.load_collection(site, collection)?;
                let mut v: Value = read_json(&path)?;
                checked(&v, Some(expected))?;
                v["deleted"] = json!(true);
                write_json(&path, &v)?;
                query::record_changed(self, site, collection, &v);
                Ok(envelope(v))
            },
        )
    }
    pub fn import(&self, site: &str, collection: &str, rows: &[Value]) -> RefineResult<Value> {
        if rows.len() > 1000 {
            return Err(invalid("Import batches are limited to 1000 records"));
        }
        let mut bytes = 0usize;
        for row in rows {
            bytes += row.to_string().len();
            if bytes > 8 * 1024 * 1024 {
                return Err(invalid("Import batches must fit 8 MiB"));
            }
        }
        let results = rows
            .iter()
            .map(|row| {
                let result = row["id"]
                    .as_str()
                    .ok_or_else(|| invalid("Record id required"))
                    .and_then(|id| self.put(site, collection, id, row));
                match result {
                    Ok(v) => json!({"id":row["id"],"ok":true,"result":{"revision":v["revision"]}}),
                    Err(e) => json!({"id":row["id"],"ok":false,"error":e.to_string()}),
                }
            })
            .collect::<Vec<_>>();
        Ok(json!({"results":results}))
    }
    pub fn status(&self, site: &str) -> RefineResult<Value> {
        let value = self.load_site(site)?;
        let target =
            crate::infrastructure::storage::project_layout::target_root_for_refine_dir(&self.root)?;
        let node = crate::application::fleet::nodes::FileNodeRegistryService::with_active_root(
            &self.root,
            &self.runtime,
        )
        .active_node_id()?;
        let threshold =
            crate::application::workers::state_sync_stale_threshold(&self.runtime, &target)?;
        let health = crate::application::persistence_sync::health::FileStateSyncHealthService::new(
            &self.runtime,
        )
        .inspect(&target, &node, threshold)?;
        Ok(
            json!({"site":envelope(value),"local_available":true,"sync":health,
            "sync_note":"Local availability does not guarantee that this latest edit has reached the state remote"}),
        )
    }
}

/// Sync and authored mutations coordinate on the same semantic collection or site.
pub(crate) fn synchronization_lock_key(relative: &Path) -> Option<String> {
    let parts = relative
        .iter()
        .map(|part| part.to_str())
        .collect::<Option<Vec<_>>>()?;
    match parts.as_slice() {
        ["hub", "sites", site, "collections", collection, ..] => {
            Some(format!("hub-collection-{site}-{collection}"))
        }
        ["hub", "sites", site, ..] => Some(format!("hub-site-{site}")),
        _ => None,
    }
}

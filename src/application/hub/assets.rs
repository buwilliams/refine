//! Atomic asset manifests: uploads are immutable blobs, publication is one JSON replacement.
use super::*;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};
const ASSET_CACHE_BYTES: usize = 64 * 1024 * 1024;
type AssetCache = BTreeMap<String, Arc<Vec<u8>>>;
static ASSETS: OnceLock<Mutex<AssetCache>> = OnceLock::new();

fn asset_name(name: &str) -> RefineResult<()> {
    let path = Path::new(name);
    if name.is_empty()
        || name.len() > 512
        || name.contains('\\')
        || name.chars().any(char::is_control)
        || name.split('/').any(|part| matches!(part, "" | "." | ".."))
        || path
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err(invalid(
            "Asset path must be relative without dot components",
        ));
    }
    Ok(())
}
impl Hub {
    pub fn assets(&self, site: &str) -> RefineResult<Value> {
        self.load_site(site)?;
        let path = self.site(site)?.join("assets.json");
        Ok(envelope(if path.exists() {
            read_json(&path)?
        } else {
            json!({})
        }))
    }
    pub fn save_asset(
        &self,
        site: &str,
        name: &str,
        bytes: &[u8],
        expected: Option<&str>,
        delete: bool,
    ) -> RefineResult<Value> {
        asset_name(name)?;
        self.load_site(site)?;
        if bytes.len() > 16 * 1024 * 1024 {
            return Err(invalid("Asset exceeds 16 MiB"));
        }
        with_record_lock(&self.root, &format!("hub-site-{site}"), || {
            let mut manifest = self.assets(site)?["item"].clone();
            if expected.is_some() || !manifest.as_object().unwrap().is_empty() {
                checked(&manifest, expected)?;
            }
            if delete {
                manifest.as_object_mut().unwrap().remove(name);
            } else {
                if manifest.get(name).is_none() && manifest.as_object().unwrap().len() >= 10_000 {
                    return Err(invalid("Site asset manifest is limited to 10000 files"));
                }
                let hash = digest(bytes);
                let path = self.site(site)?.join("blobs").join(&hash);
                reject_symlinks(&path)?;
                if !path.exists() {
                    crate::infrastructure::process::supervisor::coordination::replace_file_durably(
                        &path, bytes,
                    )?;
                }
                manifest[name] = json!({"hash":hash,"bytes":bytes.len()});
            }
            write_json(&self.site(site)?.join("assets.json"), &manifest)?;
            Ok(envelope(manifest))
        })
    }
    pub fn publish(
        &self,
        site: &str,
        expected: &str,
        collections: &[String],
        enabled: bool,
    ) -> RefineResult<Value> {
        with_record_lock(&self.root, &format!("hub-site-{site}"), || {
            let mut v = self.load_site(site)?;
            checked(&v, Some(expected))?;
            if enabled {
                let assets = self.assets(site)?["item"].clone();
                if assets["index.html"].is_null() {
                    return Err(invalid("Publish requires index.html"));
                }
                for c in collections {
                    self.load_collection(site, c)?;
                }
                v["publication"] = json!({"version":revision(&assets),"assets":assets,"collections":collections,"at":chrono::Utc::now().to_rfc3339()});
            } else {
                v["publication"] = Value::Null;
            }
            write_json(&self.site(site)?.join("site.json"), &v)?;
            Ok(envelope(v))
        })
    }
    pub fn asset(&self, site: &str, name: &str, public: bool) -> RefineResult<(Vec<u8>, String)> {
        asset_name(name)?;
        let v = self.load_site(site)?;
        let manifest = if public {
            v["publication"]["assets"].clone()
        } else {
            self.assets(site)?["item"].clone()
        };
        let hash = manifest[name]["hash"]
            .as_str()
            .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| RefineError::NotFound("Published asset not found".into()))?;
        let path = self.site(site)?.join("blobs").join(hash);
        reject_symlinks(&path)?;
        if fs::symlink_metadata(&path)
            .map_err(io)?
            .file_type()
            .is_symlink()
        {
            return Err(invalid("Asset blob cannot be a symlink"));
        }
        let cached = ASSETS
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(hash)
            .cloned();
        if let Some(bytes) = cached {
            return Ok(((*bytes).clone(), hash.into()));
        }
        use std::io::Read;
        let file = fs::File::open(path).map_err(io)?;
        let mut bytes = Vec::new();
        file.take(16 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(io)?;
        if bytes.len() > 16 * 1024 * 1024 {
            return Err(invalid("Asset exceeds 16 MiB"));
        }
        if digest(&bytes) != hash {
            return Err(RefineError::Conflict(
                "Asset blob does not match its publication".into(),
            ));
        }
        let mut cache = ASSETS
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if cache.len() >= 256
            || cache.values().map(|value| value.len()).sum::<usize>() + bytes.len()
                > ASSET_CACHE_BYTES
        {
            cache.clear();
        }
        cache.insert(hash.into(), Arc::new(bytes.clone()));
        Ok((bytes, hash.into()))
    }
    pub fn public_query(
        &self,
        site: &str,
        collection: &str,
        q: &crate::model::hub::Query,
    ) -> RefineResult<Value> {
        let v = self.load_site(site)?;
        if !v["publication"]["collections"]
            .as_array()
            .is_some_and(|a| a.iter().any(|v| v == collection))
        {
            return Err(RefineError::NotFound("Collection is not published".into()));
        }
        self.query(site, collection, q)
    }
}

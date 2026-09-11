use super::*;
use base64::Engine as _;
fn segment(s: &str) -> RefineResult<&str> {
    if s.is_empty()
        || !s
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(RefineError::InvalidInput("Invalid Hub identifier".into()));
    }
    Ok(s)
}
fn site(s: &str) -> RefineResult<String> {
    Ok(format!("/api/hub/sites/{}", segment(s)?))
}
fn collection(s: &str, c: &str) -> RefineResult<String> {
    Ok(format!("{}/collections/{}", site(s)?, segment(c)?))
}
fn bounded_file(p: &Path) -> RefineResult<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    fs::File::open(p)
        .map_err(|error| RefineError::Io(error.to_string()))?
        .take(16 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| RefineError::Io(error.to_string()))?;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err(RefineError::InvalidInput("Hub input exceeds 16 MiB".into()));
    }
    Ok(bytes)
}
fn payload(p: &Path) -> RefineResult<Value> {
    serde_json::from_slice(&bounded_file(p)?).map_err(|e| RefineError::InvalidInput(e.to_string()))
}
pub(super) fn dispatch(action: HubAction) -> RefineResult<()> {
    let result = match action {
        HubAction::List => daemon_json("GET", "/api/hub/sites", None)?,
        HubAction::Show { site: s } => daemon_json("GET", &site(&s)?, None)?,
        HubAction::Save { site: s, file } => daemon_json("PUT", &site(&s)?, Some(payload(&file)?))?,
        HubAction::Delete { site: s, revision } => {
            daemon_json("DELETE", &site(&s)?, Some(json!({"revision":revision})))?
        }
        HubAction::Collections { site: s } => {
            daemon_json("GET", &format!("{}/collections", site(&s)?), None)?
        }
        HubAction::Collection {
            site: s,
            collection: c,
            file,
        } => daemon_json("PUT", &collection(&s, &c)?, Some(payload(&file)?))?,
        HubAction::DeleteCollection {
            site: s,
            collection: c,
            revision,
        } => daemon_json(
            "DELETE",
            &collection(&s, &c)?,
            Some(json!({"revision":revision})),
        )?,
        HubAction::Get {
            site: s,
            collection: c,
            id,
        } => daemon_json(
            "GET",
            &format!("{}/records/{}", collection(&s, &c)?, segment(&id)?),
            None,
        )?,
        HubAction::Put {
            site: s,
            collection: c,
            id,
            file,
        } => daemon_json(
            "PUT",
            &format!("{}/records/{}", collection(&s, &c)?, segment(&id)?),
            Some(payload(&file)?),
        )?,
        HubAction::Remove {
            site: s,
            collection: c,
            id,
            revision,
        } => daemon_json(
            "DELETE",
            &format!("{}/records/{}", collection(&s, &c)?, segment(&id)?),
            Some(json!({"revision":revision})),
        )?,
        HubAction::Query {
            site: s,
            collection: c,
            file,
        } => daemon_json(
            "POST",
            &format!("{}/query", collection(&s, &c)?),
            Some(payload(&file)?),
        )?,
        HubAction::Index {
            site: s,
            collection: c,
        } => daemon_json(
            "POST",
            &format!("{}/index", collection(&s, &c)?),
            Some(json!({})),
        )?,
        HubAction::Assets { site: s } => {
            daemon_json("GET", &format!("{}/assets", site(&s)?), None)?
        }
        HubAction::RemoveAsset {
            site: s,
            path,
            revision,
        } => daemon_json(
            "DELETE",
            &format!("{}/assets", site(&s)?),
            Some(json!({"path":path,"revision":revision})),
        )?,
        HubAction::Publish {
            site: s,
            revision,
            collection,
        } => daemon_json(
            "POST",
            &format!("{}/publish", site(&s)?),
            Some(json!({"revision":revision,"collections":collection})),
        )?,
        HubAction::Unpublish { site: s, revision } => daemon_json(
            "POST",
            &format!("{}/unpublish", site(&s)?),
            Some(json!({"revision":revision})),
        )?,
        HubAction::Status { site: s } => {
            daemon_json("GET", &format!("{}/status", site(&s)?), None)?
        }
        HubAction::Import {
            site: s,
            collection: c,
            file,
        } => {
            use std::io::{BufRead, Read};
            let path = format!("{}/import", collection(&s, &c)?);
            let input =
                fs::File::open(&file).map_err(|error| RefineError::Io(error.to_string()))?;
            let mut reader = std::io::BufReader::new(input);
            let array = loop {
                let bytes = reader
                    .fill_buf()
                    .map_err(|error| RefineError::Io(error.to_string()))?;
                if let Some(first) = bytes.iter().find(|byte| !byte.is_ascii_whitespace()) {
                    break *first == b'[';
                }
                let length = bytes.len();
                if length == 0 {
                    break false;
                }
                reader.consume(length);
            };
            if array {
                let rows: Vec<Value> = serde_json::from_reader(reader.take(8 * 1024 * 1024 + 1))
                    .map_err(|error|RefineError::InvalidInput(format!("JSON array imports must fit 8 MiB; use JSONL for larger imports: {error}")))?;
                import_rows(&path, rows.into_iter().map(Ok))?
            } else {
                let rows = std::iter::from_fn(move || {
                    let mut bytes = Vec::new();
                    loop {
                        bytes.clear();
                        let result = (&mut reader)
                            .take(2 * 1024 * 1024 + 1)
                            .read_until(b'\n', &mut bytes);
                        match result {
                            Ok(0) => return None,
                            Ok(size) if size > 2 * 1024 * 1024 => {
                                return Some(Err(RefineError::InvalidInput(
                                    "JSONL record exceeds 2 MiB".into(),
                                )));
                            }
                            Ok(_) if bytes.iter().all(u8::is_ascii_whitespace) => continue,
                            Ok(_) => {
                                return Some(serde_json::from_slice(&bytes).map_err(|error| {
                                    RefineError::InvalidInput(error.to_string())
                                }));
                            }
                            Err(error) => return Some(Err(RefineError::Io(error.to_string()))),
                        }
                    }
                });
                import_rows(&path, rows)?
            }
        }

        HubAction::Export {
            site: s,
            collection: c,
            file,
        } => {
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(file)
                .map_err(|e| RefineError::Io(e.to_string()))?;
            let mut cursor = Value::Null;
            let mut count = 0;
            loop {
                let r = daemon_json(
                    "POST",
                    &format!("{}/query", collection(&s, &c)?),
                    Some(json!({"version":1,"limit":100,"cursor":cursor})),
                )?;
                for row in r["rows"]
                    .as_array()
                    .ok_or_else(|| RefineError::Serialization("Missing query rows".into()))?
                {
                    writeln!(output, "{}", json!({"id":row["item"]["id"],"data":row["item"]["data"],"revision":row["revision"]}))
                        .map_err(|e| RefineError::Io(e.to_string()))?;
                    count += 1;
                }
                cursor = r["next_cursor"].clone();
                if cursor.is_null() {
                    break;
                }
            }
            json!({"exported":count})
        }
        HubAction::Upload { site: s, directory } => {
            let mut paths = vec![directory.clone()];
            let mut count = 0;
            let api = format!("{}/assets", site(&s)?);
            let mut manifest = daemon_json("GET", &api, None)?;
            while let Some(path) = paths.pop() {
                let meta =
                    fs::symlink_metadata(&path).map_err(|e| RefineError::Io(e.to_string()))?;
                if meta.is_symlink() {
                    return Err(RefineError::InvalidInput(
                        "Site uploads cannot include symlinks".into(),
                    ));
                }
                if meta.is_dir() {
                    for e in fs::read_dir(path).map_err(|e| RefineError::Io(e.to_string()))? {
                        paths.push(e.map_err(|e| RefineError::Io(e.to_string()))?.path());
                    }
                } else {
                    if !meta.is_file() || meta.len() > 16 * 1024 * 1024 {
                        return Err(RefineError::InvalidInput(
                            "Assets must be regular files of at most 16 MiB".into(),
                        ));
                    }
                    let name = path
                        .strip_prefix(&directory)
                        .map_err(|e| RefineError::InvalidInput(e.to_string()))?
                        .to_string_lossy();
                    manifest = daemon_json(
                        "PUT",
                        &api,
                        Some(
                            json!({"path":name,"revision":manifest["revision"],"bytes_base64":base64::engine::general_purpose::STANDARD.encode(bounded_file(&path)?)}),
                        ),
                    )?;
                    count += 1;
                }
            }
            json!({"uploaded":count,"manifest":manifest})
        }
        HubAction::Download { site: s, directory } => {
            let api = format!("{}/assets", site(&s)?);
            let manifest = daemon_json("GET", &api, None)?;
            let assets = manifest["item"]
                .as_object()
                .ok_or_else(|| RefineError::Serialization("Invalid manifest".into()))?;
            fs::create_dir(&directory).map_err(|e| RefineError::Io(e.to_string()))?;
            for name in assets.keys() {
                if Path::new(name)
                    .components()
                    .any(|c| !matches!(c, std::path::Component::Normal(_)))
                {
                    return Err(RefineError::InvalidInput("Invalid asset path".into()));
                }
                let r = daemon_json("POST", &api, Some(json!({"path":name})))?;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(r["bytes_base64"].as_str().ok_or_else(|| {
                        RefineError::Serialization("Missing asset content".into())
                    })?)
                    .map_err(|error| RefineError::Serialization(error.to_string()))?;
                let path = directory.join(name);
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).map_err(|e| RefineError::Io(e.to_string()))?;
                }
                fs::write(path, bytes).map_err(|e| RefineError::Io(e.to_string()))?;
            }
            json!({"downloaded":assets.len()})
        }
    };
    print_json(&result);
    Ok(())
}

fn import_rows(path: &str, rows: impl Iterator<Item = RefineResult<Value>>) -> RefineResult<Value> {
    let mut batch = Vec::new();
    let mut bytes = 0;
    let mut count = 0;
    let mut failed = 0;
    let mut flush = |batch: &mut Vec<Value>| -> RefineResult<()> {
        if batch.is_empty() {
            return Ok(());
        }
        count += batch.len();
        let result = daemon_json("POST", path, Some(json!({"records":std::mem::take(batch)})))?;
        failed += result["results"]
            .as_array()
            .map(|results| results.iter().filter(|row| row["ok"] != true).count())
            .unwrap_or(0);
        print_json(&result);
        Ok(())
    };
    for row in rows {
        let row = row?;
        let size = row.to_string().len();
        if bytes + size > 4 * 1024 * 1024 || batch.len() == 100 {
            flush(&mut batch)?;
            bytes = 0;
        }
        bytes += size;
        batch.push(row);
    }
    flush(&mut batch)?;
    Ok(json!({"records":count,"failed":failed}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hub_cli_uses_management_routes_instead_of_hosted_site_urls() {
        assert_eq!(site("reports").unwrap(), "/api/hub/sites/reports");
        assert_eq!(
            collection("reports", "events").unwrap(),
            "/api/hub/sites/reports/collections/events"
        );
        assert!(site("../reports").is_err());
    }
}

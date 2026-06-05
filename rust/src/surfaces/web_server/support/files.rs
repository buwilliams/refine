use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::core::supervisor::errors::{RefineError, RefineResult};

pub(in crate::surfaces::web_server) fn resolve_project_utility_path(path: &str) -> PathBuf {
    let path = path.trim();
    if path.is_empty() {
        return std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

pub(in crate::surfaces::web_server) fn project_directories_response(
    path: &str,
    max_entries: usize,
) -> RefineResult<Value> {
    let selected_path = resolve_project_utility_path(path);
    let list_path = if selected_path.is_dir() {
        selected_path.clone()
    } else {
        selected_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| selected_path.clone())
    };
    if !list_path.exists() {
        return Err(RefineError::NotFound(format!(
            "directory {} was not found",
            list_path.display()
        )));
    }
    if !list_path.is_dir() {
        return Err(RefineError::InvalidInput(format!(
            "{} is not a directory",
            list_path.display()
        )));
    }
    let mut entries = Vec::new();
    let mut truncated = false;
    for entry in fs::read_dir(&list_path).map_err(|error| {
        RefineError::Io(format!(
            "failed to read directory {}: {error}",
            list_path.display()
        ))
    })? {
        let entry = entry.map_err(|error| {
            RefineError::Io(format!(
                "failed to read directory entry {}: {error}",
                list_path.display()
            ))
        })?;
        let metadata = entry.metadata().map_err(|error| {
            RefineError::Io(format!(
                "failed to stat directory entry {}: {error}",
                entry.path().display()
            ))
        })?;
        if !metadata.is_dir() {
            continue;
        }
        if entries.len() >= max_entries {
            truncated = true;
            break;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        entries.push(json!({
            "name": name,
            "path": entry.path().display().to_string()
        }));
    }
    entries.sort_by(|a, b| {
        a.get("name")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .cmp(b.get("name").and_then(|value| value.as_str()).unwrap_or(""))
    });
    Ok(json!({
        "path": list_path.display().to_string(),
        "selected_path": selected_path.display().to_string(),
        "parent": list_path.parent().map(|path| path.display().to_string()),
        "entries": entries,
        "truncated": truncated
    }))
}

pub(in crate::surfaces::web_server) fn files_tree_response(
    source_root: &Path,
    path: &str,
    recursive: bool,
    max_depth: usize,
    max_entries: usize,
) -> RefineResult<Value> {
    let rel_path = normalize_file_path(path)?;
    let absolute = source_root.join(&rel_path);
    if !absolute.exists() {
        return Err(RefineError::NotFound(format!(
            "source path {} was not found",
            display_rel_path(&rel_path)
        )));
    }
    if !absolute.is_dir() {
        return Err(RefineError::InvalidInput(format!(
            "source path {} is not a directory",
            display_rel_path(&rel_path)
        )));
    }
    let mut entries_by_path = serde_json::Map::new();
    let mut meta_by_path = serde_json::Map::new();
    let mut remaining = max_entries;
    collect_file_tree(
        source_root,
        &rel_path,
        recursive,
        max_depth,
        0,
        &mut remaining,
        &mut entries_by_path,
        &mut meta_by_path,
    )?;
    let path = display_rel_path(&rel_path);
    let entries = entries_by_path
        .get(&path)
        .cloned()
        .unwrap_or_else(|| json!([]));
    let truncated = meta_by_path
        .values()
        .any(|meta| meta.get("truncated").and_then(|value| value.as_bool()) == Some(true));
    Ok(json!({
        "path": path,
        "entries": entries,
        "entries_by_path": entries_by_path,
        "meta_by_path": meta_by_path,
        "truncated": truncated
    }))
}

pub(in crate::surfaces::web_server) fn collect_file_tree(
    source_root: &Path,
    rel_path: &Path,
    recursive: bool,
    max_depth: usize,
    depth: usize,
    remaining: &mut usize,
    entries_by_path: &mut serde_json::Map<String, Value>,
    meta_by_path: &mut serde_json::Map<String, Value>,
) -> RefineResult<()> {
    let absolute = source_root.join(rel_path);
    let mut entries = read_file_entries(source_root, &absolute, rel_path)?;
    let mut truncated = false;
    if entries.len() > *remaining {
        entries.truncate(*remaining);
        truncated = true;
        *remaining = 0;
    } else {
        *remaining -= entries.len();
    }
    let rel_key = display_rel_path(rel_path);
    entries_by_path.insert(rel_key.clone(), json!(entries));
    meta_by_path.insert(
        rel_key,
        json!({
            "truncated": truncated,
            "depth": depth
        }),
    );
    if recursive && depth < max_depth && *remaining > 0 {
        let child_dirs: Vec<PathBuf> = entries_by_path
            .get(&display_rel_path(rel_path))
            .and_then(|value| value.as_array())
            .into_iter()
            .flatten()
            .filter(|entry| entry.get("type").and_then(|value| value.as_str()) == Some("directory"))
            .filter_map(|entry| {
                entry
                    .get("path")
                    .and_then(|value| value.as_str())
                    .map(PathBuf::from)
            })
            .collect();
        for child in child_dirs {
            if *remaining == 0 {
                break;
            }
            collect_file_tree(
                source_root,
                &child,
                recursive,
                max_depth,
                depth + 1,
                remaining,
                entries_by_path,
                meta_by_path,
            )?;
        }
    }
    Ok(())
}

pub(in crate::surfaces::web_server) fn read_file_entries(
    source_root: &Path,
    absolute: &Path,
    rel_path: &Path,
) -> RefineResult<Vec<Value>> {
    let mut entries = Vec::new();
    let read_dir = fs::read_dir(absolute).map_err(|error| {
        RefineError::Io(format!(
            "failed to read source directory {}: {error}",
            absolute.display()
        ))
    })?;
    for entry in read_dir {
        let entry = entry.map_err(|error| {
            RefineError::Io(format!(
                "failed to read source directory entry {}: {error}",
                absolute.display()
            ))
        })?;
        let name = entry.file_name().to_string_lossy().to_string();
        if should_skip_source_entry(&name) {
            continue;
        }
        let path = entry.path();
        let metadata = entry.metadata().map_err(|error| {
            RefineError::Io(format!(
                "failed to stat source path {}: {error}",
                path.display()
            ))
        })?;
        let child_rel = rel_path.join(&name);
        let kind = if metadata.is_dir() {
            "directory"
        } else {
            "file"
        };
        entries.push(json!({
            "name": name,
            "path": display_rel_path(&child_rel),
            "type": kind,
            "size": metadata.len()
        }));
    }
    entries.sort_by(|a, b| {
        let a_type = a.get("type").and_then(|value| value.as_str()).unwrap_or("");
        let b_type = b.get("type").and_then(|value| value.as_str()).unwrap_or("");
        let a_name = a.get("name").and_then(|value| value.as_str()).unwrap_or("");
        let b_name = b.get("name").and_then(|value| value.as_str()).unwrap_or("");
        b_type.cmp(a_type).then_with(|| a_name.cmp(b_name))
    });
    let _ = source_root;
    Ok(entries)
}

pub(in crate::surfaces::web_server) fn files_read_response(
    source_root: &Path,
    path: &str,
    offset: usize,
    limit: usize,
) -> RefineResult<Value> {
    let rel_path = normalize_file_path(path)?;
    if rel_path.as_os_str().is_empty() {
        return Err(RefineError::InvalidInput(
            "file path is required".to_string(),
        ));
    }
    let absolute = source_root.join(&rel_path);
    if !absolute.exists() {
        return Err(RefineError::NotFound(format!(
            "source file {} was not found",
            display_rel_path(&rel_path)
        )));
    }
    if !absolute.is_file() {
        return Err(RefineError::InvalidInput(format!(
            "source path {} is not a file",
            display_rel_path(&rel_path)
        )));
    }
    let bytes = fs::read(&absolute).map_err(|error| {
        RefineError::Io(format!(
            "failed to read source file {}: {error}",
            absolute.display()
        ))
    })?;
    let name = rel_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_string();
    if is_binary_bytes(&bytes) {
        return Ok(json!({
            "path": display_rel_path(&rel_path),
            "name": name,
            "kind": "binary",
            "previewable": false,
            "reason": "Binary preview is not available yet.",
            "size": bytes.len(),
            "offset": offset,
            "limit": limit,
            "has_more": false,
            "next_offset": null,
            "large": bytes.len() > limit
        }));
    }
    let offset = offset.min(bytes.len());
    let end = (offset + limit).min(bytes.len());
    let content = String::from_utf8_lossy(&bytes[offset..end]).to_string();
    let start_line = bytes[..offset]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count()
        + 1;
    Ok(json!({
        "path": display_rel_path(&rel_path),
        "name": name,
        "kind": "text",
        "previewable": true,
        "content": content,
        "size": bytes.len(),
        "offset": offset,
        "limit": limit,
        "start_line": start_line,
        "has_more": end < bytes.len(),
        "next_offset": if end < bytes.len() { json!(end) } else { Value::Null },
        "large": bytes.len() > limit
    }))
}

pub(in crate::surfaces::web_server) fn files_search_response(
    source_root: &Path,
    query: &str,
    max_entries: usize,
) -> RefineResult<Value> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Ok(json!({
            "query": "",
            "entries": [],
            "truncated": false
        }));
    }
    let mut entries = Vec::new();
    let mut truncated = false;
    search_source_paths(
        source_root,
        Path::new(""),
        &query,
        max_entries,
        &mut entries,
        &mut truncated,
    )?;
    Ok(json!({
        "query": query,
        "entries": entries,
        "truncated": truncated
    }))
}

pub(in crate::surfaces::web_server) fn search_source_paths(
    source_root: &Path,
    rel_path: &Path,
    query: &str,
    max_entries: usize,
    entries: &mut Vec<Value>,
    truncated: &mut bool,
) -> RefineResult<()> {
    if entries.len() >= max_entries {
        *truncated = true;
        return Ok(());
    }
    let absolute = source_root.join(rel_path);
    for entry in fs::read_dir(&absolute).map_err(|error| {
        RefineError::Io(format!(
            "failed to search source directory {}: {error}",
            absolute.display()
        ))
    })? {
        let entry = entry.map_err(|error| {
            RefineError::Io(format!(
                "failed to read source directory entry {}: {error}",
                absolute.display()
            ))
        })?;
        let name = entry.file_name().to_string_lossy().to_string();
        if should_skip_source_entry(&name) {
            continue;
        }
        let path = entry.path();
        let metadata = entry.metadata().map_err(|error| {
            RefineError::Io(format!(
                "failed to stat source path {}: {error}",
                path.display()
            ))
        })?;
        let child_rel = rel_path.join(&name);
        let rel_display = display_rel_path(&child_rel);
        let kind = if metadata.is_dir() {
            "directory"
        } else {
            "file"
        };
        if name.to_lowercase().contains(query) || rel_display.to_lowercase().contains(query) {
            if entries.len() >= max_entries {
                *truncated = true;
                return Ok(());
            }
            entries.push(json!({
                "name": name,
                "path": rel_display,
                "type": kind,
                "size": metadata.len()
            }));
        }
        if metadata.is_dir() {
            search_source_paths(
                source_root,
                &child_rel,
                query,
                max_entries,
                entries,
                truncated,
            )?;
            if *truncated {
                return Ok(());
            }
        }
    }
    Ok(())
}

pub(in crate::surfaces::web_server) fn normalize_file_path(path: &str) -> RefineResult<PathBuf> {
    let path = path.replace('\\', "/");
    let path = path.trim().trim_start_matches('/');
    let mut normalized = PathBuf::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                return Err(RefineError::InvalidInput(
                    "source path cannot contain ..".to_string(),
                ));
            }
            value => normalized.push(value),
        }
    }
    Ok(normalized)
}

pub(in crate::surfaces::web_server) fn display_rel_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub(in crate::surfaces::web_server) fn should_skip_source_entry(name: &str) -> bool {
    matches!(name, ".git" | ".refine" | "node_modules" | "target")
}

pub(in crate::surfaces::web_server) fn is_binary_bytes(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|byte| *byte == 0)
}

pub(in crate::surfaces::web_server) fn query_param(raw_path: &str, key: &str) -> Option<String> {
    let query = raw_path.split_once('?')?.1;
    for pair in query.split('&') {
        let (raw_key, raw_value) = pair.split_once('=').unwrap_or((pair, ""));
        if percent_decode(raw_key) == key {
            return Some(percent_decode(raw_value));
        }
    }
    None
}

pub(in crate::surfaces::web_server) fn bounded_query_usize(
    raw_path: &str,
    key: &str,
    default: usize,
    max: usize,
) -> usize {
    query_param(raw_path, key)
        .and_then(|value| value.parse::<usize>().ok())
        .map(|value| value.min(max))
        .unwrap_or(default)
}

pub(in crate::surfaces::web_server) fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[index + 1..index + 3]) {
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    output.push(byte);
                    index += 3;
                    continue;
                }
            }
        }
        output.push(if bytes[index] == b'+' {
            b' '
        } else {
            bytes[index]
        });
        index += 1;
    }
    String::from_utf8_lossy(&output).to_string()
}

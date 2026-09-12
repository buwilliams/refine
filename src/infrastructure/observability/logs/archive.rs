//! One bounded page over retained application, Round, and managed-process logs.
use crate::error::{RefineError, RefineResult};
use crate::infrastructure::process::subprocess::{FileProcessSupervisor, ManagedProcess};
use crate::model::log::ActivityEntry;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

#[derive(Default)]
pub(crate) struct ArchiveQuery {
    pub goal_id: Option<String>,
    pub q: String,
    pub filters: BTreeMap<String, String>,
    pub offset: usize,
    pub limit: usize,
    pub tail: bool,
    pub cursors: BTreeMap<String, u64>,
}

pub(crate) fn query_archive(
    root: &Path,
    runtime: Option<&Path>,
    goals: &[String],
    query: ArchiveQuery,
) -> RefineResult<Value> {
    let mut files = vec![(
        "activity".to_string(),
        root.join("logs/activity.jsonl"),
        json!({"log_type":"event"}),
    )];
    for id in goals
        .iter()
        .filter(|id| query.goal_id.as_ref().is_none_or(|wanted| wanted == *id))
    {
        files.push((
            format!("round:{id}"),
            super::goal_logs_path(root, id),
            json!({"log_type":"round", "goal_id":id}),
        ));
    }
    if let Some(runtime) = runtime {
        for name in ["api-events.jsonl.1", "api-events.jsonl"] {
            files.push((
                format!("api:{name}"),
                runtime.join(name),
                json!({"log_type":"api"}),
            ));
        }
        let operations = runtime.join("operations");
        if operations.exists() {
            for file in fs::read_dir(&operations).map_err(|e| RefineError::Io(e.to_string()))? {
                let path = file.map_err(|e| RefineError::Io(e.to_string()))?.path();
                let Some(id) = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .and_then(|s| s.strip_suffix(".logs.jsonl"))
                else {
                    continue;
                };
                let record: Value = fs::read(operations.join(format!("{id}.json")))
                    .ok()
                    .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                    .unwrap_or(Value::Null);
                let goal = record["request"]["goal_id"].clone();
                if query
                    .goal_id
                    .as_ref()
                    .is_some_and(|wanted| goal.as_str().is_some_and(|id| id != wanted))
                {
                    continue;
                }
                files.push((
                    format!("operation:{id}"),
                    path.clone(),
                    json!({"log_type":"operation","goal_id":goal,"operation_id":id}),
                ));
            }
        }
        let supervisor = FileProcessSupervisor::new(runtime);
        let mut seen = BTreeSet::new();
        for dir in [supervisor.processes_dir(), supervisor.process_history_dir()] {
            if !dir.exists() {
                continue;
            }
            for file in fs::read_dir(dir).map_err(|e| RefineError::Io(e.to_string()))? {
                let path = file.map_err(|e| RefineError::Io(e.to_string()))?.path();
                // Legacy agents keep completion signals beside process records.
                // They are output artifacts, not ManagedProcess envelopes.
                if path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(".signal.json"))
                {
                    continue;
                }
                if path.extension().and_then(|s| s.to_str()) != Some("json") {
                    continue;
                }
                let bytes = match fs::read(path) {
                    Ok(v) => v,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(e) => return Err(RefineError::Io(e.to_string())),
                };
                let process: ManagedProcess = serde_json::from_slice(&bytes)
                    .map_err(|e| RefineError::Serialization(e.to_string()))?;
                if !seen.insert(process.id.clone()) {
                    continue;
                }
                let metadata: Value =
                    serde_json::from_str(process.details.as_deref().unwrap_or("{}"))
                        .unwrap_or(Value::Null);
                if query
                    .goal_id
                    .as_ref()
                    .is_some_and(|goal| metadata["goal_id"].as_str() != Some(goal))
                {
                    continue;
                }
                // A port can retain processes from previously selected projects.
                if metadata["target_app_id"]
                    .as_str()
                    .is_some_and(|target| !root.starts_with(target))
                {
                    continue;
                }
                for (kind, path) in [
                    ("stdout", process.stdout_path),
                    ("stderr", process.stderr_path),
                ] {
                    if let Some(path) = path {
                        files.push((format!("process:{}:{kind}", process.id), PathBuf::from(path), json!({"log_type":kind,"process_id":process.id,"goal_id":metadata["goal_id"],"category":"process","actor":process.label,"datetime":process.started_at,"severity":"unknown"})));
                    }
                }
            }
        }
    }
    let mut rows = BTreeMap::new();
    let mut cursors = if query.tail {
        query.cursors.clone()
    } else {
        BTreeMap::new()
    };
    let mut total = 0usize;
    let needle = query.q.to_lowercase();
    let keep = query.offset.saturating_add(query.limit).max(1);
    for (key, path, meta) in files {
        if query.tail && total >= query.limit {
            break;
        }
        let file = match fs::File::open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(RefineError::Io(e.to_string())),
        };
        let len = file
            .metadata()
            .map_err(|e| RefineError::Io(e.to_string()))?
            .len();
        let mut reader = BufReader::new(file);
        let mut position = if query.tail {
            match query.cursors.get(&key) {
                Some(position) if *position <= len => *position,
                Some(_) => 0,
                None => len.saturating_sub(16_384),
            }
        } else {
            0
        };
        reader
            .seek(SeekFrom::Start(position))
            .map_err(|e| RefineError::Io(e.to_string()))?;
        if query.tail && !query.cursors.contains_key(&key) && position > 0 {
            let mut partial = Vec::new();
            position += reader
                .read_until(b'\n', &mut partial)
                .map_err(|e| RefineError::Io(e.to_string()))? as u64;
        }
        let start = position;
        let mut index = 0usize;
        loop {
            let mut bytes = Vec::new();
            let count = reader
                .read_until(b'\n', &mut bytes)
                .map_err(|e| RefineError::Io(e.to_string()))?;
            if count == 0 {
                break;
            }
            let at = position;
            position += count as u64;
            let text = String::from_utf8_lossy(&bytes);
            let kind = meta["log_type"].as_str().unwrap();
            let mut row = match kind {
                "event" => match serde_json::from_str::<ActivityEntry>(&text) {
                    Ok(entry) => json!(entry),
                    Err(_) if !bytes.ends_with(b"\n") => {
                        position = at;
                        break;
                    }
                    Err(e) => return Err(RefineError::Serialization(e.to_string())),
                },
                "api" => match serde_json::from_str::<Value>(&text) {
                    Ok(entry) => {
                        let path = entry["path"].as_str().unwrap_or("");
                        let goal = path
                            .split("/goals/")
                            .nth(1)
                            .and_then(|s| s.split('/').next());
                        json!({"id":format!("{key}:{at}"),"datetime":entry["created_at"],"severity":if entry["status"].as_u64().unwrap_or(200)>=400 {"error"} else {"info"},"category":"api","message":format!("{} {} ({})",entry["method"].as_str().unwrap_or(""),path,entry["status"]),"details":entry,"goal_id":goal})
                    }
                    Err(_) if !bytes.ends_with(b"\n") => {
                        position = at;
                        break;
                    }
                    Err(e) => return Err(RefineError::Serialization(e.to_string())),
                },
                "operation" => match serde_json::from_str::<crate::model::log::LogEntry>(&text) {
                    Ok(entry) => {
                        let mut row = json!(entry);
                        row["id"] = json!(format!("{key}:{at}"));
                        row["operation_id"] = meta["operation_id"].clone();
                        row
                    }
                    Err(_) if !bytes.ends_with(b"\n") => {
                        position = at;
                        break;
                    }
                    Err(e) => return Err(RefineError::Serialization(e.to_string())),
                },
                "round" => match super::parse_round_log_line(&text) {
                    Ok(log) => {
                        let mut entry = json!(log.entry);
                        entry["id"] = json!(format!("archive:{key}:{at}"));
                        entry["round"] = json!(log.round_idx.map(|r| r + 1));
                        entry
                    }
                    Err(_) if !bytes.ends_with(b"\n") => {
                        position = at;
                        break;
                    }
                    Err(e) => return Err(RefineError::Serialization(e.to_string())),
                },
                _ => {
                    let mut entry = meta.clone();
                    entry["id"] = json!(format!("{key}:{at}"));
                    if query.tail {
                        entry["datetime"] = json!(chrono::Utc::now().to_rfc3339());
                    }
                    entry["timestamp_kind"] = json!(if query.tail {
                        "received"
                    } else {
                        "process_started"
                    });
                    entry["message"] = json!(text.trim_end_matches(['\r', '\n']));
                    entry
                }
            };
            row["log_type"] = meta["log_type"].clone();
            if row["goal_id"].is_null() {
                row["goal_id"] = meta["goal_id"].clone();
            }
            let matches = query
                .goal_id
                .as_ref()
                .is_none_or(|id| row["goal_id"].as_str() == Some(id))
                && query.filters.iter().all(|(k, v)| {
                    v.is_empty()
                        || row[k]
                            .as_str()
                            .map(str::to_lowercase)
                            .unwrap_or_default()
                            .contains(&v.to_lowercase())
                })
                && (needle.is_empty() || row.to_string().to_lowercase().contains(&needle));
            if matches {
                total += 1;
                let order = (
                    row["datetime"].as_str().unwrap_or("").to_string(),
                    key.clone(),
                    at,
                    index,
                );
                rows.insert(order, row);
                while rows.len() > keep {
                    rows.pop_first();
                }
            }
            index += 1;
            if query.tail && (position - start >= 262_144 || total >= query.limit) {
                break;
            }
        }
        cursors.insert(key, position);
    }
    let activity: Vec<_> = rows
        .into_values()
        .rev()
        .skip(query.offset)
        .take(query.limit)
        .collect();
    Ok(
        json!({"activity":activity,"cursors":cursors,"page":{"total":total,"offset":query.offset,"limit":query.limit,"has_more":query.offset+query.limit<total}}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn searches_old_round_details_and_archived_raw_output_and_tails_without_replay() {
        let root =
            std::env::temp_dir().join(format!("refine-log-archive-{}", uuid::Uuid::new_v4()));
        let state = root.join("state");
        let runtime = root.join("runtime");
        fs::create_dir_all(runtime.join("process-history")).unwrap();
        let logs = super::super::FileLogService::new(&state);
        for index in 0..450 {
            logs.append_round_log(
                "GOAL1",
                0,
                crate::model::log::LogEntry {
                    datetime: format!("2026-09-12T12:{:02}:{:02}Z", index / 60, index % 60),
                    severity: "info".into(),
                    category: "workflow".into(),
                    message: format!("log {index}"),
                    actor: None,
                    goal_id: None,
                    actions: vec![],
                    details: Some(
                        serde_json::from_value(
                            json!({"evidence":if index==0 {"old needle"} else {"ordinary"}}),
                        )
                        .unwrap(),
                    ),
                },
            )
            .unwrap();
        }
        fs::create_dir_all(runtime.join("processes")).unwrap();
        fs::write(
            runtime.join("processes/goal-agent-legacy.signal.json"),
            r#"{"state":"complete","message":"legacy completion"}"#,
        )
        .unwrap();
        let stdout = runtime.join("out.log");
        fs::write(&stdout, "first raw line\nsecond raw line\n").unwrap();
        fs::write(runtime.join("process-history/p1.json"), json!({"id":"p1","owner":"agent","pid":null,"state":"exited","label":"codex","details":json!({"goal_id":"GOAL1"}).to_string(),"stdout_path":stdout,"stderr_path":null,"stdin_path":null,"limits":null,"started_at":"2026-09-12T12:00:00Z","exit_code":0}).to_string()).unwrap();
        let goals = vec!["GOAL1".into()];
        let query = |q: &str| ArchiveQuery {
            goal_id: Some("GOAL1".into()),
            q: q.into(),
            limit: 200,
            ..Default::default()
        };
        let old = query_archive(&state, Some(&runtime), &goals, query("old needle")).unwrap();
        assert_eq!(old["page"]["total"], 1);
        assert_eq!(old["activity"][0]["message"], "log 0");
        let raw = query_archive(&state, Some(&runtime), &goals, query("second raw")).unwrap();
        assert_eq!(raw["activity"][0]["log_type"], "stdout");
        assert_eq!(raw["activity"][0]["process_id"], "p1");
        let cursors = serde_json::from_value(raw["cursors"].clone()).unwrap();
        use std::io::Write;
        writeln!(
            fs::OpenOptions::new().append(true).open(stdout).unwrap(),
            "new raw line"
        )
        .unwrap();
        let tail = query_archive(
            &state,
            Some(&runtime),
            &goals,
            ArchiveQuery {
                tail: true,
                cursors,
                ..query("")
            },
        )
        .unwrap();
        assert_eq!(tail["activity"].as_array().unwrap().len(), 1);
        assert_eq!(tail["activity"][0]["message"], "new raw line");
        let stopped = query_archive(
            &state,
            Some(&runtime),
            &goals,
            ArchiveQuery {
                tail: true,
                cursors: serde_json::from_value(tail["cursors"].clone()).unwrap(),
                ..query("")
            },
        )
        .unwrap();
        assert!(stopped["activity"].as_array().unwrap().is_empty());
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(runtime.join("out.log"))
            .unwrap();
        for index in 0..250 {
            writeln!(file, "burst {index}").unwrap();
        }
        let mut cursors = serde_json::from_value(stopped["cursors"].clone()).unwrap();
        let mut found = BTreeSet::new();
        for _ in 0..5 {
            let page = query_archive(
                &state,
                Some(&runtime),
                &goals,
                ArchiveQuery {
                    tail: true,
                    limit: 50,
                    cursors,
                    ..query("burst")
                },
            )
            .unwrap();
            for row in page["activity"].as_array().unwrap() {
                assert!(found.insert(row["id"].as_str().unwrap().to_string()));
            }
            cursors = serde_json::from_value(page["cursors"].clone()).unwrap();
        }
        assert_eq!(
            found.len(),
            250,
            "a full tail page must not advance past undisplayed output"
        );
        fs::write(runtime.join("out.log"), "after truncation\n").unwrap();
        let reset = query_archive(
            &state,
            Some(&runtime),
            &goals,
            ArchiveQuery {
                tail: true,
                cursors,
                ..query("")
            },
        )
        .unwrap();
        assert_eq!(reset["activity"][0]["message"], "after truncation");
        fs::remove_dir_all(root).unwrap();
    }
}

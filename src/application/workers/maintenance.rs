//! One daemon-maintenance interface for DR74050DA12C5442DF797B8AB6 and workflow liveness.
//! It runs independently of admission, worker replacement and repository cleanup.
use super::*;
use crate::infrastructure::process::subprocess::owned_groups::OwnedGroup;

#[derive(Clone, Debug, Serialize)]
pub struct MaintenanceHealth {
    pub checked_at_ms: i64,
    pub checked_groups: usize,
    pub expired_groups: usize,
    pub failures: Vec<String>,
}

pub fn maintain_daemon(runtime: &Path) -> MaintenanceHealth {
    let mut health = MaintenanceHealth {
        checked_at_ms: chrono::Utc::now().timestamp_millis(),
        checked_groups: 0,
        expired_groups: 0,
        failures: Vec::new(),
    };
    for root in [runtime.to_path_buf(), runtime.join("agents")] {
        let supervisor = FileProcessSupervisor::new(&root);
        let groups = match supervisor.owned_group_observations() {
            Ok(groups) => groups,
            Err(error) => {
                health.failures.push(error.to_string());
                continue;
            }
        };
        for group in groups {
            let group = match group {
                Ok(group) => group,
                Err(error) => {
                    health.failures.push(error.to_string());
                    continue;
                }
            };
            if supervisor
                .assess_owned_group(&group)
                .is_ok_and(|a| !a.pending())
                && !runtime
                    .join("deadline-reconciliation")
                    .join(format!("{}.json", group.process.id))
                    .exists()
            {
                continue;
            }
            health.checked_groups += 1;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                crate::infrastructure::process::supervisor::coordination::with_lock_timeout(
                    Duration::from_millis(200),
                    || maintain_group(runtime, &supervisor, &group, health.checked_at_ms),
                )
            }));
            match result {
                Ok(Ok(expired)) => health.expired_groups += usize::from(expired),
                Ok(Err(error)) => health
                    .failures
                    .push(format!("{}: {error}", group.process.id)),
                Err(_) => health.failures.push(format!(
                    "{}: process maintenance panicked",
                    group.process.id
                )),
            }
        }
    }
    let path = runtime.join("daemon-maintenance.json");
    if let Err(error) = crate::infrastructure::process::subprocess::write_json_atomically(
        &path,
        &serde_json::to_vec(&health).unwrap(),
        "daemon maintenance health",
    ) {
        health.failures.push(error.to_string());
        eprintln!("refine daemon maintenance evidence: {error}");
    }
    health
}

fn maintain_group(
    runtime: &Path,
    supervisor: &FileProcessSupervisor,
    group: &OwnedGroup,
    now: i64,
) -> RefineResult<bool> {
    let pending_path = runtime
        .join("deadline-reconciliation")
        .join(format!("{}.json", group.process.id));
    let pending: Option<Value> = match std::fs::read(&pending_path) {
        Ok(bytes) => Some(
            serde_json::from_slice(&bytes)
                .map_err(|e| RefineError::Serialization(e.to_string()))?,
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(RefineError::Io(e.to_string())),
    };
    if pending.as_ref().is_some_and(|v| v["settled"] == true) {
        return Ok(false);
    }
    let current = match supervisor.inspect(&group.process.id) {
        Ok(process) => process,
        Err(RefineError::NotFound(_)) => group.process.clone(),
        Err(error) => return Err(error),
    };
    let metadata = current
        .details
        .as_deref()
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .unwrap_or(Value::Null);
    // Observe even protected sessions so surviving descendants retain witnessed ownership.
    let observed = supervisor.observe_owned_group(group)?;
    if observed.confirmed_exit && pending.is_none() {
        return Ok(false);
    }
    if metadata["toolbar_timeout_protected"] == true {
        return match &observed.ownership_gap {
            Some(reason) => Err(RefineError::Degraded(format!(
                "{reason}; protected session evidence retained"
            ))),
            None => Ok(false),
        };
    }
    let original = group
        .process
        .details
        .as_deref()
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .unwrap_or(Value::Null);
    let start = group
        .process
        .started_at
        .parse::<i64>()
        .ok()
        .or_else(|| {
            chrono::DateTime::parse_from_rfc3339(&group.process.started_at)
                .ok()
                .map(|d| d.timestamp_millis())
        })
        .ok_or_else(|| RefineError::Degraded("original process start time unavailable".into()))?;
    let hard_cap = original["agent_hard_cap_millis"]
        .as_i64()
        .filter(|v| *v > 0);
    let idle = original["agent_idle_timeout_millis"]
        .as_i64()
        .filter(|v| *v > 0);
    // The session controller treats attached input, resize commands and signal-file writes
    // as activity too. Use the same inspectable files when its poller is unavailable.
    let last_activity = [&current.stdout_path, &current.stderr_path]
        .into_iter()
        .flatten()
        .map(String::as_str)
        .chain(
            ["command_path", "signal_path"]
                .into_iter()
                .filter_map(|key| metadata.get(key).and_then(Value::as_str)),
        )
        .filter_map(|path| std::fs::metadata(path).ok().and_then(|m| m.modified().ok()))
        .filter_map(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .max();
    let expired_hard = hard_cap.is_some_and(|cap| now >= start.saturating_add(cap));
    let expired_idle = metadata["attention_state"] != "needs_input"
        && idle.is_some_and(|idle| {
            now >= last_activity
                .unwrap_or(start)
                .max(start)
                .saturating_add(idle)
        });
    if !expired_hard && !expired_idle && pending.is_none() {
        if let Some(reason) = &observed.ownership_gap {
            return Err(RefineError::Degraded(format!(
                "{reason}; capacity and evidence retained; inspect process diagnostics"
            )));
        }
        return Ok(false);
    }
    let operation = if let Some(pending) = &pending {
        serde_json::from_value::<Option<OperationHandle>>(pending["operation"].clone())
            .map_err(|e| RefineError::Serialization(e.to_string()))?
    } else {
        metadata["operation_id"]
            .as_str()
            .map(|id| FileOperationRegistry::new(runtime).status(id))
            .transpose()?
    };
    let evidence = json!({"process_id": current.id, "original_started_at": group.process.started_at,
        "hard_cap_expired": expired_hard, "idle_expired": expired_idle, "operation": operation, "settled": false});
    write_deadline_evidence(&pending_path, &evidence)?;
    let stopped = supervisor.stop_owned_group(&observed, Duration::from_millis(200))?;
    if !stopped.confirmed_exit {
        return Err(RefineError::Degraded(
            "deadline process-group exit unconfirmed".into(),
        ));
    }
    // Process settlement is deliberately evidence-only here. The owning Goal attempt retains
    // its Round authority; operation reconciliation below only settles the exact live operation.
    if let Some(operation) = operation {
        FileOperationRegistry::new(runtime).interrupt_after_process_exit(
            &operation.id,
            operation.revision,
            || {
                for root in [runtime.to_path_buf(), runtime.join("agents")] {
                    let supervisor = FileProcessSupervisor::new(root);
                    for group in supervisor.owned_groups()? {
                        let details: Value = group
                            .process
                            .details
                            .as_deref()
                            .and_then(|s| serde_json::from_str(s).ok())
                            .unwrap_or(Value::Null);
                        if details["operation_id"].as_str() == Some(&operation.id)
                            && !supervisor.observe_owned_group(&group)?.confirmed_exit
                        {
                            return Ok(false);
                        }
                    }
                }
                Ok(true)
            },
        )?;
    }
    let mut settled = evidence;
    settled["settled"] = json!(true);
    settled["confirmed_exit"] = json!(true);
    write_deadline_evidence(&pending_path, &settled)?;
    Ok(true)
}

fn write_deadline_evidence(path: &Path, value: &Value) -> RefineResult<()> {
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| RefineError::Io(e.to_string()))?;
    crate::infrastructure::process::subprocess::write_json_atomically(
        path,
        &serde_json::to_vec(value).unwrap(),
        "deadline reconciliation",
    )
}

#[cfg(all(test, target_os = "linux"))]
mod tests;

pub fn inspect_health(runtime: &Path) -> Value {
    let value = std::fs::read(runtime.join("daemon-maintenance.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    match value {
        Some(mut value) => {
            let fresh = value["checked_at_ms"].as_i64().is_some_and(|at| {
                (0..30_000).contains(&(chrono::Utc::now().timestamp_millis() - at))
            });
            value["state"] = json!(if !fresh {
                "unavailable"
            } else if value["failures"].as_array().is_some_and(|v| v.is_empty()) {
                "healthy"
            } else {
                "unhealthy"
            });
            value
        }
        None => {
            json!({"state":"unavailable", "reason":"daemon maintenance evidence is missing or unreadable"})
        }
    }
}

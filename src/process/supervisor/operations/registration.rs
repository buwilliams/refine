use super::*;

impl FileOperationRegistry {
    pub fn register_with_id(
        &self,
        operation_id: &str,
        owner: &str,
        request: Value,
    ) -> RefineResult<OperationHandle> {
        let lock = self.mutation_lock()?;
        if self.operation_path(operation_id).exists() {
            let existing = self.status(operation_id)?;
            FileExt::unlock(&lock).ok();
            return Ok(existing);
        }
        let handle = OperationHandle {
            schema_version: 1,
            revision: 0,
            id: operation_id.to_string(),
            owner: owner.to_string(),
            state: OperationState::Running,
            request,
            progress: empty_object(),
            result: empty_object(),
            error: None,
            external_attempt: None,
        };
        self.write(&handle)?;
        FileExt::unlock(&lock).ok();
        self.append_log(
            &handle.id,
            operation_log_entry(&handle, "info", "Operation registered", None),
        )?;
        Ok(handle)
    }

    pub fn register_with_request(
        &self,
        owner: &str,
        request: Value,
    ) -> RefineResult<OperationHandle> {
        let lock = self.mutation_lock()?;
        let handle = OperationHandle {
            schema_version: 1,
            revision: 0,
            id: new_operation_id(),
            owner: owner.to_string(),
            state: OperationState::Running,
            request,
            progress: empty_object(),
            result: empty_object(),
            error: None,
            external_attempt: None,
        };
        self.write(&handle)?;
        FileExt::unlock(&lock).ok();
        self.append_log(
            &handle.id,
            operation_log_entry(&handle, "info", "Operation registered", None),
        )?;
        Ok(handle)
    }

    /// Atomically registers one active operation for an ownership key.
    ///
    /// Quality uses a Goal-and-candidate ownership key so manual and workflow callers cannot
    /// evaluate or mutate the same candidate concurrently. The operation registry mutation lock
    /// makes the exclusion effective across threads and daemon processes.
    pub fn register_exclusive_with_request(
        &self,
        owner: &str,
        request: Value,
    ) -> RefineResult<OperationHandle> {
        let lock = self.mutation_lock()?;
        if let Some(active) = self
            .recover()?
            .into_iter()
            .find(|operation| operation.owner == owner && operation_active(&operation.state))
        {
            FileExt::unlock(&lock).ok();
            return Err(RefineError::Conflict(format!(
                "Operation {} already owns {owner}",
                active.id
            )));
        }
        let handle = OperationHandle {
            schema_version: 1,
            revision: 0,
            id: new_operation_id(),
            owner: owner.to_string(),
            state: OperationState::Running,
            request,
            progress: empty_object(),
            result: empty_object(),
            error: None,
            external_attempt: None,
        };
        self.write(&handle)?;
        FileExt::unlock(&lock).ok();
        self.append_log(
            &handle.id,
            operation_log_entry(&handle, "info", "Operation registered", None),
        )?;
        Ok(handle)
    }

    /// Atomically creates or recovers a durable replacement for an interrupted operation.
    ///
    /// The source id and retry identity are written into the replacement request while the
    /// registry mutation lock is held. Concurrent callers, including callers in separate daemon
    /// processes, therefore either create one replacement or recover that exact same record.
    pub fn find_or_register_replacement(
        &self,
        owner: &str,
        source_operation_id: &str,
        retry_identity: &str,
        mut request: Value,
    ) -> RefineResult<ReplacementOperationRegistration> {
        if retry_identity.trim().is_empty() {
            return Err(RefineError::InvalidInput(
                "Replacement operation retry identity is required".to_string(),
            ));
        }

        let lock = self.mutation_lock()?;
        let source = self.status(source_operation_id)?;
        if source.owner != owner || !matches!(source.state, OperationState::Interrupted) {
            return Err(RefineError::Conflict(format!(
                "Only interrupted {owner} operations can be recovered"
            )));
        }

        let request_object = request.as_object_mut().ok_or_else(|| {
            RefineError::InvalidInput(
                "Replacement operation request must be a JSON object".to_string(),
            )
        })?;
        request_object.insert("recovery_of".to_string(), json!(source_operation_id));
        request_object.insert("retry_identity".to_string(), json!(retry_identity));

        let mut legacy_replacement = None;
        for mut operation in self.recover()? {
            let operation_retry_identity = operation
                .request
                .get("retry_identity")
                .and_then(Value::as_str);
            let operation_source = operation.request.get("recovery_of").and_then(Value::as_str);

            if operation_retry_identity == Some(retry_identity) {
                if operation.owner != owner || operation_source != Some(source_operation_id) {
                    return Err(RefineError::Conflict(format!(
                        "Retry identity {retry_identity} is already assigned to another operation"
                    )));
                }
                drop(lock);
                return Ok(ReplacementOperationRegistration {
                    operation,
                    created: false,
                });
            }

            // Round 6 replacements predate the explicit retry identity. Adopt that durable record
            // instead of creating a duplicate after an upgrade or restart.
            if operation.owner == owner
                && operation_source == Some(source_operation_id)
                && operation_retry_identity.is_none()
            {
                let operation_request = operation.request.as_object_mut().ok_or_else(|| {
                    RefineError::Serialization(format!(
                        "replacement operation {} request is not a JSON object",
                        operation.id
                    ))
                })?;
                operation_request.insert("retry_identity".to_string(), json!(retry_identity));
                legacy_replacement = Some(operation);
                break;
            }
        }

        if let Some(operation) = legacy_replacement {
            self.write(&operation)?;
            drop(lock);
            return Ok(ReplacementOperationRegistration {
                operation,
                created: false,
            });
        }

        let operation = OperationHandle {
            schema_version: 1,
            revision: 0,
            id: new_operation_id(),
            owner: owner.to_string(),
            state: OperationState::Running,
            request,
            progress: empty_object(),
            result: empty_object(),
            error: None,
            external_attempt: None,
        };
        self.write(&operation)?;
        self.append_log(
            &operation.id,
            operation_log_entry(&operation, "info", "Operation registered", None),
        )?;
        drop(lock);
        Ok(ReplacementOperationRegistration {
            operation,
            created: true,
        })
    }

    pub fn append_log(&self, operation_id: &str, mut entry: LogEntry) -> RefineResult<LogEntry> {
        let operation = self.status(operation_id)?;
        if entry.datetime.trim().is_empty() {
            entry.datetime = now_timestamp();
        }
        if entry.category.trim().is_empty() {
            entry.category = "operation".to_string();
        }
        if entry.actor.is_none() {
            entry.actor = Some("refine".to_string());
        }
        let mut details = entry.details.unwrap_or_default();
        details
            .entry("operation_id".to_string())
            .or_insert_with(|| json!(operation.id));
        details
            .entry("owner".to_string())
            .or_insert_with(|| json!(operation.owner));
        details
            .entry("state".to_string())
            .or_insert_with(|| json!(operation.state));
        entry.details = Some(details);
        fs::create_dir_all(self.operations_dir()).map_err(|error| {
            RefineError::Io(format!(
                "failed to create operation log registry {}: {error}",
                self.operations_dir().display()
            ))
        })?;
        let path = self.log_path(operation_id);
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to open operation log {}: {error}",
                    path.display()
                ))
            })?;
        let encoded = serde_json::to_string(&entry).map_err(|error| {
            RefineError::Serialization(format!("failed to encode operation log: {error}"))
        })?;
        writeln!(file, "{encoded}").map_err(|error| {
            RefineError::Io(format!(
                "failed to append operation log {}: {error}",
                path.display()
            ))
        })?;
        Ok(entry)
    }

    pub fn page_logs(
        &self,
        operation_id: &str,
        limit: usize,
        offset: usize,
    ) -> RefineResult<(Vec<LogEntry>, bool, usize)> {
        self.status(operation_id)?;
        let path = self.log_path(operation_id);
        if !path.exists() {
            return Ok((Vec::new(), false, 0));
        }
        let file = fs::File::open(&path).map_err(|error| {
            RefineError::Io(format!(
                "failed to open operation log {}: {error}",
                path.display()
            ))
        })?;
        let mut entries = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line.map_err(|error| {
                RefineError::Io(format!(
                    "failed to read operation log {}: {error}",
                    path.display()
                ))
            })?;
            if line.trim().is_empty() {
                continue;
            }
            let entry = serde_json::from_str::<LogEntry>(&line).map_err(|error| {
                RefineError::Serialization(format!(
                    "failed to parse operation log {}: {error}",
                    path.display()
                ))
            })?;
            entries.push(entry);
        }
        entries.sort_by(|a, b| {
            a.datetime
                .cmp(&b.datetime)
                .then_with(|| a.message.cmp(&b.message))
        });
        let total = entries.len();
        let limit = limit.clamp(1, 200);
        let page = entries
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect::<Vec<_>>();
        let has_more = offset + page.len() < total;
        Ok((page, has_more, total))
    }
}

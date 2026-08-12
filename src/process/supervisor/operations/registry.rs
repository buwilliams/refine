use super::*;

impl OperationRegistry for FileOperationRegistry {
    fn register(&self, owner: &str) -> RefineResult<OperationHandle> {
        self.register_with_request(owner, empty_object())
    }

    fn status(&self, operation_id: &str) -> RefineResult<OperationHandle> {
        let path = self.operation_path(operation_id);
        let bytes = fs::read(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                return RefineError::NotFound(format!("Operation {operation_id} was not found"));
            }
            RefineError::Io(format!(
                "failed to read operation {}: {error}",
                path.display()
            ))
        })?;
        serde_json::from_slice(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse operation {}: {error}",
                path.display()
            ))
        })
    }

    fn cancel(&self, operation_id: &str) -> RefineResult<OperationHandle> {
        let lock = self.mutation_lock()?;
        let mut handle = self.status(operation_id)?;
        if operation_terminal(&handle.state) && !matches!(handle.state, OperationState::Interrupted)
        {
            FileExt::unlock(&lock).ok();
            return Ok(handle);
        }
        if matches!(handle.state, OperationState::Cancelling)
            && cancellation_terminal_is_deferred(&handle)
        {
            FileExt::unlock(&lock).ok();
            return Ok(handle);
        }
        let deferred = handle
            .request
            .get("defer_cancellation_terminal")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if deferred && let Some(request) = handle.request.as_object_mut() {
            request.insert("cancellation_requested".to_string(), json!(true));
        }
        handle.state = if deferred {
            OperationState::Cancelling
        } else {
            OperationState::Cancelled
        };
        self.write(&handle)?;
        FileExt::unlock(&lock).ok();
        self.append_log(
            &handle.id,
            operation_log_entry(
                &handle,
                "warning",
                if deferred {
                    "Operation cancellation requested"
                } else {
                    "Operation cancelled"
                },
                None,
            ),
        )?;
        Ok(handle)
    }

    fn recover(&self) -> RefineResult<Vec<OperationHandle>> {
        let dir = self.operations_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut operations = Vec::new();
        for entry in fs::read_dir(&dir).map_err(|error| {
            RefineError::Io(format!(
                "failed to read operation registry {}: {error}",
                dir.display()
            ))
        })? {
            let entry = entry.map_err(|error| {
                RefineError::Io(format!(
                    "failed to inspect operation registry entry: {error}"
                ))
            })?;
            if entry.path().extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let bytes = fs::read(entry.path()).map_err(|error| {
                RefineError::Io(format!(
                    "failed to read operation {}: {error}",
                    entry.path().display()
                ))
            })?;
            let operation = serde_json::from_slice::<OperationHandle>(&bytes).map_err(|error| {
                RefineError::Serialization(format!(
                    "failed to parse operation {}: {error}",
                    entry.path().display()
                ))
            })?;
            operations.push(operation);
        }
        operations.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(operations)
    }
}

impl FileOperationRegistry {
    /// Revision-fenced mutation used by capabilities that temporarily hand
    /// ownership to a process outside the supervisor being replaced.
    pub fn compare_and_set(
        &self,
        operation_id: &str,
        expected_revision: u64,
        transition: impl FnOnce(&mut OperationHandle) -> RefineResult<()>,
    ) -> RefineResult<OperationHandle> {
        let lock = self.mutation_lock()?;
        let mut handle = self.status(operation_id)?;
        if handle.revision != expected_revision {
            FileExt::unlock(&lock).ok();
            return Err(RefineError::Conflict(format!(
                "Operation {operation_id} revision changed from {expected_revision} to {}",
                handle.revision
            )));
        }
        transition(&mut handle)?;
        handle.schema_version = operation_schema_version();
        handle.revision = handle.revision.saturating_add(1);
        self.write(&handle)?;
        FileExt::unlock(&lock).ok();
        Ok(handle)
    }
}

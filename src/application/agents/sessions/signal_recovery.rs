use super::*;

#[derive(Debug)]
pub(super) enum InvalidSignalDisposition {
    Retry(String),
    Fail(RefineError),
}

#[derive(Debug)]
pub(super) struct InvalidSignalRecovery {
    rejected_payloads: usize,
    replacement_limit: usize,
    last_rejection: Option<InvalidSignalEvidence>,
}

#[derive(Clone, Debug)]
struct InvalidSignalEvidence {
    archive_path: PathBuf,
    diagnostic_path: PathBuf,
    diagnostic: String,
}

impl Default for InvalidSignalRecovery {
    fn default() -> Self {
        Self {
            rejected_payloads: 0,
            replacement_limit: MAX_INVALID_SIGNAL_REPLACEMENTS,
            last_rejection: None,
        }
    }
}

impl InvalidSignalRecovery {
    pub(super) fn reject_malformed_transport(
        &mut self,
        signal_path: &Path,
        diagnostic: &str,
        agent_running: bool,
    ) -> RefineResult<InvalidSignalDisposition> {
        let evidence = self.preserve(signal_path, diagnostic)?;
        let attempt = self.rejected_payloads;

        if !agent_running || attempt > self.replacement_limit {
            let reason = if agent_running {
                format!(
                    "exhausted the limit of {} rejected replacement payloads",
                    self.replacement_limit
                )
            } else {
                "the Goal Agent exited before it could rewrite the payload".to_string()
            };
            return Ok(InvalidSignalDisposition::Fail(RefineError::InvalidInput(
                format!(
                    "Goal Agent completion signal {} was rejected ({reason}); final diagnostic: {diagnostic}; invalid payload preserved at {} and diagnostic preserved at {}",
                    signal_path.display(),
                    evidence.archive_path.display(),
                    evidence.diagnostic_path.display()
                ),
            )));
        }

        Ok(InvalidSignalDisposition::Retry(format!(
            "Refine rejected your completion signal: {diagnostic}. Rewrite it with exactly the same required JSON shape (replacement attempt {attempt} of {limit}). Write and parse-check `{signal_path}.tmp`, then atomically rename it over `{signal_path}`.\r",
            limit = self.replacement_limit,
            signal_path = signal_path.display()
        )))
    }

    pub(super) fn reject_invalid_contract(
        &mut self,
        signal_path: &Path,
        diagnostic: &str,
    ) -> RefineResult<RefineError> {
        let evidence = self.preserve(signal_path, diagnostic)?;
        Ok(RefineError::InvalidInput(format!(
            "Goal Agent completion signal {} was rejected because typed-schema failures are terminal; final diagnostic: {diagnostic}; invalid payload preserved at {} and diagnostic preserved at {}",
            signal_path.display(),
            evidence.archive_path.display(),
            evidence.diagnostic_path.display()
        )))
    }

    fn preserve(
        &mut self,
        signal_path: &Path,
        diagnostic: &str,
    ) -> RefineResult<InvalidSignalEvidence> {
        self.rejected_payloads += 1;
        let attempt = self.rejected_payloads;
        let archive_path = invalid_signal_archive_path(signal_path, attempt)?;
        fs::rename(signal_path, &archive_path).map_err(|error| {
            RefineError::Io(format!(
                "failed to preserve rejected Goal Agent signal {} as {}: {error}",
                signal_path.display(),
                archive_path.display()
            ))
        })?;
        let diagnostic_path = archive_path.with_extension("error.txt");
        fs::write(&diagnostic_path, format!("{diagnostic}\n")).map_err(|error| {
            RefineError::Io(format!(
                "failed to preserve rejected Goal Agent signal diagnostic {}: {error}",
                diagnostic_path.display()
            ))
        })?;
        let evidence = InvalidSignalEvidence {
            archive_path: archive_path.clone(),
            diagnostic_path: diagnostic_path.clone(),
            diagnostic: diagnostic.to_string(),
        };
        self.last_rejection = Some(evidence.clone());
        Ok(evidence)
    }

    pub(super) fn accept_valid(&mut self) {
        self.rejected_payloads = 0;
        self.last_rejection = None;
    }

    pub(super) fn agent_exited_without_replacement(
        &self,
        signal_path: &Path,
    ) -> Option<RefineError> {
        let evidence = self.last_rejection.as_ref()?;
        Some(RefineError::InvalidInput(format!(
            "Goal Agent completion signal {} was rejected and the Goal Agent exited before it produced a valid replacement; final diagnostic: {}; invalid payload preserved at {} and diagnostic preserved at {}",
            signal_path.display(),
            evidence.diagnostic,
            evidence.archive_path.display(),
            evidence.diagnostic_path.display()
        )))
    }
}

fn invalid_signal_archive_path(signal_path: &Path, attempt: usize) -> RefineResult<PathBuf> {
    let file_name = signal_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            RefineError::InvalidInput(format!(
                "Goal Agent signal path {} has no valid file name",
                signal_path.display()
            ))
        })?;
    let process_id = file_name.strip_suffix(".signal.json").ok_or_else(|| {
        RefineError::InvalidInput(format!(
            "Goal Agent signal path {} does not end in .signal.json",
            signal_path.display()
        ))
    })?;
    Ok(signal_path.with_file_name(format!("{process_id}.signal.invalid.{attempt}.json")))
}

use super::*;
use std::path::Path;

#[derive(Clone, Debug)]
pub(super) struct LegacyRegistrationBackup {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
    pub journal_dir: PathBuf,
}

impl FileInstallationService {
    pub(super) fn legacy_external_runtime_root(&self, target: &InstallTarget) -> Option<String> {
        let os = match target {
            InstallTarget::MacosDaemon => RuntimeOs::Macos,
            InstallTarget::WindowsDaemon => RuntimeOs::Windows,
            InstallTarget::LinuxCliWeb => RuntimeOs::Linux,
        };
        let root =
            RuntimePathLayout::legacy_external_for_os(os, DEFAULT_APP_ID, self.path_inputs.clone())
                .runtime_root;
        if root == self.runtime_root {
            return None;
        }
        let state_root = self
            .port
            .map(|port| root.join(port.to_string()))
            .unwrap_or_else(|| root.clone());
        (state_root.join(INSTALL_STATE_FILE).exists()
            || state_root.join(INSTALL_BACKEND_FILE).exists()
            || directory_has_entries(&state_root))
        .then(|| root.display().to_string())
    }

    pub(super) fn detect_external_registration(
        &self,
        backend: &mut InstallBackendRegistration,
        repair_legacy: bool,
    ) -> RefineResult<Option<LegacyRegistrationBackup>> {
        let Some(path) = backend.service_metadata_path.as_deref().map(PathBuf::from) else {
            return Ok(None);
        };
        let Ok(metadata) = fs::read_to_string(&path) else {
            return Ok(None);
        };
        let registered_runtime = service_metadata_runtime_root(&backend.target, &metadata);
        let registered_executable = service_metadata_executable(&backend.target, &metadata);
        let registered_working_directory =
            service_metadata_working_directory(&backend.target, &metadata);
        let expected_executable = self.checkout_root.join("bin/refine");
        let expected_port = self.port;
        let registered_port = service_metadata_port(&backend.target, &metadata);
        if registered_runtime.as_deref() == Some(self.runtime_root.as_path())
            && registered_executable.as_deref() == Some(expected_executable.as_path())
            && registered_working_directory.as_deref() == Some(self.checkout_root.as_path())
            && registered_port == expected_port
        {
            return Ok(None);
        }
        if registered_runtime.is_none()
            && registered_executable.is_none()
            && registered_working_directory.is_none()
            && !metadata.contains("runtime-root")
        {
            return Ok(None);
        }
        let diagnostic = format!(
            "legacy Refine daemon registration {} points to executable {}, working directory {}, runtime {}, and port {:?}; expected {}, {}, {}, and {:?}; external runtime and binary trees were left unchanged",
            path.display(),
            display_optional_path(registered_executable.as_deref()),
            display_optional_path(registered_working_directory.as_deref()),
            display_optional_path(registered_runtime.as_deref()),
            registered_port,
            expected_executable.display(),
            self.checkout_root.display(),
            self.runtime_root.display(),
            expected_port,
        );
        if !repair_legacy {
            return Err(RefineError::Conflict(format!(
                "{diagnostic}; run `./r system repair --port {}` from this checkout to audit and rewrite only the daemon registration",
                self.port.unwrap_or(8082)
            )));
        }
        let backup = self.persist_legacy_registration_evidence(
            &path,
            &metadata,
            registered_executable.as_deref(),
            registered_working_directory.as_deref(),
            registered_runtime.as_deref(),
            registered_port,
        )?;
        backend.notes.push(format!(
            "{diagnostic}; explicit repair preserved audit evidence before rewriting the registration"
        ));
        Ok(Some(backup))
    }

    fn persist_legacy_registration_evidence(
        &self,
        path: &Path,
        metadata: &str,
        executable: Option<&Path>,
        working_directory: Option<&Path>,
        runtime_root: Option<&Path>,
        port: Option<u16>,
    ) -> RefineResult<LegacyRegistrationBackup> {
        use sha2::{Digest, Sha256};

        let digest = format!("{:x}", Sha256::digest(metadata.as_bytes()));
        let migration_root = self.state_root().join("installation-migrations");
        fs::create_dir_all(&migration_root).map_err(|error| {
            RefineError::Io(format!(
                "failed to create installation migration journal {}: {error}",
                migration_root.display()
            ))
        })?;
        let migration = migration_root.join(format!(
            "{}-{}-{}",
            now_timestamp().replace(':', "-"),
            &digest[..12],
            uuid::Uuid::new_v4().simple()
        ));
        fs::create_dir(&migration).map_err(|error| {
            RefineError::Io(format!(
                "failed to create unique installation migration journal {}: {error}",
                migration.display()
            ))
        })?;
        super::service_registration::atomic_write(
            &migration.join("registration.original"),
            metadata.as_bytes(),
        )?;
        let manifest = serde_json::json!({
            "schema_version": 1,
            "outcome": "backup_persisted",
            "created_at": now_timestamp(),
            "original_path": path.display().to_string(),
            "original_sha256": digest,
            "original_bytes": metadata.len(),
            "parsed_executable": executable.map(|value| value.display().to_string()),
            "parsed_working_directory": working_directory.map(|value| value.display().to_string()),
            "parsed_runtime_root": runtime_root.map(|value| value.display().to_string()),
            "parsed_port": port,
            "expected_checkout": self.checkout_root.display().to_string(),
            "expected_runtime_root": self.runtime_root.display().to_string(),
            "expected_port": self.port,
        });
        super::service_registration::atomic_write(
            &migration.join("manifest.json"),
            &serde_json::to_vec_pretty(&manifest).map_err(|error| {
                RefineError::Serialization(format!("failed to encode migration manifest: {error}"))
            })?,
        )?;
        Ok(LegacyRegistrationBackup {
            path: path.to_path_buf(),
            bytes: metadata.as_bytes().to_vec(),
            journal_dir: migration,
        })
    }

    pub(super) fn record_legacy_migration_outcome(
        &self,
        backup: &LegacyRegistrationBackup,
        outcome: &str,
        detail: Option<&str>,
    ) -> RefineResult<()> {
        let path = backup.journal_dir.join("manifest.json");
        let bytes = fs::read(&path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read migration manifest {}: {error}",
                path.display()
            ))
        })?;
        let mut manifest: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| {
            RefineError::Serialization(format!("failed to parse migration manifest: {error}"))
        })?;
        manifest["outcome"] = serde_json::Value::String(outcome.to_string());
        manifest["completed_at"] = serde_json::Value::String(now_timestamp());
        if let Some(detail) = detail {
            manifest["detail"] = serde_json::Value::String(detail.to_string());
        }
        super::service_registration::atomic_write(
            &path,
            &serde_json::to_vec_pretty(&manifest).map_err(|error| {
                RefineError::Serialization(format!("failed to encode migration outcome: {error}"))
            })?,
        )
    }
}

fn directory_has_entries(path: &Path) -> bool {
    fs::read_dir(path)
        .ok()
        .and_then(|mut entries| entries.next())
        .is_some()
}

fn service_metadata_executable(target: &InstallTarget, metadata: &str) -> Option<PathBuf> {
    match target {
        InstallTarget::LinuxCliWeb => metadata
            .lines()
            .find_map(|line| line.trim().strip_prefix("ExecStart="))
            .and_then(|command| {
                state::parse_systemd_exec_arguments(command)
                    .into_iter()
                    .next()
            })
            .map(PathBuf::from),
        InstallTarget::MacosDaemon => plist_program_arguments(metadata).first().map(PathBuf::from),
        InstallTarget::WindowsDaemon => json_string(metadata, "executable").map(PathBuf::from),
    }
}

fn service_metadata_working_directory(target: &InstallTarget, metadata: &str) -> Option<PathBuf> {
    match target {
        InstallTarget::LinuxCliWeb => metadata
            .lines()
            .find_map(|line| line.trim().strip_prefix("WorkingDirectory="))
            .map(|value| PathBuf::from(value.replace("%%", "%"))),
        InstallTarget::MacosDaemon => plist_value(metadata, "WorkingDirectory"),
        InstallTarget::WindowsDaemon => {
            json_string(metadata, "working_directory").map(PathBuf::from)
        }
    }
}

fn service_metadata_runtime_root(target: &InstallTarget, metadata: &str) -> Option<PathBuf> {
    service_arguments(target, metadata)
        .windows(2)
        .find_map(|pair| (pair[0] == "--runtime-root").then(|| PathBuf::from(&pair[1])))
}

fn service_metadata_port(target: &InstallTarget, metadata: &str) -> Option<u16> {
    service_arguments(target, metadata)
        .windows(2)
        .find_map(|pair| {
            (pair[0] == "--port")
                .then(|| pair[1].parse().ok())
                .flatten()
        })
}

fn service_arguments(target: &InstallTarget, metadata: &str) -> Vec<String> {
    match target {
        InstallTarget::LinuxCliWeb => metadata
            .lines()
            .find_map(|line| line.trim().strip_prefix("ExecStart="))
            .map(state::parse_systemd_exec_arguments)
            .unwrap_or_default(),
        InstallTarget::MacosDaemon => plist_program_arguments(metadata),
        InstallTarget::WindowsDaemon => serde_json::from_str::<serde_json::Value>(metadata)
            .ok()
            .and_then(|value| {
                value
                    .get("arguments")
                    .and_then(serde_json::Value::as_array)
                    .cloned()
            })
            .unwrap_or_default()
            .into_iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect(),
    }
}

fn plist_program_arguments(metadata: &str) -> Vec<String> {
    metadata
        .split_once("<key>ProgramArguments</key>")
        .map(|(_, remaining)| remaining)
        .and_then(|remaining| remaining.split_once("</array>").map(|(array, _)| array))
        .unwrap_or_default()
        .split("<string>")
        .skip(1)
        .filter_map(|value| {
            value
                .split_once("</string>")
                .map(|(value, _)| xml_unescape(value))
        })
        .collect()
}

fn plist_value(metadata: &str, key: &str) -> Option<PathBuf> {
    metadata
        .split_once(&format!("<key>{key}</key>"))
        .and_then(|(_, remaining)| remaining.split_once("<string>"))
        .and_then(|(_, value)| value.split_once("</string>"))
        .map(|(value, _)| PathBuf::from(xml_unescape(value)))
}

fn json_string(metadata: &str, key: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(metadata)
        .ok()
        .and_then(|value| {
            value
                .get(key)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
}

fn display_optional_path(path: Option<&Path>) -> String {
    path.map(|path| path.display().to_string())
        .unwrap_or_else(|| "an unparseable path".to_string())
}

fn xml_unescape(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&gt;", ">")
        .replace("&lt;", "<")
        .replace("&amp;", "&")
}

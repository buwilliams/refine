use std::path::{Path, PathBuf};
use std::{fs::OpenOptions, io::Write};

use serde::{Deserialize, Serialize};

use super::*;

const SERVICE_REGISTRATION_BACKUP_FILE: &str = "service-registration-backup.json";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct ServiceRegistrationBackup {
    service_manager: String,
    metadata_path: String,
    metadata: String,
    candidate_executable: String,
    created_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceRegistrationUpdate {
    pub service_manager: String,
    pub candidate_executable: PathBuf,
}

impl FileInstallationService {
    pub(crate) fn prepare_service_executable(
        &self,
        executable: &Path,
    ) -> RefineResult<Option<ServiceRegistrationUpdate>> {
        let status = self.status()?;
        let Some(backend) = status.backend.filter(|backend| {
            status.installed
                && backend.registered
                && backend.activated
                && matches!(
                    backend.target,
                    InstallTarget::LinuxCliWeb | InstallTarget::MacOsAppBundle
                )
        }) else {
            return Ok(None);
        };
        let metadata_path = backend
            .service_metadata_path
            .as_deref()
            .map(PathBuf::from)
            .ok_or_else(|| {
                RefineError::Conflict(
                    "activated service registration has no metadata path".to_string(),
                )
            })?;
        let prior_metadata = fs::read_to_string(&metadata_path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read service registration {} before source promotion: {error}",
                metadata_path.display()
            ))
        })?;
        let replacement = self.service_metadata_for_executable(&backend, executable)?;
        let backup = ServiceRegistrationBackup {
            service_manager: backend.service_manager.clone(),
            metadata_path: metadata_path.display().to_string(),
            metadata: prior_metadata,
            candidate_executable: executable.display().to_string(),
            created_at: now_timestamp(),
        };
        if self.registration_backup_path().exists() {
            return Err(RefineError::Conflict(format!(
                "a prior source-promotion service registration backup remains at {}; restore or reconcile it before preparing another candidate",
                self.registration_backup_path().display()
            )));
        }
        self.save_registration_backup(&backup)?;
        if let Err(error) = atomic_write(&metadata_path, replacement.as_bytes()) {
            let _ = self.clear_registration_backup();
            return Err(error);
        }
        if let Err(error) = self.reload_registration(&backend) {
            let restore = atomic_write(&metadata_path, backup.metadata.as_bytes())
                .and_then(|_| self.reload_registration(&backend));
            if restore.is_ok() {
                let _ = self.clear_registration_backup();
            }
            return Err(RefineError::Degraded(format!(
                "failed to reload {} registration for candidate {}: {error}; registration rollback {}",
                backend.service_manager,
                executable.display(),
                if restore.is_ok() {
                    "succeeded"
                } else {
                    "failed"
                }
            )));
        }
        Ok(Some(ServiceRegistrationUpdate {
            service_manager: backup.service_manager,
            candidate_executable: executable.to_path_buf(),
        }))
    }

    pub(crate) fn restore_service_executable(&self) -> RefineResult<bool> {
        let Some(backup) = self.load_registration_backup()? else {
            return Ok(false);
        };
        let backend = self.load_backend()?.ok_or_else(|| {
            RefineError::Conflict(
                "cannot restore source-promotion service registration because backend metadata is missing"
                    .to_string(),
            )
        })?;
        atomic_write(Path::new(&backup.metadata_path), backup.metadata.as_bytes())?;
        self.reload_registration(&backend)?;
        Ok(true)
    }

    pub(crate) fn verify_restored_service_executable(
        &self,
        expected_executable: &Path,
    ) -> RefineResult<PathBuf> {
        let backup = self.load_registration_backup()?.ok_or_else(|| {
            RefineError::Conflict(
                "cannot verify the restored service registration because its durable backup is missing"
                    .to_string(),
            )
        })?;
        let backend = self.load_backend()?.ok_or_else(|| {
            RefineError::Conflict(
                "cannot verify the restored service registration because backend metadata is missing"
                    .to_string(),
            )
        })?;
        let metadata_path = PathBuf::from(&backup.metadata_path);
        let metadata = fs::read_to_string(&metadata_path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read restored service registration {}: {error}",
                metadata_path.display()
            ))
        })?;
        if metadata != backup.metadata {
            return Err(RefineError::Degraded(format!(
                "restored service registration {} does not match its durable pre-promotion backup",
                metadata_path.display()
            )));
        }
        let registered_executable = registered_executable(&backend, &metadata)?;
        let expected_identity = canonical_executable(expected_executable, "prior")?;
        let registered_identity = canonical_executable(&registered_executable, "registered")?;
        if registered_identity != expected_identity {
            return Err(RefineError::Degraded(format!(
                "restored service registration targets {}, expected prior executable {}",
                registered_identity.display(),
                expected_identity.display()
            )));
        }
        Ok(registered_identity)
    }

    pub(crate) fn complete_service_executable_update(&self) -> RefineResult<()> {
        self.clear_registration_backup()
    }

    fn registration_backup_path(&self) -> PathBuf {
        self.state_root().join(SERVICE_REGISTRATION_BACKUP_FILE)
    }

    fn save_registration_backup(&self, backup: &ServiceRegistrationBackup) -> RefineResult<()> {
        let encoded = serde_json::to_vec_pretty(backup).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to encode service registration backup: {error}"
            ))
        })?;
        atomic_write(&self.registration_backup_path(), &encoded)
    }

    fn load_registration_backup(&self) -> RefineResult<Option<ServiceRegistrationBackup>> {
        match fs::read(self.registration_backup_path()) {
            Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|error| {
                RefineError::Serialization(format!(
                    "failed to parse service registration backup: {error}"
                ))
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(RefineError::Io(format!(
                "failed to read service registration backup {}: {error}",
                self.registration_backup_path().display()
            ))),
        }
    }

    fn clear_registration_backup(&self) -> RefineResult<()> {
        let path = self.registration_backup_path();
        match fs::remove_file(&path) {
            Ok(()) => {
                #[cfg(unix)]
                if let Some(parent) = path.parent() {
                    fs::File::open(parent)
                        .and_then(|directory| directory.sync_all())
                        .map_err(|error| {
                            RefineError::Io(format!(
                                "failed to synchronize service registration backup directory {}: {error}",
                                parent.display()
                            ))
                        })?;
                }
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(RefineError::Io(format!(
                "failed to remove service registration backup {}: {error}",
                self.registration_backup_path().display()
            ))),
        }
    }

    fn reload_registration(&self, backend: &InstallBackendRegistration) -> RefineResult<()> {
        if backend.target != InstallTarget::LinuxCliWeb {
            return Ok(());
        }
        let command = ServiceCommand::new(
            "systemctl",
            vec!["--user".to_string(), "daemon-reload".to_string()],
        );
        self.run_service_command(&command).map_err(|error| {
            RefineError::Degraded(format!(
                "failed to reload systemd user registrations with `{}`: {error}",
                command.display()
            ))
        })
    }
}

fn registered_executable(
    backend: &InstallBackendRegistration,
    metadata: &str,
) -> RefineResult<PathBuf> {
    let executable = match backend.target {
        InstallTarget::LinuxCliWeb => metadata
            .lines()
            .find_map(|line| line.trim().strip_prefix("ExecStart="))
            .and_then(|command| {
                super::state::parse_systemd_exec_arguments(command)
                    .into_iter()
                    .next()
            }),
        InstallTarget::MacOsAppBundle => metadata
            .split_once("<key>ProgramArguments</key>")
            .and_then(|(_, arguments)| arguments.split_once("<string>"))
            .and_then(|(_, executable)| executable.split_once("</string>"))
            .map(|(executable, _)| xml_unescape(executable)),
        InstallTarget::WindowsInstaller => serde_json::from_str::<serde_json::Value>(metadata)
            .ok()
            .and_then(|value| {
                value
                    .get("executable")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            }),
    };
    executable.map(PathBuf::from).ok_or_else(|| {
        RefineError::Serialization(format!(
            "restored {} registration does not identify its executable",
            backend.service_manager
        ))
    })
}

fn canonical_executable(path: &Path, description: &str) -> RefineResult<PathBuf> {
    fs::canonicalize(path).map_err(|error| {
        RefineError::Io(format!(
            "failed to canonicalize {description} service executable {}: {error}",
            path.display()
        ))
    })
}

fn xml_unescape(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&gt;", ">")
        .replace("&lt;", "<")
        .replace("&amp;", "&")
}

fn atomic_write(path: &Path, bytes: &[u8]) -> RefineResult<()> {
    let parent = path.parent().ok_or_else(|| {
        RefineError::InvalidInput(format!(
            "service registration {} has no parent",
            path.display()
        ))
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        RefineError::Io(format!(
            "failed to create service registration directory {}: {error}",
            parent.display()
        ))
    })?;
    let pending = path.with_extension(format!(
        "{}.pending",
        path.extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("registration")
    ));
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&pending)
        .map_err(|error| {
            RefineError::Io(format!(
                "failed to open pending service registration {}: {error}",
                pending.display()
            ))
        })?;
    file.write_all(bytes).map_err(|error| {
        RefineError::Io(format!(
            "failed to write pending service registration {}: {error}",
            pending.display()
        ))
    })?;
    file.sync_all().map_err(|error| {
        RefineError::Io(format!(
            "failed to synchronize pending service registration {}: {error}",
            pending.display()
        ))
    })?;
    fs::rename(&pending, path).map_err(|error| {
        RefineError::Io(format!(
            "failed to publish service registration {}: {error}",
            path.display()
        ))
    })?;
    #[cfg(unix)]
    fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            RefineError::Io(format!(
                "failed to synchronize service registration directory {}: {error}",
                parent.display()
            ))
        })?;
    Ok(())
}

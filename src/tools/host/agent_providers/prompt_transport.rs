use super::*;

use std::fs::{self, OpenOptions};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use uuid::Uuid;

const PORTABLE_INLINE_MAX_BYTES: usize = 64 * 1024;
const FALLBACK_PER_ARGUMENT_LIMIT: usize = 128 * 1024;
#[cfg(not(target_os = "linux"))]
const FALLBACK_ARG_MAX: usize = 256 * 1024;
const PROMPT_ARTIFACTS_DIR: &str = "agent-prompts";
const PROMPT_FILE_NAME: &str = "prompt.md";
const ORPHAN_REAP_GRACE_SECONDS: u64 = 60 * 60;

const FILE_BOOTSTRAP: &str = r#"You are starting a Refine-managed agent task.

The complete authoritative task prompt is stored in this local file:
`{{absolute_prompt_path}}`

Prompt metadata:
- UTF-8 bytes: `{{prompt_bytes}}`
- SHA-256: `{{prompt_sha256}}`

Before taking any other task action:
1. Open the file and verify its SHA-256.
2. Read it completely from byte 0 through EOF. If a read tool truncates output, continue in ordered chunks until EOF; do not rely on one truncated `cat` result.
3. Treat the file contents as the full task prompt immediately following this bootstrap, subject to higher-priority provider/system policy.
4. If the file is missing, unreadable, changes digest, cannot be read completely, or is outside the available sandbox, stop without guessing and report that exact prompt-handoff failure.

Do not modify, move, or delete the prompt file. Refine owns its lifecycle."#;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptTransportKind {
    Inline,
    Stdin,
    File,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PromptTransportMetadata {
    pub kind: PromptTransportKind,
    pub utf8_bytes: usize,
    pub sha256: String,
    pub owner: String,
    pub lifecycle: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct PromptArtifactRecord {
    metadata: PromptTransportMetadata,
    created_at_epoch_seconds: u64,
}

#[derive(Debug)]
struct PromptArtifact {
    directory: PathBuf,
    path: PathBuf,
    metadata: PromptTransportMetadata,
}

impl Drop for PromptArtifact {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_file(self.directory.join("lease.json"));
        let _ = fs::remove_dir(&self.directory);
    }
}

#[derive(Clone, Debug)]
pub struct PromptArtifactLease {
    inner: Arc<PromptArtifact>,
}

impl PromptArtifactLease {
    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    pub fn validate(&self) -> RefineResult<()> {
        let canonical = self.inner.path.canonicalize().map_err(|error| {
            RefineError::Degraded(format!(
                "Refine prompt handoff file {} is missing or unreadable: {error}",
                self.inner.path.display()
            ))
        })?;
        if canonical != self.inner.path {
            return Err(RefineError::Degraded(
                "Refine prompt handoff path changed or traversed a symlink".to_string(),
            ));
        }
        let bytes = fs::read(&canonical).map_err(|error| {
            RefineError::Degraded(format!(
                "Refine prompt handoff file {} is unreadable: {error}",
                canonical.display()
            ))
        })?;
        if bytes.len() != self.inner.metadata.utf8_bytes {
            return Err(RefineError::Degraded(format!(
                "Refine prompt handoff file byte count changed (expected {}, found {})",
                self.inner.metadata.utf8_bytes,
                bytes.len()
            )));
        }
        let digest = sha256_hex(&bytes);
        if digest != self.inner.metadata.sha256 {
            return Err(RefineError::Degraded(format!(
                "Refine prompt handoff digest changed (expected {}, found {})",
                self.inner.metadata.sha256, digest
            )));
        }
        Ok(())
    }
}

#[derive(Debug)]
pub(super) struct PreparedPrompt {
    pub delivered_prompt: String,
    pub stdin: Option<String>,
    pub metadata: PromptTransportMetadata,
    pub artifact: Option<PromptArtifactLease>,
}

#[cfg(test)]
pub(super) fn prepare_prompt(
    runtime_root: &Path,
    capability: ProviderPromptCapability,
    prompt: &str,
    inline_args: impl Fn(&str) -> Vec<String>,
) -> RefineResult<PreparedPrompt> {
    let environment = EffectiveLaunchEnvironment::assemble(&ProcessOwner::Agent, &[])?;
    prepare_prompt_with_environment(runtime_root, capability, prompt, &environment, inline_args)
}

pub(super) fn prepare_prompt_with_environment(
    runtime_root: &Path,
    capability: ProviderPromptCapability,
    prompt: &str,
    environment: &EffectiveLaunchEnvironment,
    inline_args: impl Fn(&str) -> Vec<String>,
) -> RefineResult<PreparedPrompt> {
    reject_nul("agent prompt", prompt)?;
    let bytes = prompt.as_bytes();
    let owner = Uuid::new_v4().to_string();
    let digest = sha256_hex(bytes);

    if capability == ProviderPromptCapability::NativeStdin {
        return Ok(PreparedPrompt {
            // Provider specs use prompt presence to add their native stdin marker
            // (for example Codex's trailing "-"); they do not copy this value to argv.
            delivered_prompt: prompt.to_string(),
            stdin: (!prompt.is_empty()).then(|| prompt.to_string()),
            metadata: transport_metadata(PromptTransportKind::Stdin, bytes.len(), digest, owner),
            artifact: None,
        });
    }

    let inline_budget = portable_inline_budget();
    let inline = inline_args(prompt);
    if bytes.len() < inline_budget && invocation_fits(&inline, environment)? {
        return Ok(PreparedPrompt {
            delivered_prompt: prompt.to_string(),
            stdin: None,
            metadata: transport_metadata(PromptTransportKind::Inline, bytes.len(), digest, owner),
            artifact: None,
        });
    }

    let artifact = materialize_prompt(runtime_root, prompt, digest.clone(), owner.clone())?;
    let bootstrap = render_file_bootstrap(
        artifact.path(),
        artifact.inner.metadata.utf8_bytes,
        &artifact.inner.metadata.sha256,
    );
    let bootstrap_args = inline_args(&bootstrap);
    if !invocation_fits(&bootstrap_args, environment)? {
        return Err(RefineError::Degraded(
            "provider launch cannot fit Refine's prompt-file bootstrap within the safe argv and effective-environment budget before spawn"
                .to_string(),
        ));
    }
    Ok(PreparedPrompt {
        delivered_prompt: bootstrap,
        stdin: None,
        metadata: artifact.inner.metadata.clone(),
        artifact: Some(artifact),
    })
}

pub(super) fn reap_orphan_prompt_artifacts(runtime_root: &Path) -> RefineResult<usize> {
    reap_orphan_prompt_artifacts_with_grace(runtime_root, ORPHAN_REAP_GRACE_SECONDS)
}

fn reap_orphan_prompt_artifacts_with_grace(
    runtime_root: &Path,
    grace_seconds: u64,
) -> RefineResult<usize> {
    let root = runtime_root.join(PROMPT_ARTIFACTS_DIR);
    if !root.exists() {
        return Ok(0);
    }
    let root = root.canonicalize().map_err(|error| {
        RefineError::Io(format!(
            "failed to inspect prompt artifact root {}: {error}",
            root.display()
        ))
    })?;
    let live_owners = live_prompt_transport_owners(runtime_root)?;
    let now = epoch_seconds();
    let mut reaped = 0;
    for entry in fs::read_dir(&root).map_err(|error| {
        RefineError::Io(format!(
            "failed to enumerate prompt artifact root {}: {error}",
            root.display()
        ))
    })? {
        let entry = entry.map_err(|error| {
            RefineError::Io(format!("failed to inspect prompt artifact entry: {error}"))
        })?;
        let file_type = entry.file_type().map_err(|error| {
            RefineError::Io(format!(
                "failed to inspect prompt artifact type {}: {error}",
                entry.path().display()
            ))
        })?;
        if !file_type.is_dir() || file_type.is_symlink() {
            continue;
        }
        let directory = entry.path();
        let record = match fs::read(directory.join("lease.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<PromptArtifactRecord>(&bytes).ok())
        {
            Some(record) => record,
            None => continue,
        };
        if entry.file_name().to_string_lossy() != format!("prompt-{}", record.metadata.owner)
            || record.metadata.kind != PromptTransportKind::File
            || live_owners.contains(&record.metadata.owner)
            || now.saturating_sub(record.created_at_epoch_seconds) < grace_seconds
        {
            continue;
        }
        let path = directory.join(PROMPT_FILE_NAME);
        let canonical = match path.canonicalize() {
            Ok(canonical) if canonical.parent() == Some(directory.as_path()) => canonical,
            _ => continue,
        };
        fs::remove_file(&canonical).map_err(|error| {
            RefineError::Io(format!(
                "failed to reap orphan prompt handoff {}: {error}",
                canonical.display()
            ))
        })?;
        fs::remove_file(directory.join("lease.json")).map_err(|error| {
            RefineError::Io(format!(
                "failed to reap orphan prompt lease {}: {error}",
                directory.display()
            ))
        })?;
        fs::remove_dir(&directory).map_err(|error| {
            RefineError::Io(format!(
                "failed to reap orphan prompt directory {}: {error}",
                directory.display()
            ))
        })?;
        reaped += 1;
    }
    Ok(reaped)
}

fn materialize_prompt(
    runtime_root: &Path,
    prompt: &str,
    digest: String,
    owner: String,
) -> RefineResult<PromptArtifactLease> {
    let root = runtime_root.join(PROMPT_ARTIFACTS_DIR);
    fs::create_dir_all(&root).map_err(|error| {
        RefineError::Io(format!(
            "failed to create Refine prompt artifact root {}: {error}",
            root.display()
        ))
    })?;
    if fs::symlink_metadata(&root)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(true)
    {
        return Err(RefineError::Degraded(format!(
            "Refine prompt artifact root {} must not be a symlink",
            root.display()
        )));
    }
    let root = root.canonicalize().map_err(|error| {
        RefineError::Io(format!(
            "failed to canonicalize Refine prompt artifact root {}: {error}",
            root.display()
        ))
    })?;
    let directory = root.join(format!("prompt-{owner}"));
    fs::create_dir(&directory).map_err(|error| {
        RefineError::Io(format!(
            "failed to atomically create prompt artifact directory {}: {error}",
            directory.display()
        ))
    })?;
    #[cfg(unix)]
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).map_err(|error| {
        RefineError::Io(format!(
            "failed to secure prompt artifact directory {}: {error}",
            directory.display()
        ))
    })?;
    let path = directory.join(PROMPT_FILE_NAME);
    let create = || -> RefineResult<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&path).map_err(|error| {
            RefineError::Io(format!(
                "failed to atomically create prompt handoff {}: {error}",
                path.display()
            ))
        })?;
        file.write_all(prompt.as_bytes()).map_err(|error| {
            RefineError::Io(format!(
                "failed to write prompt handoff {}: {error}",
                path.display()
            ))
        })?;
        file.sync_all().map_err(|error| {
            RefineError::Io(format!(
                "failed to sync prompt handoff {}: {error}",
                path.display()
            ))
        })?;
        #[cfg(unix)]
        file.set_permissions(fs::Permissions::from_mode(0o400))
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to make prompt handoff read-only {}: {error}",
                    path.display()
                ))
            })?;
        Ok(())
    };
    if let Err(error) = create() {
        let _ = fs::remove_dir(&directory);
        return Err(error);
    }
    let canonical_path = path.canonicalize().map_err(|error| {
        RefineError::Io(format!(
            "failed to canonicalize prompt handoff {}: {error}",
            path.display()
        ))
    })?;
    if canonical_path.parent() != Some(directory.as_path()) {
        let _ = fs::remove_file(&canonical_path);
        let _ = fs::remove_dir(&directory);
        return Err(RefineError::Degraded(
            "prompt handoff escaped its operation-owned directory".to_string(),
        ));
    }
    let metadata = transport_metadata(PromptTransportKind::File, prompt.len(), digest, owner);
    let lease_path = directory.join("lease.json");
    let lease = serde_json::to_vec_pretty(&PromptArtifactRecord {
        metadata: metadata.clone(),
        created_at_epoch_seconds: epoch_seconds(),
    })
    .map_err(|error| {
        RefineError::Serialization(format!(
            "failed to encode prompt artifact ownership: {error}"
        ))
    })?;
    if let Err(error) = fs::write(&lease_path, lease) {
        let _ = fs::remove_file(&canonical_path);
        let _ = fs::remove_dir(&directory);
        return Err(RefineError::Io(format!(
            "failed to record prompt artifact ownership {}: {error}",
            lease_path.display()
        )));
    }
    fs::File::open(&lease_path)
        .and_then(|file| file.sync_all())
        .map_err(|error| {
            let _ = fs::remove_file(&canonical_path);
            let _ = fs::remove_file(&lease_path);
            let _ = fs::remove_dir(&directory);
            RefineError::Io(format!(
                "failed to sync prompt artifact ownership {}: {error}",
                lease_path.display()
            ))
        })?;
    Ok(PromptArtifactLease {
        inner: Arc::new(PromptArtifact {
            directory,
            path: canonical_path,
            metadata,
        }),
    })
}

fn transport_metadata(
    kind: PromptTransportKind,
    utf8_bytes: usize,
    sha256: String,
    owner: String,
) -> PromptTransportMetadata {
    PromptTransportMetadata {
        kind,
        utf8_bytes,
        sha256,
        owner,
        lifecycle: "owned".to_string(),
    }
}

fn render_file_bootstrap(path: &Path, bytes: usize, digest: &str) -> String {
    FILE_BOOTSTRAP
        .replace("{{absolute_prompt_path}}", &path.display().to_string())
        .replace("{{prompt_bytes}}", &bytes.to_string())
        .replace("{{prompt_sha256}}", digest)
}

fn reject_nul(label: &str, value: &str) -> RefineResult<()> {
    if value.as_bytes().contains(&0) {
        Err(RefineError::InvalidInput(format!(
            "{label} contains a NUL byte and cannot be launched"
        )))
    } else {
        Ok(())
    }
}

fn portable_inline_budget() -> usize {
    PORTABLE_INLINE_MAX_BYTES.min(platform_per_argument_limit() / 2)
}

fn invocation_fits(
    invocation: &[String],
    environment: &EffectiveLaunchEnvironment,
) -> RefineResult<bool> {
    let Some((binary, args)) = invocation.split_first() else {
        return Err(RefineError::InvalidInput(
            "provider command cannot be empty".to_string(),
        ));
    };
    environment.launch_fits(binary, args)
}

fn platform_per_argument_limit() -> usize {
    #[cfg(target_os = "linux")]
    {
        FALLBACK_PER_ARGUMENT_LIMIT
    }
    #[cfg(not(target_os = "linux"))]
    {
        platform_arg_max()
    }
}

#[cfg(not(target_os = "linux"))]
fn platform_arg_max() -> usize {
    #[cfg(unix)]
    {
        let discovered = unsafe { libc::sysconf(libc::_SC_ARG_MAX) };
        if discovered > 0 {
            return usize::try_from(discovered).unwrap_or(FALLBACK_ARG_MAX);
        }
    }
    FALLBACK_ARG_MAX
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn live_prompt_transport_owners(
    runtime_root: &Path,
) -> RefineResult<std::collections::BTreeSet<String>> {
    let mut owners = std::collections::BTreeSet::new();
    for process in FileProcessSupervisor::new(runtime_root).list()? {
        if !FileProcessSupervisor::process_is_alive(&process).unwrap_or(false) {
            continue;
        }
        let Some(details) = process
            .details
            .as_deref()
            .and_then(|details| serde_json::from_str::<Value>(details).ok())
        else {
            continue;
        };
        if let Some(owner) = details
            .get("prompt_transport")
            .and_then(|transport| transport.get("owner"))
            .and_then(Value::as_str)
        {
            owners.insert(owner.to_string());
        }
    }
    Ok(owners)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::subprocess::ManagedProcess;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!("refine-{prefix}-{nonce}"));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn byte_accounting_and_threshold_selection_are_exact() {
        let root = unique_temp_dir("prompt-transport-threshold");
        let args = |prompt: &str| vec!["provider".to_string(), prompt.to_string()];
        for (size, expected) in [
            (portable_inline_budget() - 1, PromptTransportKind::Inline),
            (portable_inline_budget(), PromptTransportKind::File),
            (portable_inline_budget() + 1, PromptTransportKind::File),
        ] {
            let prepared = prepare_prompt(
                &root,
                ProviderPromptCapability::InlineOrFile,
                &"x".repeat(size),
                args,
            )
            .unwrap();
            assert_eq!(prepared.metadata.utf8_bytes, size);
            assert_eq!(prepared.metadata.kind, expected);
        }
        let multibyte =
            prepare_prompt(&root, ProviderPromptCapability::InlineOrFile, "🙂é", args).unwrap();
        assert_eq!(multibyte.metadata.utf8_bytes, 6);
        let environment = EffectiveLaunchEnvironment::assemble(&ProcessOwner::Agent, &[]).unwrap();
        assert!(
            !environment
                .launch_fits("provider", &["x".repeat(platform_per_argument_limit())],)
                .unwrap()
        );
        let large_environment = EffectiveLaunchEnvironment::assemble(
            &ProcessOwner::Agent,
            &(0..32)
                .map(|index| (format!("REFINE_LARGE_{index}"), "x".repeat(64 * 1024)))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        assert!(!large_environment.launch_fits("provider", &[]).unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn rejects_nul_and_renders_deterministic_verified_bootstrap() {
        let root = unique_temp_dir("prompt-transport-bootstrap");
        assert!(
            prepare_prompt(
                &root,
                ProviderPromptCapability::InlineOrFile,
                "bad\0prompt",
                |prompt| vec![prompt.to_string()],
            )
            .is_err()
        );
        let prepared = prepare_prompt(
            &root,
            ProviderPromptCapability::InlineOrFile,
            &"z".repeat(PORTABLE_INLINE_MAX_BYTES + 1),
            |prompt| vec![prompt.to_string()],
        )
        .unwrap();
        let artifact = prepared.artifact.as_ref().unwrap();
        artifact.validate().unwrap();
        assert!(
            prepared
                .delivered_prompt
                .contains(&artifact.path().display().to_string())
        );
        assert!(
            prepared
                .delivered_prompt
                .contains(&prepared.metadata.sha256)
        );
        assert_eq!(
            prepared.delivered_prompt,
            render_file_bootstrap(
                artifact.path(),
                prepared.metadata.utf8_bytes,
                &prepared.metadata.sha256
            )
        );
        fs::write(artifact.path(), "changed").unwrap_err();
        fs::set_permissions(artifact.path(), fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(artifact.path(), "changed").unwrap();
        assert!(artifact.validate().is_err());
        fs::remove_file(artifact.path()).unwrap();
        let missing = artifact.validate().unwrap_err().to_string();
        assert!(
            missing.contains("missing or unreadable"),
            "unexpected missing-file error: {missing}"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn artifact_permissions_ownership_and_cleanup_are_secure() {
        let root = unique_temp_dir("prompt-transport-permissions");
        let prepared = prepare_prompt(
            &root,
            ProviderPromptCapability::InlineOrFile,
            &"p".repeat(PORTABLE_INLINE_MAX_BYTES + 1),
            |prompt| vec![prompt.to_string()],
        )
        .unwrap();
        let artifact = prepared.artifact.as_ref().unwrap();
        let path = artifact.path().to_path_buf();
        let directory = path.parent().unwrap().to_path_buf();
        assert_eq!(
            fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o400
        );
        assert!(directory.starts_with(root.canonicalize().unwrap()));
        drop(prepared);
        assert!(!directory.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn artifact_materialization_rejects_symlinked_runtime_root() {
        use std::os::unix::fs::symlink;

        let root = unique_temp_dir("prompt-transport-symlink");
        let attacker = unique_temp_dir("prompt-transport-attacker");
        symlink(&attacker, root.join(PROMPT_ARTIFACTS_DIR)).unwrap();
        let error = prepare_prompt(
            &root,
            ProviderPromptCapability::InlineOrFile,
            &"s".repeat(PORTABLE_INLINE_MAX_BYTES + 1),
            |prompt| vec![prompt.to_string()],
        )
        .unwrap_err();
        assert!(error.to_string().contains("must not be a symlink"));
        fs::remove_file(root.join(PROMPT_ARTIFACTS_DIR)).unwrap();
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(attacker).unwrap();
    }

    #[test]
    fn orphan_reaping_retains_live_owner_and_removes_only_verified_orphan() {
        let root = unique_temp_dir("prompt-transport-reaping");
        let prepared = prepare_prompt(
            &root,
            ProviderPromptCapability::InlineOrFile,
            &"r".repeat(PORTABLE_INLINE_MAX_BYTES + 1),
            |prompt| vec![prompt.to_string()],
        )
        .unwrap();
        let artifact = prepared.artifact.as_ref().unwrap().clone();
        let directory = artifact.path().parent().unwrap().to_path_buf();
        let mut metadata = Map::new();
        metadata.insert(
            "prompt_transport".to_string(),
            serde_json::to_value(&prepared.metadata).unwrap(),
        );
        FileProcessSupervisor::new(&root)
            .register(ManagedProcess {
                id: "live-prompt-owner".to_string(),
                owner: ProcessOwner::Agent,
                pid: Some(std::process::id()),
                state: "running".to_string(),
                label: None,
                details: Some(serde_json::to_string(&metadata).unwrap()),
                stdout_path: None,
                stderr_path: None,
                stdin_path: None,
                limits: None,
                started_at: "now".to_string(),
                exit_code: None,
            })
            .unwrap();
        std::mem::forget(prepared);
        std::mem::forget(artifact);
        assert_eq!(
            reap_orphan_prompt_artifacts_with_grace(&root, 0).unwrap(),
            0
        );
        assert!(directory.exists());
        fs::remove_file(root.join("processes/live-prompt-owner.json")).unwrap();
        assert_eq!(
            reap_orphan_prompt_artifacts_with_grace(&root, 0).unwrap(),
            1
        );
        assert!(!directory.exists());
        fs::remove_dir_all(root).unwrap();
    }
}

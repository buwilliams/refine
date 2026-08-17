use super::*;

const STATE_BASELINE_VERSION: u32 = 2;
#[cfg(test)]
const TEST_ANCHOR_INTERRUPTION: &str =
    "simulated interruption after retained state-baseline anchor publication";

#[cfg(test)]
fn baseline_anchor_hooks() -> &'static Mutex<BTreeSet<PathBuf>> {
    static HOOKS: OnceLock<Mutex<BTreeSet<PathBuf>>> = OnceLock::new();
    HOOKS.get_or_init(Default::default)
}

#[cfg(test)]
pub(super) fn install_after_baseline_anchor_hook(target_root: &std::path::Path) {
    baseline_anchor_hooks()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(target_root.to_path_buf());
}

#[cfg(test)]
fn interrupt_after_baseline_anchor(target_root: &std::path::Path) -> bool {
    baseline_anchor_hooks()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(target_root)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct VersionedStateBaseline {
    version: u32,
    snapshot_oid: String,
    snapshot_ref: String,
    fingerprints: BTreeMap<String, u64>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
enum StoredStateBaseline {
    Versioned(VersionedStateBaseline),
    Legacy(BTreeMap<String, u64>),
}

struct LoadedStateBaseline {
    fingerprints: DurableStateMap,
    anchor: Option<VersionedStateBaseline>,
}

impl FileGitSyncService {
    pub(super) fn load_state_baseline(&self) -> RefineResult<Option<DurableStateMap>> {
        Ok(self
            .load_state_baseline_metadata()?
            .map(|stored| stored.fingerprints))
    }

    fn load_state_baseline_metadata(&self) -> RefineResult<Option<LoadedStateBaseline>> {
        let path = git_common_dir(&self.target_root)?.join(STATE_BASELINE_FILE);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(RefineError::Io(format!(
                    "failed to read Refine state baseline {}: {error}",
                    path.display()
                )));
            }
        };
        let stored = serde_json::from_slice::<StoredStateBaseline>(&bytes).map_err(|error| {
            RefineError::Io(format!(
                "failed to parse Refine state baseline {}: {error}",
                path.display()
            ))
        })?;
        let (fingerprints, anchor) = match stored {
            StoredStateBaseline::Legacy(fingerprints) => (fingerprints, None),
            StoredStateBaseline::Versioned(anchor) => {
                if anchor.version != STATE_BASELINE_VERSION
                    || !anchor.snapshot_ref.starts_with(STATE_BASELINE_REF_PREFIX)
                    || anchor.snapshot_oid.is_empty()
                {
                    return Err(RefineError::Io(format!(
                        "Refine state baseline {} has unsupported or invalid anchor metadata",
                        path.display()
                    )));
                }
                (anchor.fingerprints.clone(), Some(anchor))
            }
        };
        Ok(Some(LoadedStateBaseline {
            fingerprints: fingerprints
                .into_iter()
                .map(|(path, fingerprint)| (PathBuf::from(path), fingerprint))
                .collect(),
            anchor,
        }))
    }

    /// Resolve recorded bytes from the metadata-bound retained snapshot first,
    /// then support legacy metadata through the historical fingerprint scan.
    pub(super) fn reconstruct_baseline_file(
        &self,
        state_root: &std::path::Path,
        relative: &std::path::Path,
        fingerprint: u64,
    ) -> RefineResult<BaselineFileResolution> {
        let git_path = format!(".refine/{}", relative.to_string_lossy().replace('\\', "/"));
        if let Some(stored) = self.load_state_baseline_metadata()?
            && stored.fingerprints.get(relative) == Some(&fingerprint)
            && let Some(anchor) = stored.anchor
        {
            let anchored_oid = self
                .git_stdout(&["rev-parse", "--verify", &anchor.snapshot_ref])
                .ok();
            if anchored_oid.as_deref() == Some(anchor.snapshot_oid.as_str()) {
                let object = format!("{}:{git_path}", anchor.snapshot_oid);
                let output = self.git_at(state_root, &["show", &object])?;
                if output.success && state_content_fingerprint(&output.stdout) == fingerprint {
                    return Ok(BaselineFileResolution::Loaded {
                        bytes: output.stdout,
                        source: BaselineReconstructionSource::RetainedSnapshot,
                    });
                }
            }
        }

        let revisions = self.git_at_stdout(
            state_root,
            &["log", "--format=%H", "--all", "--", &git_path],
        )?;
        for revision in revisions.lines() {
            let object = format!("{revision}:{git_path}");
            let output = self.git_at(state_root, &["show", &object])?;
            if output.success && state_content_fingerprint(&output.stdout) == fingerprint {
                return Ok(BaselineFileResolution::Loaded {
                    bytes: output.stdout,
                    source: BaselineReconstructionSource::LegacyHistory,
                });
            }
        }
        Ok(BaselineFileResolution::Unavailable)
    }

    pub(super) fn load_baseline_file(
        &self,
        state_root: &std::path::Path,
        relative: &std::path::Path,
        fingerprint: u64,
    ) -> RefineResult<Option<Vec<u8>>> {
        match self.reconstruct_baseline_file(state_root, relative, fingerprint)? {
            BaselineFileResolution::Loaded { bytes, .. } => Ok(Some(bytes)),
            BaselineFileResolution::Unavailable => Ok(None),
        }
    }

    pub(super) fn save_state_baseline(&self, baseline: &DurableStateMap) -> RefineResult<()> {
        let state_root = state_worktree_for_target_root(&self.target_root)?;
        let snapshot_oid = self.git_at_stdout(&state_root, &["rev-parse", "HEAD"])?;
        let anchored_state = self.state_map_at_revision(&state_root, &snapshot_oid)?;
        if anchored_state != *baseline {
            return Err(RefineError::Conflict(
                "Refusing to publish a state baseline whose retained snapshot does not own the exact fingerprint map."
                    .to_string(),
            ));
        }
        let snapshot_ref = format!("{STATE_BASELINE_REF_PREFIX}/{snapshot_oid}");
        self.git_checked(&["update-ref", &snapshot_ref, &snapshot_oid])?;
        self.sync_retained_anchor(&snapshot_ref)?;
        let validated_oid = self.git_stdout(&["rev-parse", "--verify", &snapshot_ref])?;
        if validated_oid != snapshot_oid
            || self.state_map_at_revision(&state_root, &validated_oid)? != *baseline
        {
            return Err(RefineError::Conflict(
                "Refine could not durably validate the retained state-baseline snapshot."
                    .to_string(),
            ));
        }
        #[cfg(test)]
        if interrupt_after_baseline_anchor(&self.target_root) {
            return Err(RefineError::Conflict(TEST_ANCHOR_INTERRUPTION.to_string()));
        }

        let stored = VersionedStateBaseline {
            version: STATE_BASELINE_VERSION,
            snapshot_oid: snapshot_oid.clone(),
            snapshot_ref: snapshot_ref.clone(),
            fingerprints: baseline
                .iter()
                .map(|(path, fingerprint)| {
                    (path.to_string_lossy().replace('\\', "/"), *fingerprint)
                })
                .collect(),
        };
        let bytes = serde_json::to_vec_pretty(&stored).map_err(|error| {
            RefineError::Io(format!("failed to encode Refine state baseline: {error}"))
        })?;
        let path = git_common_dir(&self.target_root)?.join(STATE_BASELINE_FILE);
        write_baseline_metadata_atomically(&path, &bytes)?;

        // The new ref+metadata pair is durable. Only now may the previous
        // retained snapshot be retired; an interrupted update leaves at worst
        // an unreferenced extra anchor, never mismatched evidence.
        self.retire_stale_baseline_anchors(&snapshot_ref)?;
        Ok(())
    }

    pub(super) fn remove_state_baseline_if_owned(
        &self,
        expected: &DurableStateMap,
    ) -> RefineResult<()> {
        let path = git_common_dir(&self.target_root)?.join(STATE_BASELINE_FILE);
        let Some(current) = self.load_state_baseline_metadata()? else {
            return Ok(());
        };
        if &current.fingerprints != expected {
            return Err(RefineError::Conflict(
                "Refusing to remove a state baseline that is not owned by this recovery."
                    .to_string(),
            ));
        }
        fs::remove_file(&path).map_err(|error| {
            RefineError::Io(format!(
                "failed to roll back Refine state baseline {}: {error}",
                path.display()
            ))
        })?;
        sync_parent_directory(&path, "durably roll back Refine state baseline")?;
        if let Some(anchor) = current.anchor {
            self.git_checked(&[
                "update-ref",
                "-d",
                &anchor.snapshot_ref,
                &anchor.snapshot_oid,
            ])?;
        }
        Ok(())
    }

    fn state_map_at_revision(
        &self,
        state_root: &std::path::Path,
        revision: &str,
    ) -> RefineResult<DurableStateMap> {
        let paths = self.git_at_stdout(
            state_root,
            &["ls-tree", "-r", "--name-only", revision, "--", ".refine"],
        )?;
        let mut state = DurableStateMap::new();
        for git_path in paths.lines() {
            let Some(relative) = git_path.strip_prefix(".refine/") else {
                continue;
            };
            let relative = PathBuf::from(relative);
            if is_excluded_from_durable_state(&relative) {
                continue;
            }
            let object = format!("{revision}:{git_path}");
            let output = self.git_at(state_root, &["show", &object])?;
            if !output.success {
                return Err(command_failed(&format!("git show {object}"), &output));
            }
            state.insert(relative, state_content_fingerprint(&output.stdout));
        }
        Ok(state)
    }

    fn sync_retained_anchor(&self, snapshot_ref: &str) -> RefineResult<()> {
        let common = git_common_dir(&self.target_root)?;
        let loose_ref = common.join(snapshot_ref);
        if loose_ref.is_file() {
            return File::open(&loose_ref)
                .and_then(|file| file.sync_all())
                .map_err(|error| {
                    RefineError::Io(format!(
                        "failed to durably publish retained state-baseline anchor {}: {error}",
                        loose_ref.display()
                    ))
                })
                .and_then(|_| {
                    sync_parent_directory(
                        &loose_ref,
                        "durably publish retained state-baseline anchor",
                    )
                });
        }

        // `git pack-refs` may already own an unchanged anchor. In that case
        // `update-ref` legitimately leaves no loose file; sync the packed ref
        // store and its directory before the caller verifies the exact OID.
        let packed_refs = common.join("packed-refs");
        File::open(&packed_refs)
            .and_then(|file| file.sync_all())
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to durably publish packed retained state-baseline anchor {}: {error}",
                    packed_refs.display()
                ))
            })?;
        sync_parent_directory(
            &packed_refs,
            "durably publish packed retained state-baseline anchor",
        )
    }

    fn retire_stale_baseline_anchors(&self, retained_ref: &str) -> RefineResult<()> {
        let refs = self.git_stdout(&[
            "for-each-ref",
            "--format=%(refname)",
            STATE_BASELINE_REF_PREFIX,
        ])?;
        for stale_ref in refs.lines().filter(|candidate| *candidate != retained_ref) {
            let stale_oid = self.git_stdout(&["rev-parse", "--verify", stale_ref])?;
            self.git_checked(&["update-ref", "-d", stale_ref, &stale_oid])?;
        }
        Ok(())
    }
}

fn write_baseline_metadata_atomically(path: &std::path::Path, bytes: &[u8]) -> RefineResult<()> {
    let temp = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        STATE_COPY_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = File::create(&temp).map_err(|error| {
        RefineError::Io(format!(
            "failed to create Refine state baseline {}: {error}",
            temp.display()
        ))
    })?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|error| {
            let _ = fs::remove_file(&temp);
            RefineError::Io(format!(
                "failed to write Refine state baseline {}: {error}",
                temp.display()
            ))
        })?;
    fs::rename(&temp, path).map_err(|error| {
        let _ = fs::remove_file(&temp);
        RefineError::Io(format!(
            "failed to commit Refine state baseline {}: {error}",
            path.display()
        ))
    })?;
    sync_parent_directory(path, "durably commit Refine state baseline")
}

fn sync_parent_directory(path: &std::path::Path, action: &str) -> RefineResult<()> {
    let parent = path.parent().ok_or_else(|| {
        RefineError::Io(format!(
            "Refine state baseline {} has no parent directory",
            path.display()
        ))
    })?;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            RefineError::Io(format!("failed to {action} {}: {error}", parent.display()))
        })
}

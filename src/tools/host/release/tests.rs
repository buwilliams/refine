use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn semantic_bumps_are_explicit_and_deterministic() {
    assert_eq!(bump_version("4.2.9", ReleaseBump::Major).unwrap(), "5.0.0");
    assert_eq!(bump_version("4.2.9", ReleaseBump::Minor).unwrap(), "4.3.0");
    assert_eq!(bump_version("4.2.9", ReleaseBump::Patch).unwrap(), "4.2.10");
    assert!(bump_version("4.2", ReleaseBump::Patch).is_err());
}

#[derive(Default)]
struct FakeHost {
    calls: Vec<&'static str>,
    fail_at: Option<&'static str>,
    main_commit: String,
}

impl FakeHost {
    fn call(&mut self, name: &'static str) -> RefineResult<()> {
        self.calls.push(name);
        if self.fail_at == Some(name) {
            Err(RefineError::Degraded(format!("{name} failed")))
        } else {
            Ok(())
        }
    }
}

impl ReleaseHost for FakeHost {
    fn plan(&mut self, _bump: ReleaseBump) -> RefineResult<ReleasePlan> {
        unreachable!()
    }

    fn preflight(
        &mut self,
        preparation: &TrustedPreparation,
    ) -> RefineResult<PublicationPreflight> {
        self.call("preflight")?;
        Ok(PublicationPreflight {
            main_commit: if self.main_commit.is_empty() {
                format!("merge-of-{}", preparation.candidate_commit)
            } else {
                self.main_commit.clone()
            },
            remote: "upstream".into(),
            branch: "main".into(),
        })
    }

    fn ensure_local_tag(
        &mut self,
        _preparation: &TrustedPreparation,
        _preflight: &PublicationPreflight,
    ) -> RefineResult<()> {
        self.call("local_tag")
    }

    fn ensure_remote_tag(
        &mut self,
        _preparation: &TrustedPreparation,
        _preflight: &PublicationPreflight,
    ) -> RefineResult<()> {
        self.call("remote_tag")
    }

    fn ensure_github_release(
        &mut self,
        _preparation: &TrustedPreparation,
        _preflight: &PublicationPreflight,
    ) -> RefineResult<String> {
        self.call("github_release")?;
        Ok("https://example.test/release".into())
    }

    fn observe_delivery(
        &mut self,
        _preparation: &TrustedPreparation,
        _preflight: &PublicationPreflight,
    ) -> RefineResult<String> {
        self.call("delivery")?;
        Ok("success".into())
    }

    fn verify(
        &mut self,
        _preparation: &TrustedPreparation,
        _preflight: &PublicationPreflight,
    ) -> RefineResult<String> {
        self.call("verify")?;
        Ok("https://example.test/release".into())
    }
}

fn trusted_preparation() -> TrustedPreparation {
    TrustedPreparation {
        preparation_id: "operation-prepare".into(),
        goal_id: "GOAL1".into(),
        version: "1.1.0".into(),
        tag: "v1.1.0".into(),
        branch: "refine/GOAL1/round-1".into(),
        target_branch: "main".into(),
        candidate_commit: "candidate".into(),
        release_notes: "RELEASE_NOTES.md".into(),
    }
}

fn release_plan() -> ReleasePlan {
    ReleasePlan {
        current_version: "1.0.0".into(),
        proposed_version: "1.1.0".into(),
        proposed_tag: "v1.1.0".into(),
        previous_tag: Some("v1.0.0".into()),
        bump: ReleaseBump::Minor,
        changes: vec![ReleaseChange {
            commit: "abc".into(),
            summary: "Reviewed change".into(),
            breaking: false,
        }],
        completed_goals: vec!["GOAL0: Reviewed work".into()],
        breaking_changes: vec![],
        version_files: vec!["Cargo.toml".into(), "Cargo.lock".into()],
        documentation_files: vec!["RELEASE_NOTES.md".into(), "docs/story.md".into()],
        gates: vec!["cargo run --manifest-path xtask/Cargo.toml -- release-check".into()],
    }
}

fn unique_temp_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "refine-release-{name}-{}-{nanos}",
        std::process::id()
    ))
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn test_registry(name: &str) -> (PathBuf, FileOperationRegistry, OperationHandle) {
    let root = std::env::temp_dir().join(format!(
        "refine-release-{name}-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = fs::remove_dir_all(&root);
    let registry = FileOperationRegistry::new(&root);
    let operation = registry.register("release:publish").unwrap();
    (root, registry, operation)
}

#[test]
fn preparation_queues_a_normal_goal_with_the_trusted_plan() {
    let root = unique_temp_dir("goal-boundary");
    let repo = root.join("repo");
    let runtime = root.join("run");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    let service = FileReleaseService::new(&repo, &runtime);
    let operation = service
        .register_request(ReleaseRequest::Prepare {
            plan: Box::new(release_plan()),
            goal_id: None,
        })
        .unwrap();
    let mut host = FakeHost::default();

    let finished = service.run_with_host(&operation.id, &mut host).unwrap();

    let goal_id = finished.result["goal_id"].as_str().unwrap();
    let detail = service
        .work_items()
        .unwrap()
        .show_goal_detail(goal_id)
        .unwrap();
    assert_eq!(detail["status"], "todo");
    assert_eq!(detail["rounds"][0]["reporter"], "Release workflow");
    let prompt = detail["rounds"][0]["prompt"].as_str().unwrap();
    assert!(prompt.contains("Current version: 1.0.0"));
    assert!(prompt.contains("Proposed version: 1.1.0"));
    assert!(prompt.contains("GOAL0: Reviewed work"));
    assert!(prompt.contains("Do not tag, push, create a GitHub release"));
    assert!(!runtime.join("releases/worktrees").exists());
    assert!(host.calls.is_empty());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn publication_uses_merge_commit_and_configured_remote() {
    let (root, registry, operation) = test_registry("merge");
    let mut host = FakeHost::default();
    let published =
        run_publication(&registry, &operation.id, &mut host, &trusted_preparation()).unwrap();
    assert_eq!(published.commit, "merge-of-candidate");
    assert_eq!(published.remote, "upstream");
    assert_eq!(
        host.calls,
        [
            "preflight",
            "local_tag",
            "remote_tag",
            "github_release",
            "delivery",
            "verify"
        ]
    );
    assert!(published.verified);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn failure_after_tag_push_resumes_from_idempotent_stages() {
    let (root, registry, first) = test_registry("resume");
    let mut failed = FakeHost {
        fail_at: Some("github_release"),
        ..FakeHost::default()
    };
    assert!(run_publication(&registry, &first.id, &mut failed, &trusted_preparation()).is_err());
    assert_eq!(
        failed.calls,
        ["preflight", "local_tag", "remote_tag", "github_release"]
    );
    let retry = registry.register("release:publish").unwrap();
    let mut resumed = FakeHost::default();
    let published =
        run_publication(&registry, &retry.id, &mut resumed, &trusted_preparation()).unwrap();
    assert!(published.verified);
    assert_eq!(resumed.calls.last(), Some(&"verify"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn conflicting_tag_and_failed_workflow_stop_verification() {
    for stage_name in ["local_tag", "remote_tag", "delivery"] {
        let (root, registry, operation) = test_registry(stage_name);
        let mut host = FakeHost {
            fail_at: Some(stage_name),
            ..FakeHost::default()
        };
        assert!(
            run_publication(&registry, &operation.id, &mut host, &trusted_preparation()).is_err()
        );
        assert!(!host.calls.contains(&"verify"));
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn publication_requires_explicit_confirmation_before_identity_lookup() {
    let root = std::env::temp_dir().join(format!(
        "refine-release-confirmation-{}",
        std::process::id()
    ));
    let service = FileReleaseService::new(&root, &root);
    let error = service
        .start_publish("browser-controlled-value", false)
        .unwrap_err();
    assert!(error.to_string().contains("confirmed=true"));
    let _ = fs::remove_dir_all(root);
}

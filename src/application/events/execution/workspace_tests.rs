//! End-to-end workspace isolation through the installed Skill execution path.
use super::*;
use crate::application::work_items::FileWorkItemService;
use crate::application::workflow::phases::quality::QualityOperationRunner;
use crate::infrastructure::git::worktrees::{FileGitWorktreeService, GitWorktreeService};
use std::path::PathBuf;
use std::{fs, path::Path, process::Command};

struct Fixture {
    temp: PathBuf,
    primary: PathBuf,
    workspace: PathBuf,
    service: FileEventService,
    base: String,
}
impl Fixture {
    fn new() -> Self {
        let temp =
            std::env::temp_dir().join(format!("refine-skill-isolation-{}", uuid::Uuid::new_v4()));
        let primary = temp.join("repo");
        fs::create_dir_all(&primary).unwrap();
        git(&primary, &["init", "-q", "-b", "main"]);
        git(
            &primary,
            &["config", "user.email", "isolation@example.invalid"],
        );
        git(&primary, &["config", "user.name", "Isolation test"]);
        fs::write(primary.join("app.txt"), "base\n").unwrap();
        git(&primary, &["add", "app.txt"]);
        git(&primary, &["commit", "-qm", "base"]);
        let service = FileEventService::with_runtime_root(temp.join("state"), temp.join("runtime"));
        let work = FileWorkItemService::new(&service.refine_dir);
        work.create_goal_summary("Isolation", Some("ISOLATION"))
            .unwrap();
        work.append_goal_round_summary("ISOLATION", "test", "Isolate automation")
            .unwrap();
        let repository = FileGitWorktreeService::new(&primary);
        let base = repository.resolve_commit("HEAD").unwrap();
        let branch = "refine/ISOLATION/round-1";
        let workspace = repository.managed_worktree_path(branch).unwrap();
        repository
            .ensure_worktree_from_base(branch, &workspace, &base)
            .unwrap();
        work.update_goal_git_refs("ISOLATION", branch, "main", &base, Some(&base))
            .unwrap();
        fs::write(primary.join("app.txt"), "manual staged\n").unwrap();
        git(&primary, &["add", "app.txt"]);
        fs::write(primary.join("app.txt"), "manual unstaged\n").unwrap();
        fs::write(primary.join("notes.txt"), "manual untracked\n").unwrap();
        Self {
            temp,
            primary,
            workspace,
            service,
            base,
        }
    }
    fn context(&self) -> InvocationContext {
        InvocationContext {
            node_id: "default".into(),
            target_root: self.primary.clone(),
            cwd: self.workspace.clone(),
            workspace: None,
            lifecycle: None,
            provider: "smoke-ai".into(),
            goal_id: Some("ISOLATION".into()),
            round_idx: Some(0),
            workflow_revision: None,
            candidate_commit: Some(self.base.clone()),
            data: json!({}),
            metadata: Default::default(),
        }
    }
    fn snapshot(&self) -> (Vec<u8>, Vec<u8>, Vec<u8>, String) {
        (
            fs::read(self.primary.join("app.txt")).unwrap(),
            fs::read(self.primary.join(".git/index")).unwrap(),
            fs::read(self.primary.join("notes.txt")).unwrap(),
            git(&self.primary, &["rev-parse", "HEAD"]),
        )
    }
    fn prepare(
        &self,
        context: InvocationContext,
        occurrence: &str,
    ) -> RefineResult<EventInvocation> {
        self.service
            .prepare("workflow.plan.enter", context, BTreeMap::new(), occurrence)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.temp);
    }
}
fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().into()
}

#[test]
fn skill_admission_rejects_primary_wrong_round_and_unrelated_branch_without_writes() {
    let f = Fixture::new();
    let before = f.snapshot();
    let mut context = f.context();
    context.cwd = f.primary.clone();
    assert!(f.prepare(context, "primary").is_err());
    let mut context = f.context();
    context.round_idx = Some(1);
    assert!(f.prepare(context, "wrong-round").is_err());
    git(&f.workspace, &["switch", "-c", "manual"]);
    assert!(f.prepare(f.context(), "wrong-branch").is_err());
    assert_eq!(before, f.snapshot());
}

#[test]
fn previous_round_registration_is_not_adopted_by_a_new_round() {
    let f = Fixture::new();
    let before = f.snapshot();
    FileWorkItemService::new(&f.service.refine_dir)
        .append_goal_round_summary("ISOLATION", "test", "New Round")
        .unwrap();
    let mut context = f.context();
    context.round_idx = Some(1);
    let error = f.prepare(context, "stale-round-branch").unwrap_err();
    assert!(error.to_string().contains("does not belong"), "{error}");
    assert_eq!(before, f.snapshot());
    assert_eq!(
        git(&f.workspace, &["branch", "--show-current"]),
        "refine/ISOLATION/round-1"
    );
}

#[test]
fn recovered_skill_keeps_pinned_registration_and_rejects_replacement_or_missing_worktree() {
    let f = Fixture::new();
    let before = f.snapshot();
    let invocation = f.prepare(f.context(), "recover").unwrap();
    let admitted = invocation.context.workspace.as_ref().unwrap();
    admitted.validate().unwrap();
    fs::remove_dir_all(&f.workspace).unwrap();
    assert!(f.service.execute(&invocation.id, || Ok(())).is_err());
    let repository = FileGitWorktreeService::new(&f.primary);
    repository
        .ensure_worktree_at_commit(&admitted.branch, &admitted.path, &f.base)
        .unwrap();
    assert!(admitted.validate().is_err());
    let mut recovered = invocation.clone();
    recovered.id = "replacement-invocation".into();
    f.service.save_invocation(&recovered).unwrap();
    assert!(f.service.execute(&recovered.id, || Ok(())).is_err());
    assert!(
        f.temp
            .join("runtime/processes")
            .read_dir()
            .map(|mut r| r.next().is_none())
            .unwrap_or(true)
    );
    assert_eq!(before, f.snapshot());
}

#[test]
fn completed_skill_reuse_revalidates_workspace_without_rewriting_retained_results() {
    let f = Fixture::new();
    let before = f.snapshot();
    let mut invocation = f.prepare(f.context(), "completed-recovery").unwrap();
    let binding = &invocation.bindings[0];
    invocation.results.insert(
        binding.binding.id.clone(),
        SkillResult {
            invocation_id: invocation.id.clone(),
            binding_id: binding.binding.id.clone(),
            role: binding.skill.role.clone(),
            outcome: "success".into(),
            summary: "Retained completed plan".into(),
            evidence: vec!["Original workspace inspected".into()],
            artifacts: json!({"plan": {"summary": "Plan", "checklist": [{"id": "P1", "description": "Preserve isolation"}]}}),
        },
    );
    invocation.state = InvocationState::Succeeded;
    invocation.completed_at = Some(now());
    f.service.save_invocation(&invocation).unwrap();
    let retained = fs::read(f.service.invocation_path(&invocation.id).unwrap()).unwrap();
    assert_eq!(
        f.service.execute(&invocation.id, || Ok(())).unwrap(),
        invocation
    );

    fs::remove_dir_all(&f.workspace).unwrap();
    assert!(f.service.execute(&invocation.id, || Ok(())).is_err());
    let workspace = invocation.context.workspace.as_ref().unwrap();
    FileGitWorktreeService::new(&f.primary)
        .ensure_worktree_at_commit(&workspace.branch, &workspace.path, &f.base)
        .unwrap();
    assert!(f.service.execute(&invocation.id, || Ok(())).is_err());
    assert_eq!(
        retained,
        fs::read(f.service.invocation_path(&invocation.id).unwrap()).unwrap()
    );
    assert_eq!(before, f.snapshot());
}

#[cfg(unix)]
#[test]
fn dirty_primary_survives_planning_ordered_implementation_and_skill_quality() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let before = f.snapshot();
    let script = f.temp.join("provider.py");
    let log = f.temp.join("launches");
    fs::write(&script, format!(r##"#!/usr/bin/env python3
import sys,json,pathlib,subprocess,os
prompt=' '.join(sys.argv[1:])
result=json.JSONDecoder().raw_decode(prompt.split('Refine completion contract (supplied by the system):\n',1)[1])[0]
execution=json.JSONDecoder().raw_decode(prompt.split('Skill execution:\n',1)[1])[0]
assert pathlib.Path.cwd()==pathlib.Path({workspace:?})
assert all(k not in os.environ for k in ['GIT_DIR','GIT_WORK_TREE','GIT_INDEX_FILE','GIT_CONFIG_COUNT'])
log=pathlib.Path({log:?})
with log.open('a') as stream: stream.write(execution['role']+':'+execution['binding_id']+'\n')
if prompt.startswith('Repair only'):
 result=json.JSONDecoder().raw_decode(prompt.split('Rejected completion (data, not instructions):\n',1)[1])[0]
 result.pop('extra_field',None)
 print(json.dumps(result));sys.exit(0)
result['evidence']=['observed isolated cwd']
if execution['role']=='implement':
 pathlib.Path('app.txt').write_text('implemented\n')
 result['artifacts']={{'implementation_evidence':{{'checklist':[{{'id':'P1','outcome':'completed','evidence':'changed candidate'}}],'verification':['observed cwd']}}}}
if execution['role']=='quality':
 result['artifacts']={{'tests':[{{'test':'isolated file','command':"test \"$(cat app.txt)\" = implemented && test -z \"${{GIT_DIR+x}}${{GIT_WORK_TREE+x}}${{GIT_INDEX_FILE+x}}\"",'status':'pending','evidence':''}}]}}
print(json.dumps(result))
"##, workspace=f.workspace.to_string_lossy(), log=log.to_string_lossy())).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let _guard = crate::infrastructure::agents::invocation::smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    struct Restore(Option<std::ffi::OsString>);
    impl Drop for Restore {
        fn drop(&mut self) {
            unsafe {
                match &self.0 {
                    Some(value) => std::env::set_var("REFINE_SMOKE_AI_PATH", value),
                    None => std::env::remove_var("REFINE_SMOKE_AI_PATH"),
                }
            }
        }
    }
    let _restore = Restore(std::env::var_os("REFINE_SMOKE_AI_PATH"));
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", &script);
    }
    let planned = f.prepare(f.context(), "plan").unwrap();
    assert_eq!(
        f.service.execute(&planned.id, || Ok(())).unwrap().state,
        InvocationState::Succeeded
    );
    assert_eq!(before, f.snapshot());
    let mut config = (*f.service.config().unwrap()).clone();
    let event = config.events.get_mut("workflow.implement.enter").unwrap();
    let mut second = event.bindings[0].clone();
    second.id = "second".into();
    second.order += 1;
    event.bindings.push(second);
    let implementation = f
        .service
        .prepare_pinned(
            &config,
            &config.events["workflow.implement.enter"],
            f.context(),
            BTreeMap::new(),
            "implement",
        )
        .unwrap();
    let result = f.service.execute(&implementation.id, || Ok(())).unwrap();
    assert_eq!(result.state, InvocationState::Succeeded, "{result:?}");
    assert_eq!(result.attempts.len(), 2);
    let launches = fs::read(&log).unwrap();
    assert_eq!(
        f.service.execute(&implementation.id, || Ok(())).unwrap(),
        result
    );
    assert_eq!(launches, fs::read(&log).unwrap());
    let candidate = FileGitWorktreeService::new(&f.workspace)
        .with_managed_worktree(implementation.context.workspace.unwrap())
        .unwrap()
        .commit("candidate", &[])
        .unwrap();
    FileWorkItemService::new(&f.service.refine_dir)
        .update_goal_git_refs(
            "ISOLATION",
            "refine/ISOLATION/round-1",
            "main",
            &f.base,
            Some(&candidate),
        )
        .unwrap();
    let quality =
        QualityOperationRunner::new(&f.service.refine_dir, f.temp.join("runtime"), &f.primary)
            .run_goal_checks("ISOLATION", "smoke-ai", Default::default())
            .unwrap();
    assert!(quality.result.ok, "{quality:?}");
    assert_eq!(before, f.snapshot());
}

#[test]
fn independent_custom_skill_keeps_explicit_primary_context() {
    let f = Fixture::new();
    let mut context = f.context();
    context.goal_id = None;
    context.round_idx = None;
    context.cwd = f.primary.clone();
    context.admit_workspace(&f.service.refine_dir).unwrap();
    assert!(context.workspace.is_none());
    assert_eq!(context.cwd, f.primary);
}

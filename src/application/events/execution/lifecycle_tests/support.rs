use super::*;
use crate::application::work_items::FileWorkItemService;
use crate::infrastructure::git::worktrees::FileGitWorktreeService;
use std::{fs, path::Path, process::Command};

pub(super) struct Fixture {
    pub temp: PathBuf,
    pub primary: PathBuf,
    pub service: FileEventService,
    pub base: String,
}
impl Fixture {
    pub fn new() -> Self {
        let temp = std::env::temp_dir().join(format!("refine-lifecycle-{}", uuid::Uuid::new_v4()));
        let primary = temp.join("repo");
        fs::create_dir_all(primary.join("app")).unwrap();
        git(&primary, &["init", "-q", "-b", "main"]);
        git(
            &primary,
            &["config", "user.email", "lifecycle@example.invalid"],
        );
        git(&primary, &["config", "user.name", "Lifecycle test"]);
        fs::write(primary.join("app/tracked.txt"), "base\n").unwrap();
        git(&primary, &["add", "."]);
        git(&primary, &["commit", "-qm", "base"]);
        let base = git(&primary, &["rev-parse", "HEAD"]);
        // Deliberately leave the human checkout on a different branch/commit.
        git(&primary, &["switch", "-qc", "manual"]);
        fs::write(primary.join("manual-only.txt"), "manual\n").unwrap();
        git(&primary, &["add", "."]);
        git(&primary, &["commit", "-qm", "manual"]);
        fs::write(primary.join("app/tracked.txt"), "staged\n").unwrap();
        git(&primary, &["add", "."]);
        fs::write(primary.join("app/tracked.txt"), "unstaged\n").unwrap();
        fs::write(primary.join("untracked.txt"), "untracked\n").unwrap();
        let service = FileEventService::with_runtime_root(temp.join("state"), temp.join("runtime"));
        let work = FileWorkItemService::new(&service.refine_dir);
        work.create_goal_summary("Fresh Goal", Some("FRESH"))
            .unwrap();
        work.append_goal_round_summary("FRESH", "test", "Inspect before admission")
            .unwrap();
        Self {
            temp,
            primary,
            service,
            base,
        }
    }
    pub fn work(&self) -> FileWorkItemService {
        FileWorkItemService::new(&self.service.refine_dir)
    }
    pub fn snapshot(&self) -> (Vec<u8>, Vec<u8>, Vec<u8>, String, String) {
        (
            fs::read(self.primary.join("app/tracked.txt")).unwrap(),
            fs::read(self.primary.join(".git/index")).unwrap(),
            fs::read(self.primary.join("untracked.txt")).unwrap(),
            git(&self.primary, &["rev-parse", "HEAD"]),
            git(&self.primary, &["show", ":app/tracked.txt"]),
        )
    }
    pub fn gate(&self, source: &str, mode: BindingMode) {
        let mut config = (*self.service.config().unwrap()).clone();
        let id = format!("{}-gate", source.replace('.', "-"));
        let mut skill = config.skills["default-plan"].clone();
        skill.id = id.clone();
        skill.role = "task".into();
        skill.parameters = vec![Parameter {
            name: "workspace".into(),
            required: true,
            ..Default::default()
        }];
        config.skills.insert(id.clone(), skill);
        let mut binding = config.events["workflow.plan.enter"].bindings[0].clone();
        binding.id = id.clone();
        binding.skill_id = id;
        binding.mode = mode;
        binding
            .inputs
            .insert("workspace".into(), "system.workspace".into());
        config
            .events
            .get_mut(source)
            .unwrap()
            .bindings
            .push(binding);
        crate::infrastructure::storage::automation::AutomationStore::new(&self.service.refine_dir)
            .update(config.revision, |stored| {
                *stored = config;
                Ok(())
            })
            .unwrap();
    }
    pub fn second_gate(&self, source: &str) {
        let mut config = (*self.service.config().unwrap()).clone();
        let first = config.events[source].bindings[0].clone();
        let mut skill = config.skills[&first.skill_id].clone();
        skill.id = "second-skill".into();
        config.skills.insert(skill.id.clone(), skill);
        let mut second = first;
        second.id = "second".into();
        second.skill_id = "second-skill".into();
        second.order += 1;
        config.events.get_mut(source).unwrap().bindings.push(second);
        crate::infrastructure::storage::automation::AutomationStore::new(&self.service.refine_dir)
            .update(config.revision, |stored| {
                *stored = config;
                Ok(())
            })
            .unwrap();
    }
    pub fn request_todo(&self) {
        let error = self
            .work()
            .transition_goal_status("FRESH", crate::model::workflow::GoalStatus::Todo)
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains(crate::application::events::transitions::PENDING),
            "{error}"
        );
    }
    pub fn dispatch(&self) {
        self.service.dispatch_goal_events(&self.primary).unwrap();
    }
    pub fn invocation(&self, source: &str) -> EventInvocation {
        let runs = self.service.goal_invocations("FRESH", 0, 100).unwrap();
        let run = runs["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|run| run["event"]["source"] == source)
            .unwrap();
        self.service
            .invocation(run["id"].as_str().unwrap())
            .unwrap()
    }
    pub fn execute(&self, invocation: &EventInvocation) -> EventInvocation {
        self.service
            .execute(&invocation.id, || {
                self.service.validate_manual_authority(invocation)
            })
            .unwrap()
    }
    pub fn assert_no_candidate(&self) {
        let goal = self.work().show_goal_detail("FRESH").unwrap();
        for field in ["branch_name", "candidate_commit", "base_commit"] {
            assert!(goal[field].is_null(), "{goal}");
        }
        let git = FileGitWorktreeService::new(&self.primary);
        assert!(
            git.resolve_commit("refs/heads/refine/FRESH/round-1")
                .is_err()
        );
        assert!(
            !git.managed_worktree_path("refine/FRESH/round-1")
                .unwrap()
                .exists()
        );
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.temp);
    }
}
pub(super) fn git(root: &Path, args: &[&str]) -> String {
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

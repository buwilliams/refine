use super::*;

pub fn bump_version(current: &str, bump: ReleaseBump) -> RefineResult<String> {
    let parts = current
        .split('.')
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| RefineError::InvalidInput(format!("invalid semantic version: {current}")))?;
    if parts.len() != 3 {
        return Err(RefineError::InvalidInput(format!(
            "invalid semantic version: {current}"
        )));
    }
    let (major, minor, patch) = (parts[0], parts[1], parts[2]);
    Ok(match bump {
        ReleaseBump::Major => format!("{}.0.0", major + 1),
        ReleaseBump::Minor => format!("{}.{}.0", major, minor + 1),
        ReleaseBump::Patch => format!("{}.{}.{}", major, minor, patch + 1),
    })
}

pub(super) fn release_goal_prompt(plan: &ReleasePlan) -> String {
    let changes = plan
        .changes
        .iter()
        .map(|change| format!("- {} {}", change.commit, change.summary))
        .collect::<Vec<_>>()
        .join("\n");
    let goals = plan.completed_goals.join("\n");
    let version_files = plan.version_files.join(", ");
    let documentation_files = plan.documentation_files.join(", ");
    render(
        PromptTemplate::ReleaseGoal,
        &[
            ("current_version", &plan.current_version),
            ("proposed_version", &plan.proposed_version),
            ("proposed_tag", &plan.proposed_tag),
            (
                "previous_tag",
                plan.previous_tag.as_deref().unwrap_or("none"),
            ),
            ("version_files", &version_files),
            ("documentation_files", &documentation_files),
            (
                "completed_goals",
                if goals.is_empty() { "- None" } else { &goals },
            ),
            (
                "changes",
                if changes.is_empty() {
                    "- None"
                } else {
                    &changes
                },
            ),
        ],
    )
}

pub(super) fn read_package_version(path: &Path) -> RefineResult<String> {
    let text = fs::read_to_string(path).map_err(io_error("read package manifest"))?;
    text.lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("version = \"")
                .and_then(|value| value.strip_suffix('"'))
        })
        .map(str::to_string)
        .ok_or_else(|| {
            RefineError::InvalidInput(format!("package version not found in {}", path.display()))
        })
}

pub(super) fn latest_semver_tag(root: &Path) -> RefineResult<Option<String>> {
    let tags = command_text(root, "git", &["tag", "--merged", "HEAD"])?;
    let mut versions = tags
        .lines()
        .filter_map(|tag| {
            let raw = tag.strip_prefix('v').unwrap_or(tag);
            let parts = raw
                .split('.')
                .map(str::parse::<u64>)
                .collect::<Result<Vec<_>, _>>()
                .ok()?;
            (parts.len() == 3).then_some(((parts[0], parts[1], parts[2]), tag.to_string()))
        })
        .collect::<Vec<_>>();
    versions.sort_by_key(|(version, _)| *version);
    Ok(versions.pop().map(|(_, tag)| tag))
}

pub(super) fn release_gate_commands(root: &Path) -> Vec<String> {
    let mut gates = vec!["git diff --check".to_string()];
    if root.join("Cargo.toml").is_file() {
        gates.extend([
            "cargo fmt --all -- --check".to_string(),
            "cargo clippy --all-targets -- -D warnings".to_string(),
            "cargo test --lib --bins -- --test-threads=1".to_string(),
            "cargo build --release --locked".to_string(),
            "cargo run --manifest-path xtask/Cargo.toml -- release-check".to_string(),
        ]);
    }
    gates
}

pub(super) fn completed_goal_summaries(root: &Path) -> RefineResult<Vec<String>> {
    let candidates = [
        root.join(".refine/goals"),
        refine_dir_for_target_root(root)?.join("goals"),
    ];
    let Some(goals_root) = candidates.into_iter().find(|path| path.is_dir()) else {
        return Ok(Vec::new());
    };
    let mut files = Vec::new();
    collect_named_files(&goals_root, "goal.json", &mut files)?;
    let mut goals = Vec::new();
    for path in files {
        let value: Value =
            serde_json::from_slice(&fs::read(&path).map_err(io_error("read Goal record"))?)
                .map_err(|error| {
                    RefineError::Serialization(format!(
                        "failed to parse {}: {error}",
                        path.display()
                    ))
                })?;
        if value.get("status").and_then(Value::as_str) == Some("done") {
            let id = value.get("id").and_then(Value::as_str).unwrap_or("Goal");
            let name = value
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("completed");
            goals.push(format!("{id}: {name}"));
        }
    }
    goals.sort();
    Ok(goals)
}

pub(super) fn delivery_workflows_configured(root: &Path) -> RefineResult<bool> {
    let workflows = root.join(".github/workflows");
    if !workflows.is_dir() {
        return Ok(false);
    }
    for entry in fs::read_dir(workflows).map_err(io_error("read workflow directory"))? {
        let entry = entry.map_err(io_error("inspect workflow entry"))?;
        let path = entry.path();
        if !matches!(
            path.extension().and_then(|value| value.to_str()),
            Some("yml" | "yaml")
        ) {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let body = fs::read_to_string(&path)
            .map_err(io_error("read workflow definition"))?
            .to_ascii_lowercase();
        if ["deploy", "publish", "package", "release"]
            .iter()
            .any(|term| name.contains(term) || body.contains(term))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn collect_named_files(
    root: &Path,
    name: &str,
    files: &mut Vec<PathBuf>,
) -> RefineResult<()> {
    for entry in fs::read_dir(root).map_err(io_error("read Goal state directory"))? {
        let entry = entry.map_err(io_error("inspect Goal state entry"))?;
        let path = entry.path();
        if path.is_dir() {
            collect_named_files(&path, name, files)?;
        } else if path.file_name().and_then(|value| value.to_str()) == Some(name) {
            files.push(path);
        }
    }
    Ok(())
}

pub(super) fn ensure_git_checkout(root: &Path) -> RefineResult<()> {
    if !root.join(".git").exists() {
        return Err(RefineError::InvalidInput(format!(
            "{} is not a Git checkout",
            root.display()
        )));
    }
    Ok(())
}

pub(super) fn command_text(root: &Path, program: &str, args: &[&str]) -> RefineResult<String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|error| RefineError::Io(format!("failed to run {program}: {error}")))?;
    if !output.status.success() {
        return Err(RefineError::Degraded(format!(
            "{} {} failed: {}",
            program,
            args.join(" "),
            combined_output(&output)
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub(super) fn command_optional(
    root: &Path,
    program: &str,
    args: &[&str],
) -> RefineResult<Option<String>> {
    let output = Command::new(program)
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|error| RefineError::Io(format!("failed to run {program}: {error}")))?;
    Ok(output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string()))
}

pub(super) fn combined_output(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .trim()
    .to_string()
}

pub(super) fn io_error(action: &'static str) -> impl FnOnce(std::io::Error) -> RefineError {
    move |error| RefineError::Io(format!("failed to {action}: {error}"))
}

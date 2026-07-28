use super::*;

#[test]
fn agent_open_parses_goal_instances_and_profiles() {
    let goal = Cli::try_parse_from(["refine", "agent", "open", "GOAL1"]).unwrap();
    assert!(matches!(
        goal.command,
        Commands::Agent {
            action: AgentAction::Open {
                goal_id: Some(ref goal_id),
                profile: CliAgentProfile::Agent,
                prompt: None,
            }
        } if goal_id == "GOAL1"
    ));

    let plan = Cli::try_parse_from([
        "refine",
        "agent",
        "open",
        "--profile",
        "plan",
        "--prompt",
        "Design retries",
    ])
    .unwrap();
    assert!(matches!(
        plan.command,
        Commands::Agent {
            action: AgentAction::Open {
                goal_id: None,
                profile: CliAgentProfile::Plan,
                prompt: Some(ref prompt),
            }
        } if prompt == "Design retries"
    ));
}

#[test]
fn explicit_target_root_path_detects_internal_cli_escape_hatch() {
    let target_root = PathBuf::from("/tmp/refine-state");
    let command = Commands::Goal {
        action: GoalAction::Create {
            name: "direct write".to_string(),
            target_root: Some(target_root.clone()),
            id: None,
        },
    };
    assert_eq!(explicit_target_root_path(&command), Some(&target_root));

    let default_daemon_command = Commands::Workflow {
        action: WorkflowAction::Pause {
            runtime_root: PathBuf::from("run"),
        },
    };
    assert_eq!(explicit_target_root_path(&default_daemon_command), None);
}

#[test]
fn project_worktree_cleanup_is_dry_run_by_default_and_requires_explicit_apply() {
    let parsed = Cli::try_parse_from(["refine", "project", "cleanup-worktrees"]).unwrap();
    assert!(matches!(
        parsed.command,
        Commands::Project {
            action: ProjectAction::CleanupWorktrees {
                apply: false,
                older_than_seconds: 0,
                target_root: None,
                ..
            }
        }
    ));

    let parsed = Cli::try_parse_from([
        "refine",
        "project",
        "cleanup-worktrees",
        "--apply",
        "--older-than-seconds",
        "3600",
    ])
    .unwrap();
    assert!(matches!(
        parsed.command,
        Commands::Project {
            action: ProjectAction::CleanupWorktrees {
                apply: true,
                older_than_seconds: 3600,
                ..
            }
        }
    ));
}

#[test]
fn todo_commands_parse_explicit_reporter_and_identifiers() {
    let parsed = Cli::try_parse_from([
        "refine",
        "todo",
        "done",
        "LIST-1",
        "ITEM-1",
        "--reporter",
        "Buddy Williams",
    ])
    .unwrap();
    assert!(matches!(
        parsed.command,
        Commands::Todo {
            action: TodoAction::Done {
                list_id,
                item_id,
                reporter,
                target_root: None,
            }
        } if list_id == "LIST-1"
            && item_id == "ITEM-1"
            && reporter == "Buddy Williams"
    ));
    assert!(Cli::try_parse_from(["refine", "todo", "list"]).is_err());
    assert!(
        Cli::try_parse_from(["refine", "todo", "done", "LIST-1", "--reporter", "Buddy"]).is_err()
    );
}

#[test]
fn default_static_root_finds_checkout_assets() {
    let root = super::helpers::default_static_root().expect("static root should exist");
    assert!(root.join("index.html").is_file());
}

#[test]
fn website_command_owns_static_site_options() {
    let parsed = Cli::try_parse_from(["refine", "website"]).unwrap();
    let Commands::Website { bind_address, .. } = parsed.command else {
        panic!("expected website command");
    };
    assert_eq!(bind_address, IpAddr::V4(Ipv4Addr::LOCALHOST));

    let parsed = Cli::try_parse_from([
        "refine",
        "website",
        "--port",
        "0",
        "--bind-address",
        "0.0.0.0",
        "--static-root",
        ".",
        "--once",
    ])
    .unwrap();
    let Commands::Website {
        port,
        bind_address,
        static_root,
        once,
    } = parsed.command
    else {
        panic!("expected website command");
    };
    assert_eq!(port, 0);
    assert_eq!(bind_address, IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    assert_eq!(static_root, PathBuf::from("."));
    assert!(once);
    assert_eq!(
        explicit_target_root_path(&Commands::Website {
            port,
            bind_address,
            static_root,
            once,
        }),
        None
    );
}

#[test]
fn feature_import_parses_structured_project_spec_with_shared_import_service() {
    let temp_root = unique_temp_dir("cli-feature-import-project-spec");
    let target_root = temp_root.clone();
    let refine_dir = target_root.join(".refine");
    dispatch(
        Cli::try_parse_from([
            "refine",
            "feature",
            "create",
            "Imported Project Feature",
            "--target-root",
            target_root.to_str().unwrap(),
            "--id",
            "FEA1",
        ])
        .unwrap(),
    )
    .unwrap();

    let spec = r#"{
        "project": {
            "name": "Budget App",
            "features": [
                {
                    "name": "Transactions",
                    "goals": [
                        {
                            "title": "Categorize transactions",
                            "current_state": "Transactions are uncategorized.",
                            "desired_state": "Users can categorize each transaction.",
                            "priority": "medium"
                        }
                    ]
                }
            ]
        }
    }"#;
    dispatch(
        Cli::try_parse_from([
            "refine",
            "feature",
            "import",
            "--target-root",
            target_root.to_str().unwrap(),
            "--text",
            spec,
            "--reporter",
            "Product",
            "--feature-id",
            "FEA1",
        ])
        .unwrap(),
    )
    .unwrap();

    let snapshot = FileProjectStateStore::new(&refine_dir)
        .rebuild_projection()
        .unwrap();
    let goal = snapshot.goals.values().next().unwrap();
    assert_eq!(goal.goal.name, "Categorize transactions");
    assert_eq!(goal.goal.feature_id.as_deref(), Some("FEA1"));
    assert_eq!(goal.goal.priority.as_str(), "medium");
    assert_eq!(goal.goal.reporter.as_deref(), Some("Product"));

    fs::remove_dir_all(temp_root).unwrap();
}

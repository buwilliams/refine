use super::*;
use crate::infrastructure::agents::invocation::{
    AgentProviderService, HostAgentProviderService, ProviderInvocation,
};
use crate::infrastructure::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::model::providers::{ProviderDefinition, ProviderPromptCapability, defaults, expand};
use serde_json::json;

fn root() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("refine-providers-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn validates_catalog_and_substitutes_only_the_original_template() {
    let mut catalog = defaults();
    catalog.validate().unwrap();
    let context = "quotes '\" spaces\nUnicode λ $(touch /tmp/never) {{cwd}} {{context}}";
    assert_eq!(
        expand("a{{context}}b{{cwd}}", context, "/work", ""),
        format!("a{context}b/work")
    );
    catalog.providers[0]
        .automated
        .args
        .push("{{unknown}}".into());
    assert!(catalog.validate().is_err());
    catalog = defaults();
    catalog.providers.push(catalog.providers[0].clone());
    assert!(catalog.validate().is_err());
    catalog = defaults();
    catalog.default_provider = "missing".into();
    assert!(catalog.validate().is_err());
    catalog = defaults();
    catalog.providers[0].executable = "".into();
    assert!(catalog.validate().is_err());
}

#[test]
fn inheritance_survives_unrelated_saves_migrations_copy_and_default_changes() {
    let root = root();
    let settings = FileSettingsService::for_node(&root, "default");
    assert_eq!(settings.load().unwrap()["agent_cli"], "claude");
    assert!(!root.join("providers.json").exists());
    settings.update(&json!({"parallel_run_cap":"2"})).unwrap();
    assert_eq!(settings.provider_override().unwrap(), None);
    let store = ProviderStore::new(&root);
    let mut catalog = store.load().unwrap();
    catalog.default_provider = "codex".into();
    let saved = store.save(catalog.clone()).unwrap();
    assert!(store.save(catalog).is_err());
    assert_eq!(settings.load().unwrap()["agent_cli"], "codex");
    settings.update(&json!({"agent_cli":"gemini"})).unwrap();
    let other = FileSettingsService::for_node(&root, "other");
    other.update(&json!({"parallel_run_cap":"3"})).unwrap();
    settings.copy_from_node("other", "runtime").unwrap();
    assert_eq!(settings.provider_override().unwrap(), None);
    settings.update(&json!({"agent_cli":"claude"})).unwrap();
    let mut catalog = saved;
    catalog.providers.retain(|p| p.id != "claude");
    assert!(store.save(catalog.clone()).is_err());
    settings.update(&json!({"agent_cli":null})).unwrap();
    store.save(catalog).unwrap();
    assert_eq!(settings.load().unwrap()["agent_cli"], "codex");
    assert_eq!(response(&root, None).unwrap()["selection_source"], "system");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_generic_selection_preserves_case_and_becomes_a_definition() {
    let root = root();
    let settings = FileSettingsService::new(&root);
    settings.update(&json!({"parallel_run_cap":"2"})).unwrap();
    let path = root.join("nodes.json");
    let mut nodes: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    nodes["nodes"][0]["settings"]["agent_cli"] = json!("/Some Path/MyAgent");
    nodes["nodes"][0]["settings"]["paused"] = json!("true");
    std::fs::write(&path, serde_json::to_vec(&nodes).unwrap()).unwrap();
    assert_eq!(settings.load().unwrap()["agent_cli"], "/Some Path/MyAgent");
    let store = ProviderStore::new(&root);
    let catalog = store.save(store.load().unwrap()).unwrap();
    assert_eq!(
        catalog.provider("/Some Path/MyAgent").unwrap().executable,
        "/Some Path/MyAgent"
    );
    assert_eq!(settings.load().unwrap()["agent_cli"], "/Some Path/MyAgent");
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn configuration_only_provider_launches_exact_argv_stdin_and_updated_parameters() {
    use std::os::unix::fs::PermissionsExt;
    let root = root();
    let binary = root.join("Agent With Spaces");
    std::fs::write(&binary, "#!/usr/bin/python3\nimport json,sys,os\nprint(json.dumps({'args':sys.argv[1:],'stdin':sys.stdin.read() if '--stdin' in sys.argv else '', 'cwd':os.getcwd()}))\n").unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut catalog = defaults();
    let mut custom = ProviderDefinition::generic("UnfamiliarAgent");
    custom.executable = binary.to_string_lossy().into_owned();
    custom.automated.args = vec!["first parameter".into()];
    catalog.providers.push(custom);
    let mut catalog = save(&root, &json!(catalog)).unwrap();
    FileSettingsService::new(&root)
        .update(&json!({"agent_cli":"UnfamiliarAgent"}))
        .unwrap();
    let service =
        HostAgentProviderService::with_runtime_root(root.join("run/8082")).with_refine_dir(&root);
    let context = "space '\"\nλ $(echo nope) {{context}} {{cwd}}";
    let invoke = || {
        let result = service
            .invoke(ProviderInvocation {
                provider: String::new(),
                prompt: context.into(),
                session_id: None,
                cwd: Some(root.to_string_lossy().into_owned()),
                stall_timeout_seconds: Some(10),
                process_metadata: Default::default(),
            })
            .unwrap();
        serde_json::from_str::<Value>(&result).unwrap()
    };
    assert_eq!(invoke()["args"], json!(["first parameter", context]));
    assert_eq!(
        service
            .interactive_command("UnfamiliarAgent", context)
            .unwrap()
            .args,
        vec![context]
    );
    assert!(service.configure("missing-id").is_err());
    let custom = catalog
        .providers
        .iter_mut()
        .find(|p| p.id == "UnfamiliarAgent")
        .unwrap();
    custom.automated.args = vec!["--stdin".into(), "edited parameter".into()];
    custom.automated.context_args.clear();
    custom.automated.transport = ProviderPromptCapability::NativeStdin;
    custom.automated.stdin = Some("prefix\n{{context}}\nsuffix".into());
    save(&root, &json!(catalog)).unwrap();
    let actual = invoke();
    assert_eq!(actual["args"], json!(["--stdin", "edited parameter"]));
    assert_eq!(actual["stdin"], format!("prefix\n{context}\nsuffix"));
    assert_eq!(actual["cwd"], root.to_string_lossy().as_ref());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn concurrent_catalog_edits_have_one_winner_and_targets_resolve_independently() {
    let a = root();
    let b = root();
    let mut catalog = defaults();
    catalog.default_provider = "codex".into();
    ProviderStore::new(&a).save(catalog).unwrap();
    let barrier = std::sync::Barrier::new(2);
    let results = std::thread::scope(|scope| {
        let handles: Vec<_> = ["gemini", "copilot"]
            .into_iter()
            .map(|id| {
                let barrier = &barrier;
                let root = &a;
                scope.spawn(move || {
                    let store = ProviderStore::new(root);
                    let mut draft = store.load().unwrap();
                    draft.default_provider = id.into();
                    barrier.wait();
                    store.save(draft)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    let first = HostAgentProviderService::new().with_refine_dir(&a);
    let second = HostAgentProviderService::new().with_refine_dir(&b);
    assert_ne!(
        first.selected_provider_id("").unwrap(),
        second.selected_provider_id("").unwrap()
    );
    assert_eq!(second.selected_provider_id("").unwrap(), "claude");
    assert_eq!(first.selected_provider_id("codex").unwrap(), "codex");
    std::fs::remove_dir_all(a).unwrap();
    std::fs::remove_dir_all(b).unwrap();
}

#[test]
fn rejected_provider_edits_preserve_the_saved_catalog_and_selection() {
    let root = root();
    let store = ProviderStore::new(&root);
    let saved = store.save(defaults()).unwrap();
    FileSettingsService::for_node(&root, "other")
        .update(&json!({"agent_cli":"gemini"}))
        .unwrap();
    let before = std::fs::read(root.join("providers.json")).unwrap();
    let mut invalid = Vec::new();
    for mode in ["automated", "interactive"] {
        for (key, value) in [
            ("args", json!("--prompt {{context}}")),
            ("args", json!([42])),
            ("args", json!(["{{unknown}}"])),
            ("args", json!(["{{context"])),
            ("args", json!(["\u{0}"])),
            ("args", json!(["{{session_id}}"])),
            ("transport", json!("native_stdin")),
            ("stdin", json!("{{context}}")),
        ] {
            let mut draft = json!(saved);
            draft["providers"][0][mode][key] = value;
            invalid.push(draft);
        }
    }
    let mut referenced = json!(saved);
    referenced["providers"]
        .as_array_mut()
        .unwrap()
        .retain(|p| p["id"] != "gemini");
    invalid.push(referenced);
    for draft in invalid {
        assert!(
            save(&root, &draft).is_err(),
            "accepted invalid catalog: {draft}"
        );
        assert_eq!(std::fs::read(root.join("providers.json")).unwrap(), before);
        assert_eq!(
            FileSettingsService::for_node(&root, "other")
                .load()
                .unwrap()["agent_cli"],
            "gemini"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn system_default_changes_only_inheriting_nodes_and_unknown_selections_can_be_repaired() {
    let root = root();
    let inherited = FileSettingsService::for_node(&root, "default");
    let selected = FileSettingsService::for_node(&root, "other");
    inherited.update(&json!({"parallel_run_cap":"2"})).unwrap();
    selected.update(&json!({"agent_cli":"gemini"})).unwrap();
    let mut catalog = ProviderStore::new(&root).load().unwrap();
    catalog.default_provider = "codex".into();
    save(&root, &json!(catalog)).unwrap();
    assert_eq!(inherited.load().unwrap()["agent_cli"], "codex");
    assert_eq!(selected.load().unwrap()["agent_cli"], "gemini");
    let path = root.join("nodes.json");
    let mut nodes: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let node = nodes["nodes"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|n| n["id"] == "default")
        .unwrap();
    node["settings"]["agent_cli"] = json!("removed-on-another-node");
    std::fs::write(path, serde_json::to_vec(&nodes).unwrap()).unwrap();
    assert_eq!(
        inherited.load().unwrap()["agent_cli"],
        "removed-on-another-node"
    );
    let response = response(&root, inherited.provider_override().unwrap().as_deref()).unwrap();
    assert!(
        response["selection_error"]
            .as_str()
            .unwrap()
            .contains("not configured")
    );
    let service = HostAgentProviderService::new().with_refine_dir(&root);
    assert!(service.selected_provider_id("").is_err());
    assert_eq!(service.selected_provider_id("claude").unwrap(), "claude");
    inherited.update(&json!({"agent_cli":null})).unwrap();
    assert_eq!(service.selected_provider_id("").unwrap(), "codex");
    std::fs::remove_dir_all(root).unwrap();
}

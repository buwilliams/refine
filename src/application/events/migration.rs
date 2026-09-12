use crate::error::{RefineError, RefineResult};
use crate::infrastructure::storage::automation::{read_json, write_json};
use crate::model::automation::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

pub(super) fn migrate(root: &Path) -> RefineResult<AutomationConfig> {
    let mut originals = serde_json::Map::new();
    for name in ["governance.json", "guidance.json", "quality/settings.json"] {
        let path = root.join(name);
        if path.exists() {
            originals.insert(name.into(), read_json::<Value>(&path)?);
        }
    }
    let quality_settings =
        crate::application::workflow::phases::quality::FileQualityService::new(root)
            .load_settings()?;
    let quality = serde_json::to_value(&quality_settings)
        .map_err(|e| RefineError::Serialization(e.to_string()))?;
    let mut config = build_config(&originals, &quality);
    single_trigger_skills(&mut config)?;
    // The source snapshot is written first. A crash retries deterministic conversion; once
    // config.json is installed the legacy files never regain configuration authority.
    let archive = root.join("automation/migration-v1.json");
    if !archive.exists() {
        write_json(
            &archive,
            &json!({"schema_version": 1, "sources": originals, "quality_prompt_hash": super::execution::stable_id(&config.skills["default-quality"].prompt), "quality_commands": quality_settings.legacy_commands}),
        )?;
    }
    config.validate().map_err(RefineError::InvalidInput)?;
    Ok(config)
}

fn title(s: &str) -> String {
    let mut chars = s.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

fn build_config(originals: &serde_json::Map<String, Value>, quality: &Value) -> AutomationConfig {
    let mut config = AutomationConfig {
        schema_version: SCHEMA_VERSION,
        revision: 1,
        skills: BTreeMap::new(),
        events: BTreeMap::new(),
    };
    let governance = originals
        .get("governance.json")
        .cloned()
        .unwrap_or(Value::Null);
    for (role, prompt) in [
        ("plan", "Produce a complete, concise implementation plan for the current Goal Round. Inspect the repository and applicable context, resolve assumptions, consider alternatives and critique your proposed solution before finalizing it. Choose the planning method appropriate to the work. For a scoped finding recovery Round, focus the plan on the requested correction against the retained candidate. Use your judgment to decide when planning is complete and share useful context with the implementation agent.".to_string()),
        ("implement", "Implement the current Goal Round using the available planning context and attached Skills. Inspect existing code, make the complete change, use your judgment to decide when the work is complete. Preserve unrelated changes and historical evidence. Use supported Refine controls for workflow changes, candidate publication, and integration. Follow current user authorization and preserve confirmation boundaries.".to_string()),
        ("quality", format!("Assess and correct the candidate implementation. Run meaningful tests, fix blocking failures including those outside the Goal’s scope, and rerun until they pass. Report success after verified corrections; stop unsuccessfully only for a blocker you cannot resolve within your authority or environment. Governance reviews how the overall Goal came together. Follow the project instructions.\n\nProject Quality instructions and tests:\n{}", serde_json::to_string_pretty(&quality).unwrap_or_default())),
        ("governance", format!("Review the exact candidate against the project's stated intent and attached Skills. Read the accepted plans, implementation and Quality evidence and actual diff. Use context to distinguish actual problems from hypothetical risks or preferences. Follow the Skill instructions and current user authorization; use supported Refine commands for workflow changes. Use your judgment to decide the outcome; share any useful context for subsequent work.\n\nProject intent and rules:\n{}", serde_json::to_string_pretty(&governance).unwrap_or_default())),
    ] {
        let id = format!("default-{role}");
        config.skills.insert(id.clone(), Skill { id, name: title(role), prompt, role: role.into(), enabled: true, scope: Scope::default(), parameters: Vec::new(), provenance: Some("refine-default-v1".into()) });
    }
    for source in system_catalog() {
        let role = source
            .strip_prefix("workflow.")
            .and_then(|s| s.strip_suffix(".enter"));
        let mut bindings = Vec::new();
        if let Some(role) =
            role.filter(|r| ["plan", "implement", "quality", "governance"].contains(r))
        {
            bindings.push(Binding {
                id: format!("default-{role}"),
                skill_id: format!("default-{role}"),
                enabled: true,
                mode: BindingMode::Blocking,
                order: 0,
                scope: Scope::default(),
                overrides: None,
                inputs: BTreeMap::new(),
            });
        }
        config.events.insert(
            source.clone(),
            EventDefinition {
                id: source.clone(),
                name: source.split('.').map(title).collect::<Vec<_>>().join(" "),
                kind: EventKind::System,
                source: Some(source),
                enabled: true,
                scope: Scope::default(),
                parameters: Vec::new(),
                bindings,
                on_success: None,
            },
        );
    }
    let guidance = originals.get("guidance.json").and_then(|v| {
        v.as_array()
            .or_else(|| v.get("guidance").and_then(Value::as_array))
    });
    for (index, entry) in guidance.into_iter().flatten().enumerate() {
        let id = entry
            .get("id")
            .and_then(Value::as_str)
            .filter(|v| valid_id(v) && v.len() <= 111)
            .map(|id| format!("guidance-{id}"))
            .unwrap_or_else(|| format!("guidance-{index}"));
        let prompt = format!(
            "Applicability: {}\n\n{}",
            entry
                .get("rule")
                .and_then(Value::as_str)
                .unwrap_or("When relevant to the work"),
            entry
                .get("instructions")
                .and_then(Value::as_str)
                .unwrap_or("")
        );
        let skill = Skill {
            id: id.clone(),
            name: entry
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("Imported context")
                .into(),
            prompt,
            role: "task".into(),
            enabled: entry
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            scope: Scope::default(),
            parameters: Vec::new(),
            provenance: Some("guidance-migration-v1".into()),
        };
        config.skills.insert(id.clone(), skill);
        for role in ["plan", "implement", "quality", "governance"] {
            config
                .events
                .get_mut(&format!("workflow.{role}.enter"))
                .expect("default Event")
                .bindings
                .push(Binding {
                    id: id.clone(),
                    skill_id: id.clone(),
                    enabled: true,
                    mode: BindingMode::Context,
                    order: index as i32,
                    scope: Scope::default(),
                    overrides: None,
                    inputs: BTreeMap::new(),
                });
        }
    }
    config
}

/// Exact pristine defaults, used to distinguish automatic seeding from authored
/// state during first attachment. Edited revisions never count as bootstrap data.
pub(crate) fn pristine_config_bytes() -> Vec<u8> {
    use crate::application::agent_io::prompts::{PromptEngine, PromptTemplate};
    let quality = json!({"configured":false,"business_requirements":"","instructions":PromptEngine::load(PromptTemplate::QualityDefaultInstructions),"tests":[],"legacy_commands":[],"enabled":"1"});
    serde_json::to_vec_pretty(&build_config(&Default::default(), &quality))
        .expect("static defaults are serializable")
}

/// Split authored assignments without changing binding identities, ordering,
/// overrides, parameters or historical invocation snapshots.
pub(super) fn single_trigger_skills(config: &mut AutomationConfig) -> RefineResult<()> {
    let originals = config.skills.clone();
    // Keep workflow defaults at their original phase so imported Quality checks
    // and established references retain their identity after splitting copies.
    let primary: BTreeMap<_, _> = originals
        .values()
        .filter_map(|skill| {
            let role = skill.id.strip_prefix("default-").unwrap_or(&skill.role);
            let preferred = format!("workflow.{role}.enter");
            config
                .events
                .values()
                .flat_map(|event| {
                    event
                        .bindings
                        .iter()
                        .filter(|binding| binding.skill_id == skill.id)
                        .map(move |binding| (event, binding))
                })
                .min_by_key(|(event, binding)| {
                    (
                        event.source.as_deref() != Some(preferred.as_str()),
                        binding.scope.node_id.is_some(),
                        &event.id,
                        &binding.id,
                    )
                })
                .map(|(event, binding)| (skill.id.clone(), (event.id.clone(), binding.id.clone())))
        })
        .collect();
    for event in config.events.values_mut() {
        for binding in &mut event.bindings {
            let original = originals.get(&binding.skill_id).ok_or_else(|| {
                RefineError::InvalidInput("Missing Skill during trigger migration".into())
            })?;
            let mut skill = original.clone();
            if primary.get(&original.id) != Some(&(event.id.clone(), binding.id.clone())) {
                let hash = super::execution::stable_id(&format!(
                    "{}:{}:{}",
                    original.id, event.id, binding.id
                ));
                skill.id = format!("skill-copy-{}", &hash[..24]);
                if config.skills.contains_key(&skill.id) {
                    return Err(RefineError::Conflict(
                        "A migrated Skill ID already exists; configuration was preserved".into(),
                    ));
                }
                skill.name = format!("{} · {}", original.name, event.name);
            }
            skill.scope = binding
                .scope
                .node_id
                .as_ref()
                .or(event.scope.node_id.as_ref())
                .map(|node| Scope {
                    node_id: Some(node.clone()),
                })
                .unwrap_or_else(|| original.scope.clone());
            skill.enabled &= binding.enabled && event.enabled;
            binding.skill_id = skill.id.clone();
            binding.enabled = true;
            binding.scope = skill.scope.clone();
            config.skills.insert(skill.id.clone(), skill);
        }
    }
    for event in config.events.values_mut() {
        event.enabled = true;
        event.scope = Scope::default();
    }
    config.schema_version = SCHEMA_VERSION;
    config.validate().map_err(RefineError::InvalidInput)
}

/// Retire configuration only after Skills have been durably installed. Keep exact
/// source text in content-addressed archives, including files reintroduced by an
/// older node. Archives are evidence, never configuration inputs.
pub(crate) fn retire_settings(root: &Path) -> RefineResult<Vec<String>> {
    const SOURCES: [&str; 4] = [
        "governance.json",
        "guidance.json",
        "quality/settings.json",
        "quality/legacy-command-transition.json",
    ];
    if !root.join("automation/config.json").exists()
        || !SOURCES.iter().any(|name| root.join(name).exists())
    {
        return Ok(Vec::new());
    }
    crate::infrastructure::process::supervisor::coordination::with_record_lock(
        root,
        "automation-config",
        || {
            use sha2::{Digest, Sha256};
            // A corrupt or incomplete Skills document must never authorize deletion.
            crate::infrastructure::storage::automation::AutomationStore::new(root).load()?;
            let mut removed = Vec::new();
            for name in SOURCES {
                let path = root.join(name);
                let content = match std::fs::read_to_string(&path) {
                    Ok(content) => content,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(RefineError::Io(error.to_string())),
                };
                let digest = format!("{:x}", Sha256::digest(content.as_bytes()));
                let archive = root
                    .join("automation/retired-settings")
                    .join(name)
                    .join(format!("{digest}.json"));
                let snapshot = json!({"schema_version": 1, "source": name, "content": content});
                if archive.exists() {
                    if read_json::<Value>(&archive)? != snapshot {
                        return Err(RefineError::Conflict(format!(
                            "Retired settings archive differs: {}",
                            archive.display()
                        )));
                    }
                } else {
                    write_json(&archive, &snapshot)?;
                }
                // Do not discard a write made while the archive was being installed.
                if std::fs::read_to_string(&path).map_err(|e| RefineError::Io(e.to_string()))?
                    != content
                {
                    return Err(RefineError::Conflict(format!(
                        "Legacy settings changed during retirement: {name}"
                    )));
                }
                std::fs::remove_file(&path).map_err(|e| RefineError::Io(e.to_string()))?;
                removed.push(name.to_string());
            }
            match std::fs::remove_dir(root.join("quality")) {
                Ok(()) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                    ) => {}
                Err(error) => return Err(RefineError::Io(error.to_string())),
            }
            Ok(removed)
        },
    )
}

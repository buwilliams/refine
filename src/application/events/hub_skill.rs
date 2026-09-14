//! The product Hub keeps one editable, non-removable maintenance Skill.
use crate::model::automation::*;
use std::collections::BTreeMap;
pub const ID: &str = "update-refine-hub";
pub fn install(config: &mut AutomationConfig) {
    if config.skills.contains_key(ID) {
        return;
    }
    config.skills.insert(ID.into(), Skill {
        id: ID.into(), name: "Update Refine Hub".into(),
        prompt: include_str!("skills/update-refine-hub.md").into(),
        role: "task".into(), enabled: true, scope: Scope::default(),
        parameters: vec![Parameter { name: "request".into(), description: "What should Refine Hub explain or update? Include the version and coverage dates for release notes.".into(), kind: ParameterType::Text, required: true, default: None, choices: vec![] }],
        provenance: Some("refine-builtin".into()),
    });
    config
        .events
        .entry(ID.into())
        .or_insert_with(|| {
            let mut event = custom_event();
            event.id = ID.into();
            event.name = "Update Refine Hub".into();
            event
        })
        .bindings
        .push(Binding {
            id: ID.into(),
            skill_id: ID.into(),
            enabled: true,
            mode: BindingMode::Blocking,
            order: 0,
            scope: Scope::default(),
            overrides: None,
            inputs: BTreeMap::new(),
        });
}

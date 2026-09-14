use super::*;
use crate::application::agent_io::prompts::PromptTemplate;
use std::path::PathBuf;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("refine-templates-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn store(&self) -> TemplateStore {
        TemplateStore::new(Some(&self.0))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn defaults_are_visible_without_materializing_state_and_edits_are_revision_fenced() {
    let f = Fixture::new();
    let store = f.store();
    assert_eq!(store.read("workflow").unwrap().revision, 0);
    assert!(!f.0.join("templates").exists());
    let before = store.snapshot().unwrap();
    store.save("workflow", 0, "Changed {{skill}}").unwrap();
    assert!(store.save("workflow", 0, "stale").is_err());
    assert!(store.save("unknown", 0, "new").is_err());
    assert!(
        before.records["workflow"]
            .prompt
            .contains("{{templates.supervised-skill}}")
    );
    assert_eq!(store.read("workflow").unwrap().prompt, "Changed {{skill}}");
    store.save("workflow", 1, "").unwrap();
    assert_eq!(store.read("workflow").unwrap().prompt, "");
}

#[test]
fn nested_skills_expand_but_requests_and_inserted_paths_are_literal() {
    let snapshot = TemplateStore::new(None).snapshot().unwrap();
    let mut values = TemplateScope::literals(&[
        ("refine_executable", "/opt/{{message}}/refine"),
        (
            "current_round_goal",
            "Keep {{refine_executable}} literally in this code",
        ),
    ]);
    values.insert(
        "skill".into(),
        TemplateValue::Template("Run {{refine_executable}}. {{current_round_goal}}".into()),
    );
    let text = snapshot.render_skill("{{skill}}", &values);
    // A Skill referencing itself is rejected, rather than repeatedly expanded.
    assert!(text.is_err());
    let f = Fixture::new();
    f.store().save("workflow", 0, "{{skill}}").unwrap();
    assert_eq!(
        f.store()
            .snapshot()
            .unwrap()
            .render("workflow", &values)
            .unwrap(),
        "Run /opt/{{message}}/refine. Keep {{refine_executable}} literally in this code"
    );
}

#[test]
fn shared_templates_expand_and_cycles_unknowns_and_corruption_are_visible() {
    let f = Fixture::new();
    let store = f.store();
    store.save("workflow", 0, "{{templates.agent}}").unwrap();
    store.save("agent", 0, "Hello {{message}}").unwrap();
    let text = store
        .snapshot()
        .unwrap()
        .render(
            "workflow",
            &TemplateScope::literals(&[("message", "{{skill}}")]),
        )
        .unwrap();
    assert_eq!(text, "Hello {{skill}}");
    assert!(
        store
            .save("agent", 1, "{{templates.workflow}}")
            .unwrap_err()
            .to_string()
            .contains("cycle")
    );
    assert!(store.save("workflow", 1, "{{not_a_variable}}").is_err());
    std::fs::write(f.0.join("templates/workflow.json"), "{invalid").unwrap();
    assert!(store.read("workflow").is_err());
}

#[test]
fn scoped_configuration_is_pinned_restored_and_isolated_between_threads() {
    let f = Fixture::new();
    f.store().save("agent", 0, "first {{message}}").unwrap();
    let snapshot = f.store().snapshot().unwrap();
    let scope = TemplateScope::enter(snapshot.clone());
    f.store().save("agent", 1, "second {{message}}").unwrap();
    assert_eq!(
        crate::application::agent_io::prompts::render(
            PromptTemplate::ChatAgent,
            &[("message", "one")]
        )
        .unwrap(),
        "first one"
    );
    let other = std::thread::spawn(move || {
        let _scope = TemplateScope::enter(snapshot);
        TemplateScope::render("agent", TemplateScope::literals(&[("message", "two")])).unwrap()
    })
    .join()
    .unwrap();
    assert_eq!(other, "first two");
    drop(scope);
    let _scope = TemplateScope::for_root(Some(&f.0)).unwrap();
    assert_eq!(
        TemplateScope::render("agent", TemplateScope::literals(&[("message", "three")])).unwrap(),
        "second three"
    );
}

#[test]
fn literal_placeholder_escape_and_missing_operation_values() {
    let f = Fixture::new();
    f.store()
        .save("agent", 0, r"Keep \{{skill}} and {{current_round_goal}}")
        .unwrap();
    assert_eq!(
        f.store()
            .snapshot()
            .unwrap()
            .render("agent", &Variables::new())
            .unwrap(),
        "Keep {{skill}} and "
    );
    let snapshot = TemplateStore::new(None).snapshot().unwrap();
    assert!(
        snapshot
            .render(&PromptTemplate::ImportNotes.id(), &Variables::new())
            .is_err()
    );
}

#[test]
fn expansion_is_bounded_even_for_an_acyclic_graph() {
    let mut snapshot = TemplateStore::new(None).snapshot().unwrap();
    let ids: Vec<_> = snapshot.records.keys().take(16).cloned().collect();
    for pair in ids.windows(2) {
        snapshot.records.get_mut(&pair[0]).unwrap().prompt =
            format!("{{{{templates.{0}}}}}{{{{templates.{0}}}}}", pair[1]);
    }
    snapshot
        .records
        .get_mut(ids.last().unwrap())
        .unwrap()
        .prompt
        .clear();
    snapshot.validate_references().unwrap();
    assert!(
        snapshot
            .render(&ids[0], &Variables::new())
            .unwrap_err()
            .to_string()
            .contains("4096")
    );
}

#[test]
fn retained_delivery_uses_the_pinned_source_after_an_edit() {
    let f = Fixture::new();
    f.store()
        .save("direct-agent", 0, "Only {{message}}")
        .unwrap();
    let mut metadata = serde_json::Map::new();
    let scope = TemplateScope::pin(Some(&f.0), &mut metadata).unwrap();
    f.store()
        .save("direct-agent", 1, "Changed {{message}}")
        .unwrap();
    drop(scope);
    let _delivery = TemplateScope::for_delivery(None, &mut metadata).unwrap();
    assert_eq!(
        TemplateScope::render(
            "direct-agent",
            TemplateScope::literals(&[("message", "{{skill}}")])
        )
        .unwrap(),
        "Only {{skill}}"
    );
}

#[test]
fn delivery_retains_task_values_within_the_same_pinned_operation() {
    let fixture = Fixture::new();
    fixture
        .store()
        .save("direct-agent", 0, "{{current_round_goal}}")
        .unwrap();
    let mut metadata = serde_json::Map::new();
    let _scope = TemplateScope::pin(Some(&fixture.0), &mut metadata).unwrap();
    TemplateScope::set_values(TemplateScope::literals(&[(
        "current_round_goal",
        "Pinned request",
    )]));
    let _delivery = TemplateScope::for_delivery(None, &mut metadata).unwrap();
    assert_eq!(
        TemplateScope::render("direct-agent", Variables::new()).unwrap(),
        "Pinned request"
    );
}

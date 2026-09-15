use super::*;

#[test]
fn completion_guidance_renders_with_optional_context_and_respects_authored_overrides() {
    let f = Fixture::new();
    let store = f.store();
    let contract =
        crate::application::agent_io::contracts::skill_result::report_contract().to_string();
    let skill = "Plan with stable checklist IDs.";
    let values = TemplateScope::literals(&[
        ("skill", skill),
        ("context", "{}"),
        ("execution", "{}"),
        ("attached_skills", ""),
        ("parameters", "{}"),
        ("continuation", ""),
        ("observational", ""),
        ("hubs", ""),
        ("completion_contract", &contract),
        ("diagnostics", "Invalid summary type"),
        ("raw_output", "Retained output"),
    ]);
    let snapshot = store.snapshot().unwrap();
    for id in ["workflow", "supervised-skill", "skill-repair"] {
        let text = snapshot.render(id, &values).unwrap();
        assert!(text.contains(&contract));
        assert!(text.contains("exactly one final JSON object"));
        assert!(text.contains("no surrounding prose, Markdown fences, or extra top-level fields"));
        assert!(text.contains("plans/checklists in artifacts"));
        assert!(text.contains("optional"));
        if id != "skill-repair" {
            assert!(text.contains(skill));
            assert!(text.contains("omit identity fields"));
        }
    }
    assert!(!f.0.join("templates").exists());
    for id in ["supervised-skill", "skill-repair"] {
        store
            .save(id, 0, "Authored {{completion_contract}}")
            .unwrap();
    }
    let edited = store.snapshot().unwrap();
    for id in ["supervised-skill", "skill-repair"] {
        assert_eq!(
            edited.render(id, &values).unwrap(),
            format!("Authored {contract}")
        );
        assert_eq!(store.read(id).unwrap().revision, 1);
        assert!(
            snapshot
                .render(id, &values)
                .unwrap()
                .contains("exactly one final JSON object")
        );
    }
    assert!(
        edited
            .render("workflow", &values)
            .unwrap()
            .contains(&format!("Authored {contract}"))
    );
}

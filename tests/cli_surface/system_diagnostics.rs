use super::super::*;

pub(crate) fn system_doctor_and_api_groups_run(fixture: &IntegrationFixture) {
    let runtime_root = fixture.runtime_root.display().to_string();
    let repo_root = fixture.app_root.display().to_string();
    let doctor = fixture.run_refine(&[
        "system",
        "doctor",
        "--runtime-root",
        &runtime_root,
        "--repo-root",
        &repo_root,
    ]);
    fixture.assert_success("system doctor", &doctor);
    assert!(fixture.json_stdout(&doctor).is_object());

    let api_groups = fixture.run_refine(&["system", "api-groups"]);
    fixture.assert_success("system api-groups", &api_groups);
    let payload = fixture.json_stdout(&api_groups);
    assert!(
        payload
            .as_array()
            .unwrap()
            .iter()
            .any(|group| group["prefix"].as_str() == Some("/work"))
    );
}

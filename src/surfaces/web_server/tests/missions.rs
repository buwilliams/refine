use serde_json::json;

use crate::model::mission::{MissionIndexProjection, MissionStatus};
use crate::surfaces::web_server::ApiRequest;

use super::*;
fn mission_projection(id: &str, status: MissionStatus) -> MissionIndexProjection {
    MissionIndexProjection {
        id: id.to_string(),
        name: format!("Mission {id}"),
        status,
        reporter: Some("Buddy".to_string()),
        assignee: None,
        coordinator_node_id: None,
        current_round: None,
        current_wave: None,
        criteria_summary: Default::default(),
        outcome_available: false,
        created: "created".to_string(),
        updated: "updated".to_string(),
        json_path: format!("missions/{id}/mission.json"),
    }
}

#[test]
fn missions_list_returns_projected_missions() {
    let mut server = server_with_projection();
    server.projection.missions.insert(
        "MIS1".to_string(),
        mission_projection("MIS1", MissionStatus::Draft),
    );
    let response = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/work/missions".to_string(),
        body: None,
    });
    assert_eq!(response.status, 200);
    let missions = response.body["missions"].as_array().unwrap();
    assert_eq!(missions.len(), 1);
    assert_eq!(missions[0]["id"], "MIS1");
}

#[test]
fn missions_list_filters_by_status() {
    let mut server = server_with_projection();
    server.projection.missions.insert(
        "MIS1".to_string(),
        mission_projection("MIS1", MissionStatus::Draft),
    );
    server.projection.missions.insert(
        "MIS2".to_string(),
        mission_projection("MIS2", MissionStatus::Done),
    );
    let response = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/work/missions?status=done".to_string(),
        body: None,
    });
    assert_eq!(response.status, 200);
    let missions = response.body["missions"].as_array().unwrap();
    assert_eq!(missions.len(), 1);
    assert_eq!(missions[0]["id"], "MIS2");
}

#[test]
fn mission_create_requires_name_and_intent() {
    let temp_root = unique_temp_dir("http-mission-create");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    let response = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/missions".to_string(),
        body: Some(json!({})),
    });
    assert_eq!(response.status, 400);
    assert_eq!(response.body["error"]["code"], "invalid_name");
}

#[test]
fn mission_show_requires_an_id() {
    let temp_root = unique_temp_dir("http-mission-show");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    let response = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/work/missions/".to_string(),
        body: None,
    });
    assert_eq!(response.status, 404);
}

#[test]
fn api_missions_alias_normalizes_to_work_missions() {
    let mut server = server_with_projection();
    server.projection.missions.insert(
        "MIS1".to_string(),
        mission_projection("MIS1", MissionStatus::Draft),
    );
    let response = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/missions".to_string(),
        body: None,
    });
    assert_eq!(response.status, 200);
    assert!(response.body["missions"].is_array());
}

#[test]
fn mission_operations_routes_reach_application_capabilities() {
    let temp_root = unique_temp_dir("http-mission-ops");
    let refine_dir = temp_root.join(".refine");
    std::fs::create_dir_all(&refine_dir).unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(temp_root.clone());

    let create = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/missions".to_string(),
        body: Some(json!({
            "name": "Modernize auth",
            "intent": "modernize it",
            "reporter": "Buddy",
            "id": "MOPS"
        })),
    });
    assert_eq!(create.status, 201, "{:?}", create.body);

    // Transfer validates the target node before changing the coordinator.
    let transfer_missing = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/missions/MOPS/transfer".to_string(),
        body: Some(json!({ "node_id": "missing-node" })),
    });
    assert_eq!(transfer_missing.status, 404, "{:?}", transfer_missing.body);

    // Retry without a recorded stage failure fails closed, not a crash.
    let retry = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/missions/MOPS/retry".to_string(),
        body: Some(json!({ "stage": "quality" })),
    });
    assert!(
        retry.status == 400 || retry.status == 409,
        "{:?}",
        retry.body
    );

    // Answering an unknown decision fails closed before any write.
    let decide = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/missions/MOPS/decisions/dec-1".to_string(),
        body: Some(json!({ "choice": "x" })),
    });
    assert!(
        decide.status == 400 || decide.status == 404,
        "{:?}",
        decide.body
    );

    // The context projection reads back through the alias route.
    let context = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/missions/MOPS/context".to_string(),
        body: None,
    });
    assert_eq!(context.status, 200, "{:?}", context.body);
    assert_eq!(context.body["context"]["mission_id"], "MOPS");

    // Adoption requires a Goal id.
    let adopt = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/missions/MOPS/goals".to_string(),
        body: Some(json!({ "wave": 1 })),
    });
    assert_eq!(adopt.status, 400, "{:?}", adopt.body);
    assert_eq!(adopt.body["error"]["code"], "invalid_goal");

    // Removing an unknown Goal fails closed before any write.
    let remove = server.handle(ApiRequest {
        method: "DELETE".to_string(),
        path: "/work/missions/MOPS/goals/GOAL9".to_string(),
        body: None,
    });
    assert!(
        remove.status == 400 || remove.status == 404,
        "{:?}",
        remove.body
    );
    let _ = std::fs::remove_dir_all(&temp_root);
}

#[test]
fn mission_distribution_route_previews_the_first_wave() {
    let temp_root = unique_temp_dir("http-mission-distribution");
    let refine_dir = temp_root.join(".refine");
    std::fs::create_dir_all(&refine_dir).unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(temp_root.clone());
    let create = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/missions".to_string(),
        body: Some(json!({
            "name": "M",
            "intent": "intent",
            "id": "MDIS"
        })),
    });
    assert_eq!(create.status, 201, "{:?}", create.body);
    // Without an approved plan the preview fails closed with a client error.
    let preview = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/work/missions/MDIS/distribution?wave=1".to_string(),
        body: None,
    });
    assert!(
        preview.status == 400 || preview.status == 409,
        "{:?}",
        preview.body
    );
    let _ = std::fs::remove_dir_all(&temp_root);
}

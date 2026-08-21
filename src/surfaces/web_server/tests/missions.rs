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

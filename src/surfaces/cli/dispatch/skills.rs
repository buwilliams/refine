use super::*;
use std::io::IsTerminal;

pub(super) fn skills(action: SkillAction) -> RefineResult<()> {
    let path = "/skills".to_string();
    let value = match action {
        SkillAction::List { node_id } => daemon_json(
            "GET",
            &format!(
                "{path}{}",
                node_id
                    .map(|id| format!("?node_id={id}"))
                    .unwrap_or_default()
            ),
            None,
        )?,
        SkillAction::Show { id } => daemon_json("GET", &item_path(&path, &id)?, None)?,
        SkillAction::Save {
            id,
            revision,
            payload,
        } => {
            let mut item = config_input::decode_config_input(payload, Default::default(), "Skill")?;
            let triggers = item.as_object_mut().and_then(|item| item.remove("trigger"));
            let mut body = json!({"revision": revision, "item": item});
            if let Some(triggers) = triggers {
                body["trigger"] = triggers;
            }
            daemon_json("PUT", &item_path(&path, &id)?, Some(body))?
        }
        SkillAction::Enable { id } => set_enabled(&path, &id, true)?,
        SkillAction::Disable { id } => set_enabled(&path, &id, false)?,
        SkillAction::Remove { id, revision } => daemon_json(
            "DELETE",
            &item_path(&path, &id)?,
            Some(json!({"revision": revision})),
        )?,
        SkillAction::Clone {
            id,
            new_id,
            name,
            trigger,
        } => {
            let mut current = daemon_json("GET", &item_path(&path, &id)?, None)?;
            current["item"]["id"] = json!(new_id);
            current["item"]["name"] = json!(name.unwrap_or_else(|| format!(
                "{} copy",
                current["item"]["name"].as_str().unwrap_or(&id)
            )));
            if current["trigger"].is_null() {
                current["trigger"] = json!({"source":"custom"});
            }
            current["trigger"].as_object_mut().unwrap().remove("id");
            if let Some(source) = trigger {
                current["trigger"]["source"] = json!(source);
            }
            // The observed global revision fences creation, including concurrent clones.
            current["create_only"] = json!(true);
            daemon_json("PUT", &item_path(&path, &new_id)?, Some(current))?
        }
        SkillAction::Triggers => daemon_json("GET", "/skills/catalog", None)?,
        SkillAction::Trigger {
            id,
            node_id,
            parameters,
            request_id,
        } => {
            let path = item_path("/skills", &id)?;
            let definition = daemon_json(
                "GET",
                &format!(
                    "{path}/inputs{}",
                    node_id
                        .as_ref()
                        .map(|id| format!("?node_id={id}"))
                        .unwrap_or_default()
                ),
                None,
            )?;
            let mut inputs = serde_json::Map::new();
            for parameter in parameters {
                let (name, value) = parameter
                    .split_once('=')
                    .ok_or_else(|| RefineError::InvalidInput("parameters use NAME=VALUE".into()))?;
                let kind = definition["parameters"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|p| p["name"] == name)
                    .and_then(|p| p["kind"].as_str());
                let parsed = if matches!(kind, Some("text" | "choice")) {
                    json!(
                        serde_json::from_str::<String>(value).unwrap_or_else(|_| value.to_string())
                    )
                } else {
                    serde_json::from_str(value).unwrap_or_else(|_| json!(value))
                };
                inputs.insert(name.into(), parsed);
            }
            if std::io::stdin().is_terminal() {
                for parameter in definition["parameters"].as_array().into_iter().flatten() {
                    let Some(name) = parameter["name"].as_str() else {
                        continue;
                    };
                    if inputs.contains_key(name)
                        || parameter["required"] != true
                        || !parameter["default"].is_null()
                    {
                        continue;
                    }
                    eprint!("{name}: ");
                    std::io::stderr()
                        .flush()
                        .map_err(|e| RefineError::Io(e.to_string()))?;
                    let mut value = String::new();
                    std::io::stdin()
                        .read_line(&mut value)
                        .map_err(|e| RefineError::Io(e.to_string()))?;
                    inputs.insert(
                        name.into(),
                        if parameter["kind"] == "number" || parameter["kind"] == "boolean" {
                            serde_json::from_str(value.trim())
                                .map_err(|e| RefineError::InvalidInput(e.to_string()))?
                        } else {
                            json!(value.trim_end())
                        },
                    );
                }
            }
            daemon_json(
                "POST",
                &format!("{path}/trigger"),
                Some(
                    json!({"node_id": node_id, "parameters": inputs, "request_id": request_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string())}),
                ),
            )?
        }
        SkillAction::Runs { offset, limit } => daemon_json(
            "GET",
            &format!("/event-invocations?offset={offset}&limit={limit}"),
            None,
        )?,
        SkillAction::Status { id } => {
            daemon_json("GET", &item_path("/event-invocations", &id)?, None)?
        }
        SkillAction::Cancel { id } => daemon_json(
            "POST",
            &format!("{}/cancel", item_path("/event-invocations", &id)?),
            Some(json!({})),
        )?,
    };
    print_json(&value);
    Ok(())
}

fn item_path(path: &str, id: &str) -> RefineResult<String> {
    if !crate::model::automation::valid_id(id) {
        return Err(RefineError::InvalidInput("invalid ID".into()));
    }
    Ok(format!("{path}/{id}"))
}
fn set_enabled(path: &str, id: &str, enabled: bool) -> RefineResult<Value> {
    let path = item_path(path, id)?;
    let mut current = daemon_json("GET", &path, None)?;
    current["item"]["enabled"] = json!(enabled);
    daemon_json("PUT", &path, Some(current))
}

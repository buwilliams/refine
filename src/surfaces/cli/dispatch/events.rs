use super::*;
use std::io::IsTerminal;

pub(super) fn definitions(collection: &str, action: DefinitionAction) -> RefineResult<()> {
    let path = format!("/{collection}");
    let value = match action {
        DefinitionAction::List { node_id } => daemon_json(
            "GET",
            &format!(
                "{path}{}",
                node_id
                    .map(|id| format!("?node_id={id}"))
                    .unwrap_or_default()
            ),
            None,
        )?,
        DefinitionAction::Show { id } => daemon_json("GET", &item_path(&path, &id)?, None)?,
        DefinitionAction::Save {
            id,
            revision,
            payload,
        } => {
            let item =
                config_input::decode_config_input(payload, Default::default(), "definition")?;
            daemon_json(
                "PUT",
                &item_path(&path, &id)?,
                Some(json!({"revision": revision, "item": item})),
            )?
        }
        DefinitionAction::Enable { id } => set_enabled(&path, &id, true)?,
        DefinitionAction::Disable { id } => set_enabled(&path, &id, false)?,
        DefinitionAction::Remove { id, revision } => daemon_json(
            "DELETE",
            &item_path(&path, &id)?,
            Some(json!({"revision": revision})),
        )?,
    };
    print_json(&value);
    Ok(())
}

pub(super) fn events(action: EventAction) -> RefineResult<()> {
    let value = match action {
        EventAction::Catalog => daemon_json("GET", "/event-definitions/catalog", None)?,
        EventAction::List { node_id } => {
            return definitions("event-definitions", DefinitionAction::List { node_id });
        }
        EventAction::Show { id } => {
            return definitions("event-definitions", DefinitionAction::Show { id });
        }
        EventAction::Save {
            id,
            revision,
            payload,
        } => {
            return definitions(
                "event-definitions",
                DefinitionAction::Save {
                    id,
                    revision,
                    payload,
                },
            );
        }
        EventAction::Enable { id } => {
            return definitions("event-definitions", DefinitionAction::Enable { id });
        }
        EventAction::Disable { id } => {
            return definitions("event-definitions", DefinitionAction::Disable { id });
        }
        EventAction::Remove { id, revision } => {
            return definitions(
                "event-definitions",
                DefinitionAction::Remove { id, revision },
            );
        }
        EventAction::Bind {
            id,
            revision,
            payload,
        } => {
            let path = item_path("/event-definitions", &id)?;
            let mut current = daemon_json("GET", &path, None)?;
            if current["revision"].as_u64() != Some(revision) {
                return Err(RefineError::Conflict(
                    "Event changed; refresh before editing bindings".into(),
                ));
            }
            let binding =
                config_input::decode_config_input(payload, Default::default(), "binding")?;
            let binding_id = binding
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| RefineError::InvalidInput("binding ID is required".into()))?;
            let bindings = current["item"]["bindings"].as_array_mut().ok_or_else(|| {
                RefineError::Serialization("Event bindings must be an array".into())
            })?;
            bindings.retain(|b| b["id"].as_str() != Some(binding_id));
            bindings.push(binding);
            daemon_json("PUT", &path, Some(current))?
        }
        EventAction::Unbind {
            id,
            binding_id,
            revision,
        } => {
            let path = item_path("/event-definitions", &id)?;
            let mut current = daemon_json("GET", &path, None)?;
            if current["revision"].as_u64() != Some(revision) {
                return Err(RefineError::Conflict(
                    "Event changed; refresh before editing bindings".into(),
                ));
            }
            current["item"]["bindings"]
                .as_array_mut()
                .ok_or_else(|| RefineError::Serialization("invalid bindings".into()))?
                .retain(|b| b["id"].as_str() != Some(&binding_id));
            daemon_json("PUT", &path, Some(current))?
        }
        EventAction::Trigger {
            id,
            goal_id,
            node_id,
            parameters,
            request_id,
        } => {
            let path = item_path("/event-definitions", &id)?;
            let definition = daemon_json(
                "GET",
                &format!(
                    "{path}/inputs{}",
                    goal_id
                        .as_ref()
                        .map(|id| format!("?goal_id={id}"))
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
                    json!({"goal_id": goal_id, "node_id": node_id, "parameters": inputs, "request_id": request_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string())}),
                ),
            )?
        }
        EventAction::Runs { offset, limit } => daemon_json(
            "GET",
            &format!("/event-invocations?offset={offset}&limit={limit}"),
            None,
        )?,
        EventAction::Status { id } => {
            daemon_json("GET", &item_path("/event-invocations", &id)?, None)?
        }
        EventAction::Cancel { id } => daemon_json(
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

use super::*;

pub(super) fn templates(action: TemplateAction) -> RefineResult<()> {
    let path = |id: &str| -> RefineResult<String> {
        if crate::application::templates::definition(id).is_none() {
            return Err(RefineError::NotFound(format!("Template {id}")));
        }
        Ok(format!("/templates/{id}"))
    };
    let value = match action {
        TemplateAction::List => daemon_json("GET", "/templates", None)?,
        TemplateAction::Show { id } => daemon_json("GET", &path(&id)?, None)?,
        TemplateAction::Variables => daemon_json("GET", "/templates/variables", None)?,
        TemplateAction::Save {
            id,
            revision,
            payload,
        } => {
            let mut body =
                config_input::decode_config_input(payload, Default::default(), "Template")?;
            body["revision"] = json!(revision);
            daemon_json("PUT", &path(&id)?, Some(body))?
        }
        TemplateAction::Preview { id, payload } => {
            let body =
                config_input::decode_config_input(payload, Default::default(), "Template preview")?;
            daemon_json("POST", &format!("{}/preview", path(&id)?), Some(body))?
        }
    };
    print_json(&value);
    Ok(())
}

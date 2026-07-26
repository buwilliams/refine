use super::*;

impl FileProjectStateStore {
    pub(super) fn project_feature(
        &self,
        path: &Path,
    ) -> RefineResult<Option<FeatureIndexProjection>> {
        let value = Self::read_json(path)?;
        let Some(object) = value.as_object() else {
            return Ok(None);
        };
        let id = text(object.get("id")).unwrap_or_default();
        if id.is_empty() {
            return Ok(None);
        }
        Ok(Some(FeatureIndexProjection {
            id,
            name: text(object.get("name")).unwrap_or_else(|| "Untitled Feature".to_string()),
            description: Some(text(object.get("description")).unwrap_or_default()),
            reporter: Some(text(object.get("reporter")).unwrap_or_default()),
            assignee: nullable_text(object.get("assignee"))
                .or_else(|| text(object.get("reporter")))
                .filter(|assignee| !assignee.is_empty()),
            node_id: Some(
                nullable_text(object.get("node_id")).unwrap_or_else(|| "default".to_string()),
            ),
            created: text(object.get("created")).unwrap_or_else(|| "unknown".to_string()),
            updated: text(object.get("updated"))
                .or_else(|| text(object.get("created")))
                .unwrap_or_else(|| "unknown".to_string()),
            json_path: self.relative_path(path)?,
        }))
    }
}

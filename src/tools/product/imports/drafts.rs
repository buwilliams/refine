use super::*;
use crate::tools::product::work_items::FileWorkItemService;

pub fn import_drafts_from_value(
    body: &serde_json::Value,
    default_reporter: Option<&str>,
) -> RefineResult<Vec<ImportDraft>> {
    let default_reporter = body
        .get("reporter")
        .and_then(|value| value.as_str())
        .or(default_reporter)
        .unwrap_or("")
        .trim();
    let drafts = body
        .get("drafts")
        .or_else(|| body.get("items"))
        .unwrap_or(body);
    let Some(drafts) = drafts.as_array() else {
        return Err(RefineError::InvalidInput(
            "body.drafts must be an array".to_string(),
        ));
    };
    drafts
        .iter()
        .enumerate()
        .map(|(index, value)| import_draft_from_value(value, default_reporter, index + 1))
        .collect()
}

pub(super) fn import_draft_from_value(
    value: &serde_json::Value,
    default_reporter: &str,
    index: usize,
) -> RefineResult<ImportDraft> {
    let Some(object) = value.as_object() else {
        return Err(RefineError::InvalidInput(format!(
            "draft {index} must be an object"
        )));
    };
    let field = |key: &str| -> &str { string_field(object, &[key]) };
    let prompt = field("prompt").to_string();
    let priority = normalized_priority(field("priority")).map_err(|_| {
        RefineError::InvalidInput(format!(
            "draft {index} priority must be one of low, medium, or high"
        ))
    })?;
    let reporter = nonempty_or(field("reporter"), default_reporter).to_string();
    let assignee = nonempty_or(field("assignee"), &reporter).to_string();
    Ok(ImportDraft {
        name: import_name(string_field(object, &["name", "title", "summary"]), &prompt),
        prompt,
        reporter,
        assignee: (!assignee.is_empty()).then_some(assignee),
        priority,
        duplicate_decision: field("duplicate_decision").to_string(),
        dependency_names: string_list_field(
            object,
            &[
                "dependency_names",
                "depends_on",
                "dependencies",
                "after",
                "requires",
            ],
        ),
    })
}

pub fn order_feature_dependency_drafts(
    work_items: &FileWorkItemService,
    feature_id: &str,
    created_drafts: &[(ImportDraft, String)],
) -> RefineResult<()> {
    let ordered_goal_ids = dependency_ordered_goal_ids(created_drafts);
    if !ordered_goal_ids.is_empty() {
        work_items.order_goals_in_feature(feature_id, &ordered_goal_ids)?;
    }
    Ok(())
}

pub(super) fn dependency_ordered_goal_ids(created_drafts: &[(ImportDraft, String)]) -> Vec<String> {
    let mut name_to_goal_id = BTreeMap::new();
    let mut position_by_goal_id = BTreeMap::new();
    for (index, (draft, goal_id)) in created_drafts.iter().enumerate() {
        position_by_goal_id.insert(goal_id.clone(), index);
        for key in [&draft.name, goal_id] {
            let key = normalize_dependency_key(key);
            if !key.is_empty() {
                name_to_goal_id.insert(key, goal_id.clone());
            }
        }
    }

    let mut edges: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut involved = BTreeSet::new();
    for (draft, goal_id) in created_drafts {
        for dependency in &draft.dependency_names {
            let dependency_key = normalize_dependency_key(dependency);
            let Some(prerequisite_id) = name_to_goal_id.get(&dependency_key) else {
                continue;
            };
            if prerequisite_id == goal_id {
                continue;
            }
            edges
                .entry(prerequisite_id.clone())
                .or_default()
                .insert(goal_id.clone());
            involved.insert(prerequisite_id.clone());
            involved.insert(goal_id.clone());
        }
    }
    if involved.is_empty() {
        return Vec::new();
    }

    let mut incoming: BTreeMap<String, usize> = involved
        .iter()
        .map(|goal_id| (goal_id.clone(), 0usize))
        .collect();
    for dependents in edges.values() {
        for dependent in dependents {
            if let Some(count) = incoming.get_mut(dependent) {
                *count += 1;
            }
        }
    }

    let mut ordered = Vec::new();
    while let Some(next_id) = incoming
        .iter()
        .filter(|(_, count)| **count == 0)
        .min_by_key(|(goal_id, _)| {
            position_by_goal_id
                .get(*goal_id)
                .copied()
                .unwrap_or(usize::MAX)
        })
        .map(|(goal_id, _)| goal_id.clone())
    {
        incoming.remove(&next_id);
        ordered.push(next_id.clone());
        if let Some(dependents) = edges.get(&next_id) {
            for dependent in dependents {
                if let Some(count) = incoming.get_mut(dependent) {
                    *count = count.saturating_sub(1);
                }
            }
        }
    }

    if !incoming.is_empty() {
        let mut fallback = involved.into_iter().collect::<Vec<_>>();
        fallback.sort_by_key(|goal_id| {
            position_by_goal_id
                .get(goal_id)
                .copied()
                .unwrap_or(usize::MAX)
        });
        return fallback;
    }
    ordered
}

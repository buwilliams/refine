use super::*;

impl FileWorkItemService {
    pub fn feature_goal_authoring_capability(
        goal: &GoalSummaryProjection,
    ) -> FeatureGoalAuthoringCapability {
        let operations = [
            GoalOperation::EditMetadata,
            if goal.goal.round_count == 0 {
                GoalOperation::SubmitNewRound
            } else {
                GoalOperation::EditLatestRound
            },
            GoalOperation::ReorderInFeature,
        ];
        let denied = operations
            .iter()
            .map(|operation| goal_operation_allowed(&goal.goal.status, operation))
            .find(|decision| !decision.allowed);
        FeatureGoalAuthoringCapability {
            editable: denied.is_none(),
            reason: denied.and_then(|decision| decision.reason),
        }
    }

    pub fn author_feature_goal(
        &self,
        feature_id: &str,
        request: FeatureGoalAuthoringRequest,
    ) -> RefineResult<FeatureGoalAuthoringResult> {
        let snapshot = self.projection_snapshot()?;
        self.author_goal_with_context(
            GoalAuthoringRequest {
                goal_id: request.goal_id,
                name: request.name,
                prompt: request.prompt,
                reporter: request.reporter,
                assignee: request.assignee,
                priority: request.priority,
                feature_id: Some(feature_id.to_string()),
                placement: request.placement,
                duplicate_decision: request.duplicate_decision,
                ..GoalAuthoringRequest::default()
            },
            true,
            Some(&snapshot),
            false,
        )
    }

    pub fn author_goal(&self, request: GoalAuthoringRequest) -> RefineResult<GoalAuthoringResult> {
        let snapshot = self.projection_snapshot()?;
        self.author_goal_with_context(request, false, Some(&snapshot), false)
    }

    /// Authors a new Goal from a caller-validated coherent projection. The web
    /// create route uses this with its process-hot snapshot so duplicate
    /// detection and validation do not independently reload project state. New
    /// Goal persistence is collapsed into one atomic record write; the caller
    /// owns the single projection refresh after backlog promotion.
    pub fn author_goal_from_projection(
        &self,
        request: GoalAuthoringRequest,
        snapshot: &ProjectionSnapshot,
    ) -> RefineResult<GoalAuthoringResult> {
        self.author_goal_with_context(request, false, Some(snapshot), true)
    }

    pub(super) fn author_goal_with_context(
        &self,
        request: GoalAuthoringRequest,
        feature_inline: bool,
        snapshot: Option<&ProjectionSnapshot>,
        direct_create: bool,
    ) -> RefineResult<GoalAuthoringResult> {
        let goal_id = request
            .goal_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let id = request
            .id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if goal_id.is_some() && id.is_some() {
            return Err(RefineError::InvalidInput(
                "id cannot be supplied when editing a Goal".to_string(),
            ));
        }
        let prompt = request.prompt.trim();
        let resolved_name = request
            .name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| derive_goal_name(prompt));
        if goal_id.is_none() && resolved_name.is_none() {
            return Err(RefineError::InvalidInput(
                "Goal name is required".to_string(),
            ));
        }
        let feature_id = request
            .feature_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if let Some(feature_id) = feature_id {
            let feature = match snapshot {
                Some(snapshot) => snapshot.features.get(feature_id).cloned().ok_or_else(|| {
                    RefineError::NotFound(format!(
                        "Feature {feature_id} was not found in refine state"
                    ))
                })?,
                None => self.show_feature_summary(feature_id)?,
            };
            self.ensure_feature_owned(&feature)?;
        }

        let reporter = Self::validate_goal_reporter(&request.reporter)?;
        if feature_inline && (reporter.is_empty() || prompt.is_empty()) {
            return Err(RefineError::InvalidInput(
                "reporter and prompt are required".to_string(),
            ));
        }
        let priority = request.priority.trim();
        if GoalPriority::parse_wire(priority).is_none() {
            return Err(RefineError::InvalidInput(
                "priority must be one of low, medium, or high".to_string(),
            ));
        }
        let assignee = request
            .assignee
            .as_deref()
            .map(Self::validate_goal_assignee)
            .transpose()?
            .filter(|value| !value.is_empty());

        let current = goal_id
            .map(|goal_id| match snapshot {
                Some(snapshot) => snapshot.goals.get(goal_id).cloned().ok_or_else(|| {
                    RefineError::NotFound(format!("Goal {goal_id} was not found in refine state"))
                }),
                None => self.show_goal_summary(goal_id),
            })
            .transpose()?;
        if let Some(current) = &current {
            self.ensure_goal_owned(current)?;
            if current.goal.feature_id.as_deref() != feature_id {
                return Err(RefineError::Conflict(format!(
                    "Goal {} is not assigned to Feature {}",
                    current.goal.id,
                    feature_id.unwrap_or("")
                )));
            }
            let capability = Self::feature_goal_authoring_capability(current);
            if !capability.editable {
                return Err(RefineError::InvalidInput(capability.reason.unwrap_or_else(
                    || "Goal cannot be authored in its current status".to_string(),
                )));
            }
            if request
                .name
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .is_none()
            {
                return Err(RefineError::InvalidInput(
                    "Goal name is required when editing".to_string(),
                ));
            }
        }
        if let Some(feature_id) = feature_id {
            self.validate_feature_goal_placement(feature_id, goal_id, &request.placement)?;
        } else if !matches!(request.placement, FeatureGoalPlacement::Unordered) {
            return Err(RefineError::InvalidInput(
                "Goal placement requires a Feature".to_string(),
            ));
        }

        if current.is_none()
            && id.is_none()
            && let Some(duplicate) = match snapshot {
                Some(snapshot) => Self::latest_round_duplicate_in_snapshot(snapshot, prompt),
                None => self.latest_round_duplicate(prompt)?,
            }
        {
            match request.duplicate_decision.trim() {
                "" => {
                    return Ok(GoalAuthoringResult {
                        created: false,
                        goal: None,
                        duplicate_action: None,
                        duplicate: Some(duplicate),
                        move_result: None,
                        requires_duplicate_decision: true,
                    });
                }
                "duplicate" => {
                    return Ok(GoalAuthoringResult {
                        created: false,
                        goal: None,
                        duplicate_action: Some("duplicate".to_string()),
                        duplicate: Some(duplicate),
                        move_result: None,
                        requires_duplicate_decision: false,
                    });
                }
                "move_original_to_backlog" => {
                    let from = duplicate.status.clone();
                    let move_result = if from == GoalStatus::Backlog {
                        GoalAuthoringDuplicateMove {
                            moved: false,
                            from,
                            to: GoalStatus::Backlog,
                            reason: Some("already_backlog".to_string()),
                        }
                    } else if match (direct_create, snapshot) {
                        (true, Some(snapshot)) => snapshot
                            .goals
                            .get(&duplicate.id)
                            .ok_or_else(|| {
                                RefineError::NotFound(format!(
                                    "Goal {} was not found in refine state",
                                    duplicate.id
                                ))
                            })
                            .and_then(|goal| {
                                self.transition_goal_status_from_projection(
                                    &goal.goal,
                                    GoalStatus::Backlog,
                                )
                            }),
                        _ => self
                            .transition_goal_status(&duplicate.id, GoalStatus::Backlog)
                            .map(|_| ()),
                    }
                    .is_ok()
                    {
                        GoalAuthoringDuplicateMove {
                            moved: true,
                            from,
                            to: GoalStatus::Backlog,
                            reason: None,
                        }
                    } else {
                        GoalAuthoringDuplicateMove {
                            moved: false,
                            from,
                            to: GoalStatus::Backlog,
                            reason: Some("protected_status".to_string()),
                        }
                    };
                    return Ok(GoalAuthoringResult {
                        created: false,
                        goal: None,
                        duplicate_action: Some("move_original_to_backlog".to_string()),
                        duplicate: Some(duplicate),
                        move_result: Some(move_result),
                        requires_duplicate_decision: false,
                    });
                }
                "original" => {}
                other => {
                    return Err(RefineError::InvalidInput(format!(
                        "unknown duplicate_decision: {other}"
                    )));
                }
            }
        }

        let saved = if let Some(current) = current {
            let goal_id = current.goal.id.clone();
            self.update_goal_metadata_summary(
                &goal_id,
                request.name.as_deref(),
                Some(priority),
                Some(reporter),
                None,
            )?;
            let assignee = assignee
                .or(current.goal.assignee.as_deref())
                .unwrap_or(reporter);
            if current.goal.round_count == 0 {
                self.append_goal_round_summary_with_assignee(
                    &goal_id,
                    reporter,
                    Some(assignee),
                    prompt,
                )?;
            } else {
                self.edit_latest_goal_round_summary(
                    &goal_id,
                    Some(reporter),
                    Some(assignee),
                    Some(prompt),
                )?;
            }
            if let Some(feature_id) = feature_id {
                self.apply_feature_goal_placement(feature_id, &goal_id, &request.placement)?;
            }
            self.show_goal_summary(&goal_id)?.goal
        } else {
            let name = resolved_name
                .ok_or_else(|| RefineError::InvalidInput("Goal name is required".to_string()))?;
            if direct_create {
                self.create_authored_goal(
                    &name, id, priority, reporter, assignee, prompt, feature_id,
                )?
                .goal
            } else {
                let goal = self.create_goal_summary(&name, id)?;
                self.update_goal_metadata_summary(
                    &goal.goal.id,
                    None,
                    (priority != "low").then_some(priority),
                    Some(reporter),
                    None,
                )?;
                if !reporter.is_empty() && !prompt.is_empty() {
                    self.append_goal_round_summary_with_assignee(
                        &goal.goal.id,
                        reporter,
                        Some(assignee.unwrap_or(reporter)),
                        prompt,
                    )?;
                }
                if let Some(feature_id) = feature_id {
                    self.assign_goal_to_feature(feature_id, &goal.goal.id)?;
                    self.apply_feature_goal_placement(
                        feature_id,
                        &goal.goal.id,
                        &request.placement,
                    )?;
                }
                self.show_goal_summary(&goal.goal.id)?.goal
            }
        };

        Ok(GoalAuthoringResult {
            created: goal_id.is_none(),
            goal: Some(saved),
            duplicate_action: None,
            duplicate: None,
            move_result: None,
            requires_duplicate_decision: false,
        })
    }

    pub fn latest_round_duplicate(
        &self,
        prompt: &str,
    ) -> RefineResult<Option<GoalAuthoringDuplicate>> {
        if prompt.is_empty() {
            return Ok(None);
        }
        let snapshot = self.projection_snapshot()?;
        Ok(Self::latest_round_duplicate_in_snapshot(&snapshot, prompt))
    }

    pub(super) fn latest_round_duplicate_in_snapshot(
        snapshot: &ProjectionSnapshot,
        prompt: &str,
    ) -> Option<GoalAuthoringDuplicate> {
        if prompt.is_empty() {
            return None;
        }
        for goal in snapshot.goals.values() {
            if goal.latest_round_prompt.as_deref() == Some(prompt) {
                return Some(GoalAuthoringDuplicate {
                    id: goal.goal.id.clone(),
                    name: goal.goal.name.clone(),
                    status: goal.goal.status.clone(),
                    node_id: goal.goal.node_id.clone(),
                    node_display_name: goal.node_display_name.clone(),
                    prompt: prompt.to_string(),
                });
            }
        }
        None
    }

    // Authoring keeps the public Goal fields explicit at the persistence edge.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn create_authored_goal(
        &self,
        name: &str,
        id: Option<&str>,
        priority: &str,
        reporter: &str,
        assignee: Option<&str>,
        prompt: &str,
        feature_id: Option<&str>,
    ) -> RefineResult<GoalSummaryProjection> {
        let goal_id = id
            .map(|id| id.trim().to_uppercase())
            .filter(|id| !id.is_empty())
            .unwrap_or_else(new_ulid_like);
        if goal_id.len() < 3 {
            return Err(RefineError::InvalidInput(
                "Goal id must be at least three characters".to_string(),
            ));
        }
        let goal_path = goal_json_path(&self.refine_dir, &goal_id);
        if goal_path.exists() {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} already exists"
            )));
        }

        let node_id = self.active_node_id()?;
        let now = now_timestamp();
        let round_assignee = assignee.unwrap_or(reporter);
        let latest_round_prompt =
            (!reporter.is_empty() && !prompt.is_empty()).then(|| prompt.to_string());
        let rounds = latest_round_prompt
            .as_ref()
            .map(|_| vec![new_round_value(reporter, round_assignee, prompt)])
            .unwrap_or_default();
        let mut object = Map::new();
        object.insert("id".to_string(), Value::String(goal_id.clone()));
        object.insert("name".to_string(), Value::String(name.to_string()));
        object.insert("status".to_string(), Value::String("backlog".to_string()));
        object.insert("priority".to_string(), Value::String(priority.to_string()));
        object.insert(
            "reporter".to_string(),
            if reporter.is_empty() {
                Value::Null
            } else {
                Value::String(reporter.to_string())
            },
        );
        object.insert("branch_name".to_string(), Value::Null);
        object.insert("target_branch".to_string(), Value::Null);
        object.insert("base_commit".to_string(), Value::Null);
        object.insert("candidate_commit".to_string(), Value::Null);
        object.insert(
            "feature_id".to_string(),
            feature_id
                .map(|feature_id| Value::String(feature_id.to_string()))
                .unwrap_or(Value::Null),
        );
        object.insert("feature_order".to_string(), Value::Null);
        object.insert("node_id".to_string(), Value::String(node_id.clone()));
        object.insert("created".to_string(), Value::String(now.clone()));
        object.insert("updated".to_string(), Value::String(now.clone()));
        object.insert("notes".to_string(), Value::Array(Vec::new()));
        object.insert("rounds".to_string(), Value::Array(rounds));
        write_json_atomically(&goal_path, &Value::Object(object))?;

        let json_path = goal_path
            .strip_prefix(&self.refine_dir)
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .map_err(|error| {
                RefineError::InvalidInput(format!(
                    "Goal path {} is not under refine dir {}: {error}",
                    goal_path.display(),
                    self.refine_dir.display()
                ))
            })?;
        let priority = GoalPriority::parse_wire(priority).ok_or_else(|| {
            RefineError::InvalidInput("priority must be one of low, medium, or high".to_string())
        })?;
        let round_count = usize::from(latest_round_prompt.is_some());
        let assignee = (round_count > 0).then(|| round_assignee.to_string());
        let mut searchable_parts = vec![name.to_string(), reporter.to_string()];
        if let Some(assignee) = &assignee {
            searchable_parts.push(assignee.clone());
        }
        if let Some(prompt) = &latest_round_prompt {
            searchable_parts.push(prompt.clone());
        }
        Ok(GoalSummaryProjection {
            goal: GoalIndexProjection {
                id: goal_id,
                name: name.to_string(),
                status: GoalStatus::Backlog,
                priority,
                reporter: (!reporter.is_empty()).then(|| reporter.to_string()),
                assignee,
                round_count,
                created: now.clone(),
                updated: now,
                branch_name: None,
                node_id: Some(node_id),
                feature_id: feature_id.map(str::to_string),
                feature_order: None,
                json_path,
            },
            node_display_name: None,
            latest_round_prompt,
            searchable_text: searchable_parts.join("\n"),
            activity_ids: Vec::new(),
        })
    }
}

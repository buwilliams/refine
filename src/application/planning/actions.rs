use super::*;
use crate::application::events::{FileEventService, InvocationState};
use crate::model::automation::BindingMode;
impl FilePlanningService {
    pub(super) fn advance(&self, a: &mut PlanningAction) -> RefineResult<()> {
        if !a.command.operation.starts_with("card.") {
            return self.advance_locked(a);
        }
        let mut boards = Vec::new();
        boards.extend(a.command.board_id.iter().cloned());
        boards.extend(a.destination.iter().map(|p| p.board_id.clone()));
        boards.extend(a.before.iter().map(|p| p.board_id.clone()));
        if let Some(goal) = &a.command.goal_id {
            let path = self.path("cards", goal)?;
            if path.exists() {
                boards.push(read_json::<Placement>(&path)?.board_id);
            }
        }
        boards.sort();
        boards.dedup();
        self.with_board_locks(&boards, || {
            if let Some(board) = a
                .destination
                .as_ref()
                .map(|p| &p.board_id)
                .or(a.command.board_id.as_ref())
            {
                self.board(board)?;
            }
            self.advance_locked(a)
        })
    }
    fn with_board_locks<T>(
        &self,
        boards: &[String],
        operation: impl FnOnce() -> RefineResult<T>,
    ) -> RefineResult<T> {
        if let Some((board, rest)) = boards.split_first() {
            with_record_lock(&self.root, &format!("planning-board-{board}"), || {
                self.with_board_locks(rest, operation)
            })
        } else {
            operation()
        }
    }
    fn advance_locked(&self, a: &mut PlanningAction) -> RefineResult<()> {
        if let Some(report) = crate::application::persistence_sync::conflict_reports::latest_state_sync_conflict_report(&self.runtime)? {
            let target = self.target.canonicalize().unwrap_or_else(|_| self.target.clone());
            if report.target_identity == target.to_string_lossy()
                && report.unresolved_paths.iter().any(|path| path.trim_start_matches(".refine/").starts_with("planning/"))
            {
                return Err(waiting("Resolve the shared Project Planning state conflict before processing actions"));
            }
        }
        if a.command.operation == "migrate" {
            return self.migrate(a);
        }
        if a.command.operation.starts_with("board.") || a.command.operation.starts_with("lane.") {
            return self.mutate_board(a);
        }
        let goal_id = a
            .command
            .goal_id
            .clone()
            .unwrap_or_else(|| format!("PLN{}", stable_id(&a.id)[..24].to_uppercase()));
        let work = self.work();
        // Explicitly removing an orphaned placement must remain possible after
        // a Goal was deleted through another surface. Unreadable Goals still fail closed.
        if a.command.operation == "card.detach"
            && matches!(
                work.show_goal_detail(&goal_id),
                Err(RefineError::NotFound(_))
            )
        {
            let path = self.path("cards", &goal_id)?;
            if a.phase == "prepare" {
                revision(
                    a.command.expected_revision,
                    self.placement(&goal_id)?.revision,
                )?;
                a.phase = "detach".into();
                self.save_action(a)?;
            }
            if path.exists() {
                fs::remove_file(path).map_err(io)?;
            }
            a.state = "complete".into();
            a.result = json!({"goal_id":goal_id});
            return Ok(());
        }
        for _ in 0..12 {
            a.state = "running".into();
            a.message = None;
            if a.phase == "prepare" {
                let pinned = (*FileEventService::with_runtime_root(&self.root, &self.runtime)
                    .config()?)
                .clone();
                let path = self.path("cards", &goal_id)?;
                let previous_revision = if path.exists() {
                    read_json::<Placement>(&path)?.revision
                } else {
                    0
                };
                let mut prior = if path.exists() {
                    match self.placement(&goal_id) {
                        Ok(placement) => Some(placement),
                        Err(RefineError::NotFound(_)) if a.command.operation == "card.attach" => {
                            None
                        }
                        Err(error) => return Err(error),
                    }
                } else {
                    None
                };
                if !matches!(a.command.operation.as_str(), "card.create" | "card.attach") {
                    revision(
                        a.command.expected_revision,
                        prior
                            .as_ref()
                            .ok_or_else(|| invalid("Card is not on a board"))?
                            .revision,
                    )?;
                } else if prior.is_some() && prior.as_ref().unwrap().last_request_id != a.id {
                    return Err(conflict("Goal is already on a board"));
                }
                if a.command.operation == "card.create" {
                    let board = self.board(
                        a.command
                            .board_id
                            .as_deref()
                            .ok_or_else(|| invalid("board_id is required"))?,
                    )?;
                    let lane = board
                        .lanes
                        .iter()
                        .find(|l| Some(&l.id) == a.command.lane_id.as_ref())
                        .ok_or_else(|| invalid("Lane does not belong to board"))?;
                    if board.archived {
                        return Err(conflict("Board is archived"));
                    }
                    let existing = work.show_goal_detail(&goal_id);
                    if existing.is_ok() && prior.as_ref().is_none_or(|p| p.last_request_id != a.id)
                    {
                        return Err(conflict("Goal already exists; attach it instead"));
                    }
                    let name = text(&a.command.data, "name")?;
                    let placement = Placement {
                        goal_id: goal_id.clone(),
                        board_id: board.id,
                        lane_id: lane.id.clone(),
                        position: a.command.data["position"]
                            .as_f64()
                            .unwrap_or_else(|| chrono::Utc::now().timestamp_millis() as f64),
                        revision: 1,
                        archived: false,
                        routing: route(&a.command.data)?,
                        last_request_id: a.id.clone(),
                    };
                    write_json(&path, &placement)?;
                    match existing {
                        Err(RefineError::NotFound(_)) => {
                            if let Err(error) = work.create_goal_in_step(
                                &name,
                                Some(&goal_id),
                                GoalStatus::Draft,
                                a.command.data["description"].as_str(),
                                a.command.data["reporter"].as_str(),
                                a.command.data["priority"].as_str(),
                            ) {
                                if work.show_goal_detail(&goal_id).is_err() {
                                    let _ = fs::remove_file(&path);
                                }
                                return Err(error);
                            }
                        }
                        Err(error) => return Err(error),
                        Ok(_) => {}
                    }
                    prior = None;
                }
                let goal = work.show_goal_detail(&goal_id)?;
                if goal["node_id"] != self.node {
                    return Err(waiting("The current Goal node must process this action"));
                }
                if a.command.operation == "card.detach" {
                    a.phase = "detach".into();
                    self.save_action(a)?;
                    continue;
                }
                let board_id = a
                    .command
                    .board_id
                    .as_deref()
                    .or(prior.as_ref().map(|p| p.board_id.as_str()))
                    .ok_or_else(|| invalid("board_id is required"))?;
                let board = self.board(board_id)?;
                if board.archived {
                    return Err(conflict("Board is archived"));
                }
                let lane_id = a
                    .command
                    .lane_id
                    .as_deref()
                    .or(prior.as_ref().map(|p| p.lane_id.as_str()))
                    .ok_or_else(|| invalid("lane_id is required"))?;
                let lane = board
                    .lanes
                    .iter()
                    .find(|l| l.id == lane_id)
                    .ok_or_else(|| invalid("Lane does not belong to this board"))?;
                let mut dest = Placement {
                    goal_id: goal_id.clone(),
                    board_id: board.id.clone(),
                    lane_id: lane.id.clone(),
                    position: a.command.data["position"]
                        .as_f64()
                        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis() as f64),
                    revision: prior.as_ref().map_or(previous_revision, |p| p.revision) + 1,
                    archived: prior.as_ref().is_some_and(|p| p.archived),
                    routing: prior.as_ref().and_then(|p| p.routing.clone()),
                    last_request_id: a.id.clone(),
                };
                if a.command.data.get("routing").is_some() {
                    dest.routing = route(&a.command.data)?
                }
                if a.command.operation == "card.archive" {
                    dest.archived = a.command.data["archived"].as_bool().unwrap_or(true)
                }
                if a.command.operation == "card.update"
                    && ["name", "description", "priority", "reporter"]
                        .iter()
                        .any(|key| a.command.data.get(key).is_some())
                {
                    work.edit_planning_goal(&goal_id, &a.command.data, &a.id)?;
                }
                let entry = prior
                    .as_ref()
                    .is_none_or(|p| p.board_id != dest.board_id || p.lane_id != dest.lane_id)
                    || a.command.operation == "card.apply";
                a.behavior = if entry && !dest.archived {
                    lane.action.clone()
                } else {
                    LaneAction::None
                };
                // Snapshot routing and bindings before any Skill is launched.
                a.result = json!({"goal_id":goal_id,"routing":dest.routing.as_ref().or(lane.routing.as_ref()).or(board.routing.as_ref()),"entry":entry,"source_status":goal["status"]});
                a.before = prior;
                a.destination = Some(dest);
                a.configuration = Some(pinned);
                a.phase = "exit".into();
                self.save_action(a)?;
            }
            let goal = work.show_goal_detail(&goal_id)?;
            let owner = goal["node_id"].as_str().unwrap_or("default");
            if a.phase != "handoff" && owner != self.node {
                // An explicit transfer can move pending actions; it never invents release authority.
                a.owner = owner.into();
                return Err(waiting("Waiting for the current Goal node"));
            }
            match a.phase.as_str() {
                "detach" => {
                    let path = self.path("cards", &goal_id)?;
                    if path.exists() {
                        fs::remove_file(path).map_err(io)?;
                    }
                    a.state = "complete".into();
                    a.result = json!({"goal_id": goal_id});
                    return Ok(());
                }
                "exit" => {
                    if a.result["entry"] == true && a.before.is_some() {
                        self.lane_event(a, "exit", &goal_id)?;
                    }
                    a.phase = "place".into();
                    self.save_action(a)?;
                }
                "place" => {
                    let dest = a.destination.as_ref().unwrap();
                    let path = self.path("cards", &goal_id)?;
                    if path.exists() {
                        let actual: Placement = read_json(&path)?;
                        if actual.last_request_id != a.id {
                            let expected = if a.before.is_none()
                                && a.command.operation == "card.attach"
                                && read_json::<Board>(&self.path("boards", &actual.board_id)?)?
                                    .deleted
                            {
                                dest.revision.checked_sub(1)
                            } else {
                                a.before.as_ref().map(|p| p.revision)
                            };
                            revision(expected, actual.revision)?
                        }
                    }
                    write_json(&path, dest)?;
                    a.phase = "enter".into();
                    self.save_action(a)?;
                }
                "enter" => {
                    if a.result["entry"] == true {
                        self.lane_event(a, "enter", &goal_id)?;
                    }
                    if a.behavior == LaneAction::None
                        || !matches!(goal["status"].as_str(), Some("draft" | "backlog"))
                    {
                        a.state = "complete".into();
                        return Ok(());
                    }
                    a.phase = "accept".into();
                    self.save_action(a)?;
                }
                "accept" => {
                    if goal["status"] == "draft" {
                        self.transition(&goal_id, GoalStatus::Backlog)?;
                        continue;
                    }
                    if goal["status"] != "backlog" {
                        return Err(conflict("Goal changed while acceptance was pending"));
                    }
                    if a.behavior == LaneAction::AcceptIntoBacklog {
                        a.state = "complete".into();
                        return Ok(());
                    }
                    a.phase = "route".into();
                    self.save_action(a)?;
                }
                "route" => {
                    if goal["status"] != "backlog" {
                        return Err(conflict("Release requires Backlog"));
                    }
                    if a.target_node.is_none() {
                        a.target_node = Some(self.select_node(a.result["routing"].as_str())?);
                        self.save_action(a)?;
                    }
                    let target = a.target_node.as_deref().unwrap();
                    self.validate_target(target)?;
                    if goal["feature_id"].is_string() && target != owner {
                        return Err(conflict(
                            "Transfer the Feature before routing its card to another node",
                        ));
                    }
                    // Source Exit gates run before ownership changes; target Todo Entry runs later.
                    for edge in [
                        "workflow.backlog.enter",
                        "workflow.backlog.success",
                        "workflow.backlog.exit",
                    ] {
                        self.lane_event(a, edge, &goal_id)?;
                    }
                    // Consume source gate evidence before materializing a Round:
                    // a crash after the Round write must not revalidate a roundless
                    // invocation against that newly created Round.
                    a.result["release_owner"] = json!(self.node);
                    a.result["round_count"] = json!(goal["rounds"].as_array().map_or(0, Vec::len));
                    let round = goal["rounds"].as_array().and_then(|rounds| rounds.last());
                    a.result["round_prompt"] =
                        round.map(|r| r["prompt"].clone()).unwrap_or_else(|| {
                            json!(
                                format!(
                                    "{}\n\n{}",
                                    goal["name"].as_str().unwrap_or_default(),
                                    goal["description"].as_str().unwrap_or_default()
                                )
                                .trim()
                            )
                        });
                    a.result["round_reporter"] = round
                        .map(|r| r["reporter"].clone())
                        .unwrap_or_else(|| goal["reporter"].clone());
                    a.phase = "materialize".into();
                    self.save_action(a)?;
                }
                "materialize" => {
                    if goal["status"] != "backlog" || a.result["release_owner"] != self.node {
                        return Err(conflict("Release authoring or ownership was superseded"));
                    }
                    let count = goal["rounds"].as_array().map_or(0, Vec::len);
                    let expected = a.result["round_count"].as_u64().unwrap_or(0) as usize;
                    let prompt = a.result["round_prompt"].as_str().unwrap_or_default();
                    let reporter = a.result["round_reporter"].as_str().filter(|r| !r.trim().is_empty())
                        .ok_or_else(|| waiting("Cancel this action, set a Reporter, then apply the lane action again"))?;
                    if count == 0 && expected == 0 {
                        let current_prompt = format!(
                            "{}\n\n{}",
                            goal["name"].as_str().unwrap_or_default(),
                            goal["description"].as_str().unwrap_or_default()
                        );
                        if current_prompt.trim() != prompt || goal["reporter"] != reporter {
                            return Err(conflict(
                                "Card content changed after release gates completed",
                            ));
                        }
                        work.append_goal_round_summary(&goal_id, reporter, prompt)?;
                    } else {
                        let round = goal["rounds"]
                            .as_array()
                            .and_then(|rounds| rounds.last())
                            .unwrap();
                        if count != expected.max(1)
                            || round["prompt"] != prompt
                            || round["reporter"] != reporter
                        {
                            return Err(conflict(
                                "Authored Round changed after release gates completed",
                            ));
                        }
                    }
                    a.phase = "handoff".into();
                    self.save_action(a)?;
                }
                "handoff" => {
                    let target = a
                        .target_node
                        .as_deref()
                        .ok_or_else(|| invalid("Missing release target"))?;
                    if owner != target {
                        if self.node != a.owner {
                            return Err(waiting("Waiting for source-node handoff"));
                        }
                        self.validate_target(target)?;
                        work.handoff_planning_goal(&goal_id, target, &a.id, &json!(a.invocations))?;
                    } else if goal["planning_release"]["request_id"] != a.id {
                        work.handoff_planning_goal(&goal_id, target, &a.id, &json!(a.invocations))?;
                    }
                    if target != self.node {
                        // Only the receiving node writes subsequent phases. Background
                        // state synchronization publishes both the receipt and Goal.
                        a.owner = target.into();
                        return Err(waiting(
                            "Waiting for state synchronization and the execution node",
                        ));
                    }
                    let handed = work.show_goal_detail(&goal_id)?;
                    if handed["planning_release"]["request_id"] != a.id
                        || handed["planning_release"]["target_node"] != target
                    {
                        return Err(conflict(
                            "Planning handoff receipt is missing or superseded",
                        ));
                    }
                    if handed["planning_release"]["source_node"] != target {
                        let sync =
                            crate::application::persistence_sync::state::FileGitSyncService::new(
                                &self.target,
                                &self.runtime,
                            )
                            .try_sync_state()?;
                        if !sync.ok
                            || !sync.attempted
                            || sync.deferred
                            || sync.remote_configured != Some(true)
                        {
                            return Err(waiting(
                                "Publish the node handoff through project state synchronization",
                            ));
                        }
                        let current = work.show_goal_detail(&goal_id)?;
                        if current["node_id"] != target
                            || current["planning_release"] != handed["planning_release"]
                        {
                            return Err(conflict("Handoff changed during synchronization"));
                        }
                    }
                    if target != self.node {
                        return Err(waiting("Waiting for the selected execution node"));
                    }
                    a.owner = target.into();
                    a.phase = "release".into();
                    self.save_action(a)?;
                }
                "release" => {
                    if goal["status"] == "backlog" {
                        // Source gate evidence was checked before the durable ownership handoff.
                        crate::application::events::transitions::approve_exit(
                            &self.root, &goal, "todo",
                        )?;
                        self.transition(&goal_id, GoalStatus::Todo)?;
                        continue;
                    }
                    if matches!(
                        goal["status"].as_str(),
                        Some("draft" | "failed" | "cancelled")
                    ) {
                        return Err(conflict("Release was superseded"));
                    }
                    a.state = "complete".into();
                    a.result["target_node"] = json!(a.target_node);
                    return Ok(());
                }
                _ => return Err(invalid("Unknown planning phase")),
            }
        }
        Err(waiting("Lifecycle transition is pending"))
    }
    fn transition(&self, id: &str, to: GoalStatus) -> RefineResult<()> {
        let goal = self.work().show_goal_detail(id)?;
        if goal["pending_event_transition"]["state"] == "pending" {
            return Err(waiting("Required lifecycle Skills are pending"));
        }
        self.work()
            .transition_goal_status(id, to)
            .map(|_| ())
            .map_err(|e| {
                if e.to_string().contains("pending") {
                    waiting("Required lifecycle Skills are pending")
                } else {
                    e
                }
            })
    }
    fn validate_target(&self, id: &str) -> RefineResult<()> {
        let registry = crate::application::fleet::nodes::FileNodeRegistryService::with_active_root(
            &self.root,
            &self.runtime,
        )
        .load_registry()?;
        if registry.nodes.iter().any(|n| {
            n.id == id
                && n.enabled
                && !n.archived
                && n.health
                    .as_ref()
                    .is_none_or(|h| h.status != "failed" && h.status != "deprovisioned")
        }) {
            Ok(())
        } else {
            Err(waiting(
                "Selected node is disabled, archived, unhealthy, or unknown",
            ))
        }
    }
    fn select_node(&self, route: Option<&str>) -> RefineResult<String> {
        let route = route.ok_or_else(|| {
            waiting(
                "Cancel this action, configure execution routing, then apply the lane action again",
            )
        })?;
        if route != "auto" {
            self.validate_target(route)?;
            return Ok(route.into());
        }
        let registry = crate::application::fleet::nodes::FileNodeRegistryService::with_active_root(
            &self.root,
            &self.runtime,
        )
        .load_registry()?;
        let goals = self.work().list_goal_summaries()?;
        let mut candidates: Vec<_> = registry
            .nodes
            .into_iter()
            .filter(|n| self.validate_target(&n.id).is_ok())
            .map(|n| {
                let load = goals
                    .iter()
                    .filter(|g| {
                        g.goal.node_id.as_deref() == Some(&n.id)
                            && matches!(
                                g.goal.status,
                                GoalStatus::Todo
                                    | GoalStatus::Plan
                                    | GoalStatus::Implement
                                    | GoalStatus::Quality
                                    | GoalStatus::Governance
                            )
                    })
                    .count();
                (load, n.id)
            })
            .collect();
        candidates.sort();
        candidates
            .first()
            .map(|(_, id)| id.clone())
            .ok_or_else(|| waiting("No eligible execution node"))
    }
    fn lane_event(&self, a: &mut PlanningAction, edge: &str, goal_id: &str) -> RefineResult<()> {
        let events = FileEventService::with_runtime_root(&self.root, &self.runtime);
        let source = if edge.starts_with("workflow.") {
            edge.to_string()
        } else {
            format!("planning.lane.{edge}")
        };
        let id = if let Some(id) = a.invocations.get(edge) {
            id.clone()
        } else {
            let action_config = a
                .configuration
                .as_ref()
                .ok_or_else(|| invalid("Missing pinned configuration"))?;
            let goal = self.work().show_goal_detail(goal_id)?;
            let entry_config;
            let config = if edge == "workflow.backlog.enter" {
                entry_config =
                    crate::application::events::gate_configuration::transition_entry_configuration(
                        &goal,
                        action_config,
                        &self.node,
                        "backlog",
                    )?;
                &entry_config
            } else {
                action_config
            };
            let mut event = config
                .events
                .get(&source)
                .cloned()
                .ok_or_else(|| invalid("Missing lane Event definition"))?;
            let placement = if edge == "exit" {
                a.before.as_ref()
            } else {
                a.destination.as_ref()
            }
            .unwrap();
            event.bindings.retain(|b| {
                b.planning.as_ref().is_none_or(|f| {
                    f.board_id.as_ref().is_none_or(|v| v == &placement.board_id)
                        && f.lane_id.as_ref().is_none_or(|v| v == &placement.lane_id)
                })
            });
            if !event.enabled || config.bindings(&event, &self.node).is_empty() {
                return Ok(());
            }
            let mut context = events.manual_context(&self.target, &json!({"goal_id":goal_id}))?;
            context.data["planning"] = json!({"action_id":a.id,"edge":edge,"board_id":placement.board_id,"lane_id":placement.lane_id,"from":a.before,"to":a.destination,"actor":a.command.actor,"routing":a.result["routing"],"behavior":a.behavior});
            let occurrence = if edge == "workflow.backlog.enter" {
                if let Some(occurrence) = goal["workflow_events"].as_array().and_then(|items| {
                    items.iter().find(|item| {
                        item["generation"] == goal["event_generation"] && item["to"] == "backlog"
                    })
                }) {
                    context.data["occurrence"] = occurrence.clone();
                }
                format!(
                    "{goal_id}:{}:{}:{}:{edge}:",
                    context.round_idx.unwrap_or(0),
                    goal["event_generation"].as_u64().unwrap_or(0),
                    self.node
                )
            } else {
                format!("planning:{}:{edge}", a.id)
            };
            let invocation =
                events.prepare_pinned(config, &event, context, BTreeMap::new(), &occurrence)?;
            a.invocations.insert(edge.into(), invocation.id.clone());
            self.save_action(a)?;
            invocation.id
        };
        let invocation = events.invocation(&id)?;
        if invocation.state == InvocationState::Cancelled {
            return Err(conflict("Lane Skill was cancelled"));
        }
        for binding in invocation
            .bindings
            .iter()
            .filter(|b| b.binding.mode == BindingMode::Blocking)
        {
            match invocation.results.get(&binding.binding.id) {
                Some(r) if r.outcome == "success" => {}
                Some(_) => {
                    return Err(conflict(
                        "Required lane Skill failed; inspect its invocation",
                    ));
                }
                None if invocation.state.terminal() => {
                    return Err(conflict("Required lane Skill has no valid completion"));
                }
                None => return Err(waiting("Required lane Skills are pending")),
            }
        }
        if let Some(required) =
            crate::application::events::execution::BlockingInvocation::pin(&invocation)
        {
            events.settle_blocking(&[required], |result| result)?;
        }
        Ok(())
    }
}

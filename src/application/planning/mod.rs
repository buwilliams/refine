//! Shared project boards. Cards reference Goals; placement never rewrites execution state.
use crate::application::{events::execution::stable_id, work_items::FileWorkItemService};
use crate::infrastructure::{
    process::supervisor::coordination::with_record_lock,
    storage::automation::{read_json, write_json},
};
use crate::{
    error::{RefineError, RefineResult},
    model::workflow::GoalStatus,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::PathBuf};

mod actions;
mod migration;
#[cfg(test)]
mod tests;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LaneAction {
    #[default]
    None,
    AcceptIntoBacklog,
    Release,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Lane {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub action: LaneAction,
    #[serde(default)]
    pub routing: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Board {
    pub id: String,
    pub name: String,
    pub revision: u64,
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub deleted: bool,
    #[serde(default)]
    pub routing: Option<String>,
    pub lanes: Vec<Lane>,
    pub created: String,
    #[serde(default)]
    pub reporter: Option<String>,
    #[serde(default)]
    pub last_request_id: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Placement {
    pub goal_id: String,
    pub board_id: String,
    pub lane_id: String,
    pub position: f64,
    pub revision: u64,
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub routing: Option<String>,
    #[serde(default)]
    pub last_request_id: String,
}
/// Transport-independent commands. `data` holds the operation's editable fields.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PlanningCommand {
    pub request_id: String,
    pub operation: String,
    #[serde(default)]
    pub expected_revision: Option<u64>,
    #[serde(default)]
    pub board_id: Option<String>,
    #[serde(default)]
    pub lane_id: Option<String>,
    #[serde(default)]
    pub goal_id: Option<String>,
    #[serde(default)]
    pub actor: String,
    #[serde(default)]
    pub data: Value,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PlanningAction {
    pub id: String,
    pub command: PlanningCommand,
    pub owner: String,
    pub state: String,
    pub phase: String,
    pub created: String,
    pub result: Value,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub target_node: Option<String>,
    #[serde(default)]
    pub before: Option<Placement>,
    #[serde(default)]
    pub destination: Option<Placement>,
    #[serde(default)]
    pub behavior: LaneAction,
    #[serde(default)]
    pub configuration: Option<crate::model::automation::AutomationConfig>,
    #[serde(default)]
    pub invocations: BTreeMap<String, String>,
}
impl PlanningAction {
    pub fn terminal(&self) -> bool {
        matches!(self.state.as_str(), "complete" | "failed" | "cancelled")
    }
}

pub struct FilePlanningService {
    pub root: PathBuf,
    pub target: PathBuf,
    pub runtime: PathBuf,
    pub node: String,
}
impl FilePlanningService {
    pub fn new(
        root: impl Into<PathBuf>,
        target: impl Into<PathBuf>,
        runtime: impl Into<PathBuf>,
    ) -> RefineResult<Self> {
        let root = root.into();
        let runtime = runtime.into();
        let node = crate::application::fleet::nodes::FileNodeRegistryService::with_active_root(
            &root, &runtime,
        )
        .active_node_id()?;
        Ok(Self {
            root,
            target: target.into(),
            runtime,
            node,
        })
    }
    fn work(&self) -> FileWorkItemService {
        FileWorkItemService::for_node(&self.root, &self.node)
    }
    fn path(&self, collection: &str, id: &str) -> RefineResult<PathBuf> {
        if !crate::model::automation::valid_id(id) {
            return Err(invalid("Invalid planning identifier"));
        }
        Ok(self
            .root
            .join("planning")
            .join(collection)
            .join(format!("{id}.json")))
    }
    fn records<T: serde::de::DeserializeOwned>(&self, collection: &str) -> RefineResult<Vec<T>> {
        let path = self.root.join("planning").join(collection);
        if !path.exists() {
            return Ok(vec![]);
        }
        let mut entries = fs::read_dir(path)
            .map_err(io)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(io)?;
        entries.sort_by_key(|e| e.file_name());
        entries
            .into_iter()
            .filter(|e| e.path().extension().is_some_and(|v| v == "json"))
            .map(|e| read_json(&e.path()))
            .collect()
    }
    pub fn board(&self, id: &str) -> RefineResult<Board> {
        let board: Board = read_json(&self.path("boards", id)?)?;
        if board.deleted {
            return Err(RefineError::NotFound("Board was deleted".into()));
        }
        Ok(board)
    }
    pub fn placement(&self, id: &str) -> RefineResult<Placement> {
        let placement: Placement = read_json(&self.path("cards", id)?)?;
        let board: Board = read_json(&self.path("boards", &placement.board_id)?)?;
        if board.deleted {
            return Err(RefineError::NotFound(
                "Card placement was removed with its board".into(),
            ));
        }
        Ok(placement)
    }
    pub fn action(&self, id: &str) -> RefineResult<PlanningAction> {
        read_json(&self.path("actions", id)?)
    }
    fn save_action(&self, a: &PlanningAction) -> RefineResult<()> {
        let path = self.path("actions", &a.id)?;
        if let Ok(previous) = read_json::<PlanningAction>(&path)
            && serde_json::to_value(previous).ok() == serde_json::to_value(a).ok()
        {
            return Ok(());
        }
        write_json(&path, a)
    }
    pub fn snapshot(&self) -> RefineResult<Value> {
        let mut boards: Vec<Board> = self.records("boards")?;
        let deleted: std::collections::BTreeSet<_> = boards
            .iter()
            .filter(|board| board.deleted)
            .map(|board| board.id.clone())
            .collect();
        boards.retain(|board| !board.deleted);
        let placements: Vec<Placement> = self.records("cards")?;
        let mut cards = Vec::new();
        for placement in placements
            .into_iter()
            .filter(|placement| !deleted.contains(&placement.board_id))
        {
            match self.work().show_goal_detail(&placement.goal_id) {
                Ok(goal) => cards.push(json!({"placement":placement,"goal":goal})),
                Err(e) => cards.push(json!({"placement":placement,"error":e.to_string()})),
            }
        }
        let (actions, errors) = self.read_actions()?;
        let nodes = crate::application::fleet::nodes::FileNodeRegistryService::with_active_root(
            &self.root,
            &self.runtime,
        )
        .load_registry()?
        .nodes;
        Ok(
            json!({"boards":boards,"cards":cards,"actions":actions,"errors":errors,"node_id":self.node,"nodes":nodes,"migration":self.root.join("planning/migration.json").exists()}),
        )
    }
    pub fn submit(&self, mut command: PlanningCommand) -> RefineResult<PlanningAction> {
        if let Some(id) = command.goal_id.as_mut() {
            *id = id.trim().to_uppercase();
        }
        if !crate::model::automation::valid_id(&command.request_id) {
            return Err(invalid("request_id must be a stable valid identifier"));
        }
        if command.actor.trim().is_empty() {
            command.actor = "operator".into()
        }
        let id = command.request_id.clone();
        with_record_lock(&self.root, &format!("planning-action-{id}"), || {
            let path = self.path("actions", &id)?;
            if path.exists() {
                let a = self.action(&id)?;
                if a.command != command {
                    return Err(conflict("request_id identifies a different command"));
                }
                return Ok(a);
            }
            validate_command(&command)?;
            let owner = if let Some(goal) = &command.goal_id
                && command.operation != "card.create"
            {
                match self.work().show_goal_detail(goal) {
                    Ok(goal) => goal["node_id"].as_str().unwrap_or("default").to_string(),
                    Err(RefineError::NotFound(_)) if command.operation == "card.detach" => {
                        self.node.clone()
                    }
                    Err(error) => return Err(error),
                }
            } else {
                self.node.clone()
            };
            let action = PlanningAction {
                id: id.clone(),
                command,
                owner,
                state: "queued".into(),
                phase: "prepare".into(),
                created: now(),
                result: Value::Null,
                message: None,
                target_node: None,
                before: None,
                destination: None,
                behavior: LaneAction::None,
                configuration: None,
                invocations: BTreeMap::new(),
            };
            self.save_action(&action)?;
            Ok(action)
        })
    }
    pub fn cancel(&self, id: &str) -> RefineResult<PlanningAction> {
        with_record_lock(&self.root, &format!("planning-action-{id}"), || {
            let mut a = self.action(id)?;
            if !a.terminal() {
                a.state = "cancelled".into();
                a.message = Some(
                    "Cancelled by operator; completed moves and workflow transitions are retained"
                        .into(),
                );
                self.save_action(&a)?;
                let events = crate::application::events::FileEventService::with_runtime_root(
                    &self.root,
                    &self.runtime,
                );
                for invocation in a.invocations.values() {
                    let _ = events.cancel_invocation(invocation);
                }
            }
            Ok(a)
        })
    }
    fn read_actions(&self) -> RefineResult<(Vec<PlanningAction>, Vec<String>)> {
        let directory = self.root.join("planning/actions");
        if !directory.exists() {
            return Ok((vec![], vec![]));
        }
        let mut actions: Vec<PlanningAction> = vec![];
        let mut errors = vec![];
        for entry in fs::read_dir(directory).map_err(io)? {
            let path = entry.map_err(io)?.path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            match read_json(&path) {
                Ok(action) => actions.push(action),
                Err(e) => errors.push(format!("{}: {e}", path.display())),
            }
        }
        actions.sort_by(|a, b| a.created.cmp(&b.created).then_with(|| a.id.cmp(&b.id)));
        Ok((actions, errors))
    }
    pub fn process_action(&self, id: &str) -> RefineResult<()> {
        let action = self.action(id)?;
        if action.terminal() {
            return Ok(());
        }
        let current_owner = action
            .command
            .goal_id
            .as_deref()
            .and_then(|id| self.work().show_goal_detail(id).ok())
            .and_then(|g| g["node_id"].as_str().map(str::to_string));
        if action.owner != self.node
            && action.target_node.as_deref() != Some(&self.node)
            && current_owner.as_deref() != Some(&self.node)
        {
            return Ok(());
        }
        let key = action
            .command
            .goal_id
            .clone()
            .unwrap_or_else(|| action.id.clone());
        with_record_lock(&self.root, &format!("planning-card-{key}"), || {
            with_record_lock(
                &self.root,
                &format!("planning-action-{}", action.id),
                || {
                    let mut a = self.action(&action.id)?;
                    if a.terminal() {
                        return Ok(());
                    }
                    if let Some(owner) = current_owner.as_ref()
                        && owner == &self.node
                        && a.phase == "prepare"
                    {
                        a.owner = owner.clone();
                    }
                    // Serialize the whole resumable action, including waiting Skills,
                    // rather than only each individual worker tick.
                    if let Some(goal_id) = a.command.goal_id.as_ref() {
                        let (actions, _) = self.read_actions()?;
                        let mut siblings: Vec<_> = actions
                            .into_iter()
                            .filter(|other| {
                                !other.terminal() && other.command.goal_id.as_ref() == Some(goal_id)
                            })
                            .collect();
                        siblings.sort_by_key(|other| {
                            (
                                other.phase == "prepare",
                                other.created.clone(),
                                other.id.clone(),
                            )
                        });
                        if let Some(first) = siblings.first()
                            && first.id != a.id
                        {
                            a.state = "waiting".into();
                            a.message =
                                Some(format!("Waiting for earlier card action {}", first.id));
                            return self.save_action(&a);
                        }
                    }
                    match self.advance(&mut a) {
                        Ok(()) => {}
                        Err(RefineError::Conflict(message)) if message.starts_with("wait:") => {
                            a.state = "waiting".into();
                            a.message = Some(message)
                        }
                        Err(e) => {
                            a.state = "failed".into();
                            a.message = Some(e.to_string())
                        }
                    }
                    self.save_action(&a)
                },
            )
        })
    }
    /// Bounded and fair: waiting work and damaged records cannot starve siblings.
    pub fn process_pending(&self) -> RefineResult<usize> {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static CURSOR: AtomicUsize = AtomicUsize::new(0);
        let (actions, errors) = self.read_actions()?;
        for error in errors {
            eprintln!("refine planning: {error}");
        }
        let mut pending: Vec<_> = actions.into_iter().filter(|a| !a.terminal()).collect();
        if pending.is_empty() {
            return Ok(0);
        }
        let offset = CURSOR.fetch_add(64, Ordering::Relaxed) % pending.len();
        pending.rotate_left(offset);
        let mut count = 0;
        for action in pending.into_iter().take(64) {
            let before = self.action(&action.id)?;
            if let Err(error) = self.process_action(&action.id) {
                eprintln!("refine planning action {}: {error}", action.id);
            }
            let after = self.action(&action.id)?;
            if serde_json::to_value(&before).ok() != serde_json::to_value(&after).ok() {
                count += 1;
            }
        }
        Ok(count)
    }
    fn mutate_board(&self, a: &mut PlanningAction) -> RefineResult<()> {
        let c = &a.command;
        let id = c
            .board_id
            .clone()
            .unwrap_or_else(|| format!("board-{}", &stable_id(&a.id)[..24]));
        with_record_lock(&self.root, &format!("planning-board-{id}"), || {
            let path = self.path("boards", &id)?;
            let mut b = if path.exists() {
                read_json::<Board>(&path)?
            } else if c.operation == "board.create" {
                Board {
                    id: id.clone(),
                    name: text(&c.data, "name")?,
                    revision: 0,
                    archived: false,
                    deleted: false,
                    routing: None,
                    lanes: vec![],
                    created: now(),
                    reporter: c.data["reporter"].as_str().map(str::to_string),
                    last_request_id: String::new(),
                }
            } else {
                return Err(invalid("Board does not exist"));
            };
            if b.last_request_id == a.id {
                a.result = json!(b);
                a.state = "complete".into();
                return Ok(());
            }
            if b.deleted {
                return Err(RefineError::NotFound("Board was deleted".into()));
            }
            if c.operation != "board.create" {
                revision(c.expected_revision, b.revision)?;
            } else if b.revision > 0 {
                return Err(conflict("Board already exists"));
            }
            match c.operation.as_str() {
                "board.create" => {
                    b.routing = route(&c.data)?;
                    b.lanes = vec![
                        Lane {
                            id: format!("lane-{}", &stable_id(&format!("{}:open", a.id))[..24]),
                            name: "Ideas".into(),
                            action: LaneAction::None,
                            routing: None,
                        },
                        Lane {
                            id: format!("lane-{}", &stable_id(&format!("{}:done", a.id))[..24]),
                            name: "Done".into(),
                            action: LaneAction::None,
                            routing: None,
                        },
                    ];
                }
                "board.update" => {
                    if c.data.get("name").is_some() {
                        b.name = text(&c.data, "name")?
                    }
                    if c.data.get("routing").is_some() {
                        b.routing = route(&c.data)?
                    }
                }
                "board.delete" => {
                    let placements: Vec<Placement> = self.records("cards")?;
                    let (actions, errors) = self.read_actions()?;
                    if !errors.is_empty() {
                        return Err(conflict(
                            "Repair unreadable planning actions before deleting this board",
                        ));
                    }
                    if actions.iter().any(|pending| {
                        pending.id != a.id
                            && !pending.terminal()
                            && (pending.command.board_id.as_deref() == Some(&id)
                                || pending.before.as_ref().is_some_and(|p| p.board_id == id)
                                || pending
                                    .destination
                                    .as_ref()
                                    .is_some_and(|p| p.board_id == id)
                                || pending.command.goal_id.as_ref().is_some_and(|goal_id| {
                                    placements
                                        .iter()
                                        .any(|p| &p.goal_id == goal_id && p.board_id == id)
                                }))
                    }) {
                        return Err(conflict(
                            "Finish or cancel pending board actions before deleting this board",
                        ));
                    }
                    // Retain a synced deletion marker so old placements cannot resurrect the board.
                    b.deleted = true;
                }
                "board.archive" => b.archived = c.data["archived"].as_bool().unwrap_or(true),
                "lane.create" => b.lanes.push(Lane {
                    id: format!("lane-{}", &stable_id(&a.id)[..24]),
                    name: text(&c.data, "name")?,
                    action: serde_json::from_value(
                        c.data.get("action").cloned().unwrap_or(json!("none")),
                    )
                    .map_err(|e| invalid(&e.to_string()))?,
                    routing: route(&c.data)?,
                }),
                "lane.update" => {
                    let lane = b
                        .lanes
                        .iter_mut()
                        .find(|l| Some(&l.id) == c.lane_id.as_ref())
                        .ok_or_else(|| invalid("Lane does not exist"))?;
                    if c.data.get("name").is_some() {
                        lane.name = text(&c.data, "name")?
                    }
                    if let Some(action) = c.data.get("action") {
                        lane.action = serde_json::from_value(action.clone())
                            .map_err(|e| invalid(&e.to_string()))?
                    }
                    if c.data.get("routing").is_some() {
                        lane.routing = route(&c.data)?
                    }
                }
                "lane.reorder" => {
                    let ids: Vec<String> = serde_json::from_value(c.data["lane_ids"].clone())
                        .map_err(|e| invalid(&e.to_string()))?;
                    let old: std::collections::BTreeSet<_> =
                        b.lanes.iter().map(|l| l.id.clone()).collect();
                    if ids.len() != old.len()
                        || ids
                            .iter()
                            .cloned()
                            .collect::<std::collections::BTreeSet<_>>()
                            != old
                    {
                        return Err(invalid("lane_ids must contain each lane exactly once"));
                    }
                    b.lanes
                        .sort_by_key(|l| ids.iter().position(|id| id == &l.id).unwrap())
                }
                "lane.delete" => {
                    if self
                        .records::<Placement>("cards")?
                        .iter()
                        .any(|p| p.board_id == id && Some(&p.lane_id) == c.lane_id.as_ref())
                    {
                        return Err(conflict(
                            "Move or detach every card before deleting its lane",
                        ));
                    }
                    b.lanes.retain(|l| Some(&l.id) != c.lane_id.as_ref());
                }
                _ => return Err(invalid("Unknown board operation")),
            }
            b.revision += 1;
            b.last_request_id = a.id.clone();
            write_json(&path, &b)?;
            a.result = json!(b);
            a.state = "complete".into();
            Ok(())
        })
    }
}
fn io(e: std::io::Error) -> RefineError {
    RefineError::Io(e.to_string())
}
fn invalid(s: &str) -> RefineError {
    RefineError::InvalidInput(s.into())
}
fn conflict(s: &str) -> RefineError {
    RefineError::Conflict(s.into())
}
fn waiting(s: &str) -> RefineError {
    conflict(&format!("wait: {s}"))
}
fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}
fn text(v: &Value, key: &str) -> RefineResult<String> {
    v[key]
        .as_str()
        .map(str::trim)
        .filter(|v| !v.is_empty() && v.len() <= 16000)
        .map(str::to_string)
        .ok_or_else(|| {
            invalid(&format!(
                "{key} must be nonempty text (maximum 16000 bytes)"
            ))
        })
}
fn revision(expected: Option<u64>, actual: u64) -> RefineResult<()> {
    if expected != Some(actual) {
        Err(conflict("Planning record changed; refresh its revision"))
    } else {
        Ok(())
    }
}
fn route(v: &Value) -> RefineResult<Option<String>> {
    match v.get("routing") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if crate::model::automation::valid_id(s) => Ok(Some(s.clone())),
        _ => Err(invalid("routing must be a node ID, auto, or null")),
    }
}
fn validate_command(c: &PlanningCommand) -> RefineResult<()> {
    if ![
        "board.create",
        "board.update",
        "board.archive",
        "board.delete",
        "lane.create",
        "lane.update",
        "lane.reorder",
        "lane.delete",
        "card.create",
        "card.attach",
        "card.update",
        "card.move",
        "card.archive",
        "card.detach",
        "card.apply",
        "migrate",
    ]
    .contains(&c.operation.as_str())
    {
        return Err(invalid("Unknown planning operation"));
    }
    for id in [&c.board_id, &c.lane_id, &c.goal_id].into_iter().flatten() {
        if !crate::model::automation::valid_id(id) {
            return Err(invalid("Invalid identifier"));
        }
    }
    if !c.data.is_null() && !c.data.is_object() {
        return Err(invalid("data must be an object"));
    }
    Ok(())
}

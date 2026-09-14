use super::{TemplateRecord, TemplateStore, catalog};
use crate::application::agent_io::prompts::{PromptEngine, PromptTemplateError};
use crate::error::{RefineError, RefineResult};
use serde::{Deserialize, Serialize};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    path::Path,
};

const MAX_DEPTH: usize = 32;
pub const MAX_RENDER_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug)]
pub enum TemplateValue {
    Literal(String),
    Template(String),
}
pub type Variables = BTreeMap<String, TemplateValue>;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TemplateSnapshot {
    pub records: BTreeMap<String, TemplateRecord>,
}

impl TemplateSnapshot {
    pub fn render(&self, id: &str, values: &Variables) -> RefineResult<String> {
        self.expand_template(id, values, &mut Vec::new(), &Cell::new(0))
    }

    pub fn render_skill(&self, source: &str, values: &Variables) -> RefineResult<String> {
        self.expand(
            source,
            values,
            &mut vec!["skill".into()],
            false,
            &Cell::new(0),
        )
    }

    fn expand_template(
        &self,
        id: &str,
        values: &Variables,
        stack: &mut Vec<String>,
        budget: &Cell<usize>,
    ) -> RefineResult<String> {
        let label = format!("templates.{id}");
        self.check_cycle(&label, stack)?;
        let record = self
            .records
            .get(id)
            .ok_or_else(|| RefineError::InvalidInput(format!("Unknown template: {id}")))?;
        stack.push(label);
        let result = self.expand(&record.prompt, values, stack, true, budget);
        stack.pop();
        result
    }

    fn check_cycle(&self, label: &str, stack: &[String]) -> RefineResult<()> {
        if stack.iter().any(|entry| entry == label) || stack.len() >= MAX_DEPTH {
            return Err(RefineError::InvalidInput(format!(
                "Template expansion cycle or depth limit: {} → {label}",
                stack.join(" → ")
            )));
        }
        Ok(())
    }

    fn expand(
        &self,
        source: &str,
        values: &Variables,
        stack: &mut Vec<String>,
        strict: bool,
        budget: &Cell<usize>,
    ) -> RefineResult<String> {
        budget.set(budget.get() + 1);
        if budget.get() > 4096 {
            return Err(RefineError::InvalidInput(
                "Template expansion exceeds 4096 fragments".into(),
            ));
        }
        PromptEngine::render_resolved(source, strict, MAX_RENDER_BYTES, |name| {
            let mut resolve = || -> RefineResult<Option<String>> {
                if let Some(id) = name.strip_prefix("templates.") {
                    return self.expand_template(id, values, stack, budget).map(Some);
                }
                match values.get(name) {
                    Some(TemplateValue::Literal(value)) => Ok(Some(value.clone())),
                    Some(TemplateValue::Template(source)) => {
                        self.check_cycle(name, stack)?;
                        stack.push(name.into());
                        let result = self.expand(source, values, stack, false, budget);
                        stack.pop();
                        result.map(Some)
                    }
                    None if name == "refine_executable" => {
                        let path =
                            std::env::current_exe().map_err(|e| RefineError::Io(e.to_string()))?;
                        Ok(Some(
                            path.to_str()
                                .ok_or_else(|| {
                                    RefineError::InvalidInput(
                                        "Refine executable path is not Unicode".into(),
                                    )
                                })?
                                .into(),
                        ))
                    }
                    None if catalog::COMMON.iter().any(|(key, _)| *key == name) => {
                        Ok(Some(String::new()))
                    }
                    None => Ok(None),
                }
            };
            resolve().map_err(|e| PromptTemplateError::Resolution(e.to_string()))
        })
        .map_err(|e| RefineError::InvalidInput(format!("Template rendering failed: {e}")))
    }

    pub fn validate_references(&self) -> RefineResult<()> {
        fn visit(
            snapshot: &TemplateSnapshot,
            id: &str,
            path: &mut Vec<String>,
            visited: &mut std::collections::BTreeSet<String>,
        ) -> RefineResult<()> {
            snapshot.check_cycle(id, path)?;
            if visited.contains(id) {
                return Ok(());
            }
            path.push(id.into());
            let record = snapshot
                .records
                .get(id)
                .ok_or_else(|| RefineError::InvalidInput(format!("Unknown template {id}")))?;
            for name in catalog::references(&record.prompt).map_err(RefineError::InvalidInput)? {
                if let Some(next) = name.strip_prefix("templates.") {
                    visit(snapshot, next, path, visited)?;
                }
            }
            path.pop();
            visited.insert(id.into());
            Ok(())
        }
        let mut visited = std::collections::BTreeSet::new();
        for id in self.records.keys() {
            visit(self, id, &mut Vec::new(), &mut visited)?;
        }
        Ok(())
    }
}

#[derive(Clone)]
struct Scope {
    snapshot: TemplateSnapshot,
    values: Variables,
}
thread_local! { static CURRENT: RefCell<Option<Scope>> = const { RefCell::new(None) }; }

/// Explicit per-operation scope; thread-local storage keeps existing synchronous
/// prompt builders and transport on the same pinned configuration. Never shared
/// across concurrent project operations, and restored even on early return.
pub struct TemplateScope {
    previous: Option<Scope>,
}
impl TemplateScope {
    pub fn for_workspace(workspace: Option<&Path>) -> RefineResult<Self> {
        let root = Self::workspace_root(workspace)?;
        Self::for_root(root.as_deref())
    }
    fn workspace_root(workspace: Option<&Path>) -> RefineResult<Option<std::path::PathBuf>> {
        let Some(workspace) = workspace else {
            return Ok(None);
        };
        if !workspace.ancestors().any(|path| path.join(".git").exists()) {
            return Ok(None);
        }
        crate::infrastructure::storage::project_layout::refine_dir_for_target_root(workspace)
            .map(Some)
    }
    pub fn for_delivery(
        workspace: Option<&Path>,
        metadata: &mut serde_json::Map<String, serde_json::Value>,
    ) -> RefineResult<Self> {
        if metadata.contains_key("template_snapshot") {
            return Self::pin(None, metadata);
        }
        if let Some(scope) = CURRENT.with(|current| current.borrow().clone()) {
            metadata.insert(
                "template_snapshot".into(),
                serde_json::json!(scope.snapshot),
            );
            let guard = Self::enter(scope.snapshot);
            Self::set_values(scope.values);
            return Ok(guard);
        }
        let root = Self::workspace_root(workspace)?;
        Self::pin(root.as_deref(), metadata)
    }
    pub fn pin(
        root: Option<&Path>,
        metadata: &mut serde_json::Map<String, serde_json::Value>,
    ) -> RefineResult<Self> {
        let snapshot = if let Some(value) = metadata.get("template_snapshot") {
            serde_json::from_value(value.clone()).map_err(|e| {
                RefineError::Serialization(format!("Invalid retained Templates: {e}"))
            })?
        } else {
            let snapshot = TemplateStore::new(root).snapshot()?;
            metadata.insert("template_snapshot".into(), serde_json::json!(snapshot));
            snapshot
        };
        let values = CURRENT.with(|current| {
            current
                .borrow()
                .as_ref()
                .filter(|scope| scope.snapshot == snapshot)
                .map(|scope| scope.values.clone())
                .unwrap_or_default()
        });
        let guard = Self::enter(snapshot);
        Self::set_values(values);
        Ok(guard)
    }

    pub fn context_values(data: &serde_json::Value) -> Variables {
        let mut values = Variables::new();
        if let Some(system) = data["system"].as_object() {
            for (name, value) in system {
                if name != "refine_executable" {
                    values.insert(
                        name.clone(),
                        TemplateValue::Literal(
                            value
                                .as_str()
                                .map(str::to_string)
                                .unwrap_or_else(|| value.to_string()),
                        ),
                    );
                }
            }
        }
        let goal = &data["goal"];
        values.insert(
            "goal_context".into(),
            TemplateValue::Literal(if goal.is_null() {
                String::new()
            } else {
                goal.to_string()
            }),
        );
        if let Some(round) = goal["rounds"].as_array().and_then(|rounds| rounds.last()) {
            values.insert(
                "current_round_goal".into(),
                TemplateValue::Literal(round["prompt"].as_str().unwrap_or_default().into()),
            );
            values.insert(
                "accepted_plan".into(),
                TemplateValue::Literal(
                    round["implementation_plan"]["final_plan"]
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| round["implementation_plan"].to_string()),
                ),
            );
        }
        values
    }
    pub fn enter(snapshot: TemplateSnapshot) -> Self {
        Self {
            previous: CURRENT.with(|current| {
                current.replace(Some(Scope {
                    snapshot,
                    values: Variables::new(),
                }))
            }),
        }
    }
    pub fn inherit_or_root(root: Option<&Path>) -> RefineResult<Self> {
        if let Some(scope) = CURRENT.with(|current| current.borrow().clone()) {
            let guard = Self::enter(scope.snapshot);
            Self::set_values(scope.values);
            Ok(guard)
        } else {
            Self::for_root(root)
        }
    }
    pub fn for_root(root: Option<&Path>) -> RefineResult<Self> {
        Ok(Self::enter(TemplateStore::new(root).snapshot()?))
    }
    pub fn snapshot() -> RefineResult<TemplateSnapshot> {
        CURRENT
            .with(|current| {
                current
                    .borrow()
                    .as_ref()
                    .map(|scope| scope.snapshot.clone())
            })
            .map(Ok)
            .unwrap_or_else(|| TemplateStore::new(None).snapshot())
    }
    pub fn set_values(values: Variables) {
        CURRENT.with(|current| {
            if let Some(scope) = current.borrow_mut().as_mut() {
                scope.values.extend(values);
            }
        });
    }
    pub fn render(id: &str, values: Variables) -> RefineResult<String> {
        let mut all = CURRENT.with(|current| {
            current
                .borrow()
                .as_ref()
                .map(|scope| scope.values.clone())
                .unwrap_or_default()
        });
        all.extend(values);
        Self::snapshot()?.render(id, &all)
    }
    pub fn literals(values: &[(&str, &str)]) -> Variables {
        values
            .iter()
            .map(|(key, value)| ((*key).into(), TemplateValue::Literal((*value).into())))
            .collect()
    }
}
impl Drop for TemplateScope {
    fn drop(&mut self) {
        CURRENT.with(|current| {
            current.replace(self.previous.take());
        });
    }
}

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};

use chrono::Utc;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::process::subprocess::write_json_atomically;
use crate::process::supervisor::errors::{RefineError, RefineResult};

pub const TODO_LISTS_FILE: &str = "todo-lists.json";
const TODO_LISTS_LOCK_FILE: &str = ".todo-lists.lock";
const TODO_LIST_NAME_LIMIT: usize = 120;
const TODO_ITEM_TEXT_LIMIT: usize = 4_000;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
struct TodoStore {
    #[serde(default = "todo_store_version")]
    version: u8,
    #[serde(default)]
    lists: Vec<TodoList>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TodoList {
    pub id: String,
    pub reporter: String,
    pub name: String,
    pub created: String,
    pub updated: String,
    #[serde(default)]
    pub items: Vec<TodoItem>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TodoItem {
    pub id: String,
    pub text: String,
    pub done: bool,
    pub created: String,
    pub updated: String,
}

#[derive(Clone, Debug)]
pub struct FileTodoService {
    refine_dir: PathBuf,
}

impl FileTodoService {
    pub fn new(refine_dir: impl Into<PathBuf>) -> Self {
        Self {
            refine_dir: refine_dir.into(),
        }
    }

    pub fn list(&self, reporter: &str) -> RefineResult<Value> {
        let reporter = validate_reporter(reporter)?;
        let store = self.load()?;
        Ok(todo_list_response(&store, reporter))
    }

    pub fn create_list(&self, reporter: &str, name: &str) -> RefineResult<Value> {
        let reporter = validate_reporter(reporter)?.to_string();
        let name = validate_text("list name", name, TODO_LIST_NAME_LIMIT)?.to_string();
        self.mutate(|store| {
            ensure_unique_list_name(store, &reporter, &name, None)?;
            let now = now_timestamp();
            let list = TodoList {
                id: Uuid::new_v4().to_string(),
                reporter: reporter.clone(),
                name,
                created: now.clone(),
                updated: now,
                items: Vec::new(),
            };
            store.lists.push(list.clone());
            Ok(todo_mutation_response(store, &reporter, Some(&list), None))
        })
    }

    pub fn rename_list(&self, reporter: &str, list_id: &str, name: &str) -> RefineResult<Value> {
        let reporter = validate_reporter(reporter)?.to_string();
        let list_id = validate_id("list", list_id)?.to_string();
        let name = validate_text("list name", name, TODO_LIST_NAME_LIMIT)?.to_string();
        self.mutate(|store| {
            ensure_unique_list_name(store, &reporter, &name, Some(&list_id))?;
            let list = owned_list_mut(store, &reporter, &list_id)?;
            list.name = name;
            list.updated = now_timestamp();
            let changed = list.clone();
            Ok(todo_mutation_response(
                store,
                &reporter,
                Some(&changed),
                None,
            ))
        })
    }

    pub fn delete_list(&self, reporter: &str, list_id: &str) -> RefineResult<Value> {
        let reporter = validate_reporter(reporter)?.to_string();
        let list_id = validate_id("list", list_id)?.to_string();
        self.mutate(|store| {
            let position = store
                .lists
                .iter()
                .position(|list| list.id == list_id && list.reporter == reporter)
                .ok_or_else(|| {
                    RefineError::NotFound(format!("Todo list {list_id} was not found"))
                })?;
            store.lists.remove(position);
            Ok(todo_mutation_response(store, &reporter, None, None))
        })
    }

    pub fn add_item(&self, reporter: &str, list_id: &str, text: &str) -> RefineResult<Value> {
        let reporter = validate_reporter(reporter)?.to_string();
        let list_id = validate_id("list", list_id)?.to_string();
        let text = validate_text("todo text", text, TODO_ITEM_TEXT_LIMIT)?.to_string();
        self.mutate(|store| {
            let list = owned_list_mut(store, &reporter, &list_id)?;
            let now = now_timestamp();
            let item = TodoItem {
                id: Uuid::new_v4().to_string(),
                text,
                done: false,
                created: now.clone(),
                updated: now.clone(),
            };
            list.items.push(item.clone());
            list.updated = now;
            let changed_list = list.clone();
            Ok(todo_mutation_response(
                store,
                &reporter,
                Some(&changed_list),
                Some(&item),
            ))
        })
    }

    pub fn update_item(
        &self,
        reporter: &str,
        list_id: &str,
        item_id: &str,
        text: Option<&str>,
        done: Option<bool>,
    ) -> RefineResult<Value> {
        let reporter = validate_reporter(reporter)?.to_string();
        let list_id = validate_id("list", list_id)?.to_string();
        let item_id = validate_id("item", item_id)?.to_string();
        let text = text
            .map(|text| validate_text("todo text", text, TODO_ITEM_TEXT_LIMIT))
            .transpose()?
            .map(str::to_string);
        if text.is_none() && done.is_none() {
            return Err(RefineError::InvalidInput(
                "text or done is required".to_string(),
            ));
        }
        self.mutate(|store| {
            let list = owned_list_mut(store, &reporter, &list_id)?;
            let item = list
                .items
                .iter_mut()
                .find(|item| item.id == item_id)
                .ok_or_else(|| {
                    RefineError::NotFound(format!("Todo item {item_id} was not found"))
                })?;
            if let Some(text) = text {
                item.text = text;
            }
            if let Some(done) = done {
                item.done = done;
            }
            let now = now_timestamp();
            item.updated = now.clone();
            let changed_item = item.clone();
            list.updated = now;
            let changed_list = list.clone();
            Ok(todo_mutation_response(
                store,
                &reporter,
                Some(&changed_list),
                Some(&changed_item),
            ))
        })
    }

    pub fn delete_item(&self, reporter: &str, list_id: &str, item_id: &str) -> RefineResult<Value> {
        let reporter = validate_reporter(reporter)?.to_string();
        let list_id = validate_id("list", list_id)?.to_string();
        let item_id = validate_id("item", item_id)?.to_string();
        self.mutate(|store| {
            let list = owned_list_mut(store, &reporter, &list_id)?;
            let position = list
                .items
                .iter()
                .position(|item| item.id == item_id)
                .ok_or_else(|| {
                    RefineError::NotFound(format!("Todo item {item_id} was not found"))
                })?;
            list.items.remove(position);
            list.updated = now_timestamp();
            let changed_list = list.clone();
            Ok(todo_mutation_response(
                store,
                &reporter,
                Some(&changed_list),
                None,
            ))
        })
    }

    pub fn reassign_reporter(&self, old: &str, new: &str) -> RefineResult<()> {
        let old = validate_reporter(old)?.to_string();
        let new = validate_reporter(new)?.to_string();
        if old == new || !self.path().exists() {
            return Ok(());
        }
        self.mutate(|store| {
            let mut names = store
                .lists
                .iter()
                .filter(|list| list.reporter == new)
                .map(|list| list.name.to_lowercase())
                .collect::<BTreeSet<_>>();
            for list in store.lists.iter_mut().filter(|list| list.reporter == old) {
                list.reporter = new.clone();
                list.name = available_merged_list_name(&list.name, &mut names);
                list.updated = now_timestamp();
            }
            Ok(())
        })
    }

    fn path(&self) -> PathBuf {
        self.refine_dir.join(TODO_LISTS_FILE)
    }

    fn load(&self) -> RefineResult<TodoStore> {
        read_store(&self.path())
    }

    fn mutate<T>(&self, action: impl FnOnce(&mut TodoStore) -> RefineResult<T>) -> RefineResult<T> {
        fs::create_dir_all(&self.refine_dir).map_err(|error| {
            RefineError::Io(format!(
                "failed to create todo state directory {}: {error}",
                self.refine_dir.display()
            ))
        })?;
        let lock_path = self.refine_dir.join(TODO_LISTS_LOCK_FILE);
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to open todo mutation lock {}: {error}",
                    lock_path.display()
                ))
            })?;
        lock.lock_exclusive().map_err(|error| {
            RefineError::Io(format!(
                "failed to acquire todo mutation lock {}: {error}",
                lock_path.display()
            ))
        })?;
        let result = (|| {
            let mut store = self.load()?;
            let value = action(&mut store)?;
            write_store(&self.path(), &store)?;
            Ok(value)
        })();
        FileExt::unlock(&lock).map_err(|error| {
            RefineError::Io(format!(
                "failed to release todo mutation lock {}: {error}",
                lock_path.display()
            ))
        })?;
        result
    }
}

fn todo_store_version() -> u8 {
    1
}

fn read_store(path: &Path) -> RefineResult<TodoStore> {
    if !path.exists() {
        return Ok(TodoStore {
            version: todo_store_version(),
            lists: Vec::new(),
        });
    }
    let bytes = fs::read(path).map_err(|error| {
        RefineError::Io(format!(
            "failed to read todo state {}: {error}",
            path.display()
        ))
    })?;
    let store: TodoStore = serde_json::from_slice(&bytes).map_err(|error| {
        RefineError::Serialization(format!(
            "failed to parse todo state {}: {error}",
            path.display()
        ))
    })?;
    if store.version != todo_store_version() {
        return Err(RefineError::Serialization(format!(
            "unsupported todo state version {} in {}",
            store.version,
            path.display()
        )));
    }
    Ok(store)
}

fn write_store(path: &Path, store: &TodoStore) -> RefineResult<()> {
    let encoded = serde_json::to_string_pretty(store).map_err(|error| {
        RefineError::Serialization(format!("failed to encode todo state: {error}"))
    })?;
    write_json_atomically(path, format!("{encoded}\n").as_bytes(), "todo state")
}

fn todo_list_response(store: &TodoStore, reporter: &str) -> Value {
    let lists = store
        .lists
        .iter()
        .filter(|list| list.reporter == reporter)
        .cloned()
        .collect::<Vec<_>>();
    json!({
        "reporter": reporter,
        "lists": lists
    })
}

fn todo_mutation_response(
    store: &TodoStore,
    reporter: &str,
    list: Option<&TodoList>,
    item: Option<&TodoItem>,
) -> Value {
    let mut response = todo_list_response(store, reporter);
    if let Some(object) = response.as_object_mut() {
        object.insert("ok".to_string(), Value::Bool(true));
        if let Some(list) = list {
            object.insert("list".to_string(), json!(list));
        }
        if let Some(item) = item {
            object.insert("item".to_string(), json!(item));
        }
    }
    response
}

fn owned_list_mut<'a>(
    store: &'a mut TodoStore,
    reporter: &str,
    list_id: &str,
) -> RefineResult<&'a mut TodoList> {
    store
        .lists
        .iter_mut()
        .find(|list| list.id == list_id && list.reporter == reporter)
        .ok_or_else(|| RefineError::NotFound(format!("Todo list {list_id} was not found")))
}

fn ensure_unique_list_name(
    store: &TodoStore,
    reporter: &str,
    name: &str,
    except_id: Option<&str>,
) -> RefineResult<()> {
    if store.lists.iter().any(|list| {
        list.reporter == reporter
            && Some(list.id.as_str()) != except_id
            && list.name.eq_ignore_ascii_case(name)
    }) {
        return Err(RefineError::InvalidInput(format!(
            "Reporter {reporter} already has a todo list named {name}"
        )));
    }
    Ok(())
}

fn available_merged_list_name(name: &str, names: &mut BTreeSet<String>) -> String {
    if names.insert(name.to_lowercase()) {
        return name.to_string();
    }
    for suffix in 2.. {
        let candidate = format!("{name} ({suffix})");
        if names.insert(candidate.to_lowercase()) {
            return candidate;
        }
    }
    unreachable!("an integer suffix always produces a distinct todo list name")
}

fn validate_reporter(reporter: &str) -> RefineResult<&str> {
    validate_text("reporter", reporter, TODO_LIST_NAME_LIMIT)
}

fn validate_text<'a>(field: &str, value: &'a str, limit: usize) -> RefineResult<&'a str> {
    let value = value.trim();
    if value.is_empty() {
        return Err(RefineError::InvalidInput(format!("{field} is required")));
    }
    if value.chars().any(char::is_control) || value.chars().count() > limit {
        return Err(RefineError::InvalidInput(format!("invalid {field}")));
    }
    Ok(value)
}

fn validate_id<'a>(kind: &str, id: &'a str) -> RefineResult<&'a str> {
    let id = id.trim();
    if id.is_empty() || id.contains('/') || id.chars().any(char::is_control) {
        return Err(RefineError::InvalidInput(format!("invalid {kind} id")));
    }
    Ok(id)
}

fn now_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn reporter_scoped_todo_lists_persist_edit_done_undo_and_delete() {
        let temp_root = unique_temp_dir("todo-service");
        let refine_dir = temp_root.join(".refine");
        let service = FileTodoService::new(&refine_dir);

        let created = service.create_list("Buddy", "Release").unwrap();
        let list_id = created["list"]["id"].as_str().unwrap().to_string();
        service.create_list("Alex", "Release").unwrap();
        let added = service
            .add_item("Buddy", &list_id, "Verify the candidate")
            .unwrap();
        let item_id = added["item"]["id"].as_str().unwrap().to_string();

        let done = service
            .update_item("Buddy", &list_id, &item_id, None, Some(true))
            .unwrap();
        assert_eq!(done["item"]["done"], true);
        let edited = service
            .update_item(
                "Buddy",
                &list_id,
                &item_id,
                Some("Verify exact results"),
                Some(false),
            )
            .unwrap();
        assert_eq!(edited["item"]["text"], "Verify exact results");
        assert_eq!(edited["item"]["done"], false);

        let buddy = service.list("Buddy").unwrap();
        assert_eq!(buddy["lists"].as_array().unwrap().len(), 1);
        assert_eq!(buddy["lists"][0]["items"][0]["done"], false);
        assert_eq!(
            service.list("Alex").unwrap()["lists"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(refine_dir.join(TODO_LISTS_FILE).exists());

        service.delete_item("Buddy", &list_id, &item_id).unwrap();
        assert!(
            service.list("Buddy").unwrap()["lists"][0]["items"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        service.delete_list("Buddy", &list_id).unwrap();
        assert!(
            service.list("Buddy").unwrap()["lists"]
                .as_array()
                .unwrap()
                .is_empty()
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn todo_mutations_enforce_reporter_ownership_and_valid_input() {
        let temp_root = unique_temp_dir("todo-service-validation");
        let service = FileTodoService::new(temp_root.join(".refine"));
        let created = service.create_list("Buddy", "Personal").unwrap();
        let list_id = created["list"]["id"].as_str().unwrap();

        assert!(service.add_item("Alex", list_id, "No access").is_err());
        assert!(service.create_list("Buddy", " personal ").is_err());
        assert!(service.create_list("", "Missing reporter").is_err());
        assert!(
            service
                .update_item("Buddy", list_id, "missing", None, None)
                .is_err()
        );

        service.create_list("Alex", "Personal").unwrap();
        service.reassign_reporter("Buddy", "Alex").unwrap();
        let merged = service.list("Alex").unwrap();
        assert_eq!(merged["lists"].as_array().unwrap().len(), 2);
        assert_eq!(merged["lists"][0]["name"], "Personal (2)");

        fs::remove_dir_all(temp_root).unwrap();
    }

    fn unique_temp_dir(label: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("refine-{label}-{stamp}"))
    }
}

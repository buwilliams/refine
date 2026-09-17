use super::*;
impl FilePlanningService {
    pub(super) fn migrate(&self, a: &mut PlanningAction) -> RefineResult<()> {
        use fs2::FileExt;
        fs::create_dir_all(&self.root).map_err(io)?;
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.root.join(".todo-lists.lock"))
            .map_err(io)?;
        lock.lock_exclusive().map_err(io)?;
        let receipt = self.root.join("planning/migration.json");
        if receipt.exists() {
            a.result = read_json(&receipt)?;
            a.state = "complete".into();
            return Ok(());
        }
        let legacy = self.root.join(crate::application::todos::TODO_LISTS_FILE);
        let store: Value = if legacy.exists() {
            read_json(&legacy)?
        } else {
            json!({"lists":[]})
        };
        let mut boards = 0;
        let mut cards = 0;
        let lists = store["lists"].as_array().ok_or_else(|| {
            invalid("Legacy Todo lists must be an array; repair the source before importing")
        })?;
        for list in lists {
            let old = text(list, "id")?;
            let bid = format!("legacy-{}", &stable_id(&old)[..24]);
            let open = format!("{bid}-open");
            let done = format!("{bid}-done");
            let b = Board {
                id: bid.clone(),
                name: text(list, "name")?,
                revision: 1,
                archived: false,
                routing: None,
                lanes: vec![
                    Lane {
                        id: open.clone(),
                        name: "Open".into(),
                        action: LaneAction::None,
                        routing: None,
                    },
                    Lane {
                        id: done.clone(),
                        name: "Done".into(),
                        action: LaneAction::None,
                        routing: None,
                    },
                ],
                created: list["created"].as_str().unwrap_or(&a.created).into(),
                reporter: list["reporter"].as_str().map(str::to_string),
                last_request_id: format!("migration:{old}"),
            };
            if !self.path("boards", &bid)?.exists() {
                write_json(&self.path("boards", &bid)?, &b)?;
            }
            let items = list["items"].as_array().ok_or_else(|| {
                invalid("Legacy Todo items must be an array; repair the source before importing")
            })?;
            for (index, item) in items.iter().enumerate() {
                let source = text(item, "id")?;
                let id = format!(
                    "PLN{}",
                    stable_id(&format!("todo:{old}:{source}"))[..24].to_uppercase()
                );
                self.work().import_planning_goal(
                    &id,
                    &text(item, "text")?,
                    list["reporter"].as_str(),
                    item,
                    &old,
                    &source,
                )?;
                let p = Placement {
                    goal_id: id.clone(),
                    board_id: bid.clone(),
                    lane_id: if item["done"] == true {
                        done.clone()
                    } else {
                        open.clone()
                    },
                    position: index as f64,
                    revision: 1,
                    archived: false,
                    routing: None,
                    last_request_id: format!("migration:{source}"),
                };
                if !self.path("cards", &id)?.exists() {
                    write_json(&self.path("cards", &id)?, &p)?;
                }
                cards += 1;
            }
            boards += 1;
        }
        let result = json!({"version":1,"boards":boards,"cards":cards,"at":now(),"node_id":self.node,"source":crate::application::todos::TODO_LISTS_FILE});
        write_json(&receipt, &result)?;
        a.result = result;
        a.state = "complete".into();
        Ok(())
    }
}

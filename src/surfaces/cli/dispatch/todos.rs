use super::*;

pub(super) fn dispatch_command(command: Commands) -> RefineResult<()> {
    match command {
        Commands::Todo { action } => dispatch_todo(action),
        _ => unreachable!("command family was routed incorrectly"),
    }
}

pub(super) fn dispatch_todo(action: TodoAction) -> RefineResult<()> {
    let response = match action {
        TodoAction::List {
            reporter,
            target_root,
        } => match target_root {
            Some(target_root) => {
                FileTodoService::new(refine_dir_for_target_root(&target_root)?).list(&reporter)?
            }
            None => daemon_json(
                "GET",
                &format!("/todos?reporter={}", query_component(&reporter)),
                None,
            )?,
        },
        TodoAction::CreateList {
            name,
            reporter,
            target_root,
        } => match target_root {
            Some(target_root) => FileTodoService::new(refine_dir_for_target_root(&target_root)?)
                .create_list(&reporter, &name)?,
            None => daemon_json(
                "POST",
                "/todos/lists",
                Some(json!({
                    "reporter": reporter,
                    "name": name
                })),
            )?,
        },
        TodoAction::RenameList {
            list_id,
            name,
            reporter,
            target_root,
        } => match target_root {
            Some(target_root) => FileTodoService::new(refine_dir_for_target_root(&target_root)?)
                .rename_list(&reporter, &list_id, &name)?,
            None => daemon_json(
                "PATCH",
                &format!("/todos/lists/{}", path_segment(&list_id)),
                Some(json!({
                    "reporter": reporter,
                    "name": name
                })),
            )?,
        },
        TodoAction::DeleteList {
            list_id,
            reporter,
            target_root,
        } => match target_root {
            Some(target_root) => FileTodoService::new(refine_dir_for_target_root(&target_root)?)
                .delete_list(&reporter, &list_id)?,
            None => daemon_json(
                "DELETE",
                &format!("/todos/lists/{}", path_segment(&list_id)),
                Some(json!({ "reporter": reporter })),
            )?,
        },
        TodoAction::Add {
            list_id,
            text,
            reporter,
            target_root,
        } => match target_root {
            Some(target_root) => FileTodoService::new(refine_dir_for_target_root(&target_root)?)
                .add_item(&reporter, &list_id, &text)?,
            None => daemon_json(
                "POST",
                &format!("/todos/lists/{}/items", path_segment(&list_id)),
                Some(json!({
                    "reporter": reporter,
                    "text": text
                })),
            )?,
        },
        TodoAction::Edit {
            list_id,
            item_id,
            text,
            reporter,
            target_root,
        } => match target_root {
            Some(target_root) => FileTodoService::new(refine_dir_for_target_root(&target_root)?)
                .update_item(&reporter, &list_id, &item_id, Some(&text), None)?,
            None => daemon_json(
                "PATCH",
                &format!(
                    "/todos/lists/{}/items/{}",
                    path_segment(&list_id),
                    path_segment(&item_id)
                ),
                Some(json!({
                    "reporter": reporter,
                    "text": text
                })),
            )?,
        },
        TodoAction::Delete {
            list_id,
            item_id,
            reporter,
            target_root,
        } => match target_root {
            Some(target_root) => FileTodoService::new(refine_dir_for_target_root(&target_root)?)
                .delete_item(&reporter, &list_id, &item_id)?,
            None => daemon_json(
                "DELETE",
                &format!(
                    "/todos/lists/{}/items/{}",
                    path_segment(&list_id),
                    path_segment(&item_id)
                ),
                Some(json!({ "reporter": reporter })),
            )?,
        },
        TodoAction::Done {
            list_id,
            item_id,
            reporter,
            target_root,
        } => dispatch_todo_done(target_root, &reporter, &list_id, &item_id, true)?,
        TodoAction::Undo {
            list_id,
            item_id,
            reporter,
            target_root,
        } => dispatch_todo_done(target_root, &reporter, &list_id, &item_id, false)?,
    };
    print_json(&response);
    Ok(())
}

pub(super) fn dispatch_todo_done(
    target_root: Option<PathBuf>,
    reporter: &str,
    list_id: &str,
    item_id: &str,
    done: bool,
) -> RefineResult<Value> {
    match target_root {
        Some(target_root) => FileTodoService::new(refine_dir_for_target_root(&target_root)?)
            .update_item(reporter, list_id, item_id, None, Some(done)),
        None => daemon_json(
            "PATCH",
            &format!(
                "/todos/lists/{}/items/{}",
                path_segment(list_id),
                path_segment(item_id)
            ),
            Some(json!({
                "reporter": reporter,
                "done": done
            })),
        ),
    }
}

use super::*;

pub(super) fn dispatch_command(command: Commands) -> RefineResult<()> {
    match command {
        Commands::Next {
            target_root: Some(target_root),
        } => {
            let next = FileNextActionsService::new(refine_dir_for_target_root(&target_root)?)
                .next_response()?;
            println!("{}", serde_json::to_string_pretty(&next).unwrap());
            Ok(())
        }
        Commands::Next { target_root: None } => {
            let next = daemon_json("GET", "/guidance/next", None)?;
            print_json(&next);
            Ok(())
        }
        Commands::Commands => {
            print_json(&super::super::catalog::commands_catalog());
            Ok(())
        }
        _ => unreachable!("command family was routed incorrectly"),
    }
}

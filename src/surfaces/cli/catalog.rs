//! Machine-readable CLI catalog.
//!
//! `refine commands` emits the full command tree — names, descriptions, and
//! arguments — as JSON so an agent can load the entire current operational
//! vocabulary in one call instead of relying on a committed reference snapshot
//! or exploring `--help` per subcommand.

use clap::CommandFactory;
use serde_json::{Value, json};

use super::actions::Cli;

pub fn commands_catalog() -> Value {
    let command = Cli::command();
    json!({
        "product": "refine",
        "version": env!("CARGO_PKG_VERSION"),
        "commands": subcommand_values(&command),
        "hints": {
            "next": "Run `refine next` for state-aware suggestions of what to do now.",
            "runbooks": "Task-oriented guides live in docs/runbooks/.",
            "api": "Every command maps onto the daemon HTTP API; discover route groups with `refine system api-groups`."
        }
    })
}

fn subcommand_values(command: &clap::Command) -> Vec<Value> {
    command
        .get_subcommands()
        .filter(|subcommand| subcommand.get_name() != "help")
        .map(command_value)
        .collect()
}

fn command_value(command: &clap::Command) -> Value {
    let mut value = json!({
        "name": command.get_name(),
        "about": about_text(command),
    });
    let arguments: Vec<Value> = command
        .get_arguments()
        .filter(|argument| !argument.is_hide_set() && argument.get_id() != "help")
        .map(argument_value)
        .collect();
    if !arguments.is_empty() {
        value["arguments"] = json!(arguments);
    }
    let subcommands = subcommand_values(command);
    if !subcommands.is_empty() {
        value["subcommands"] = json!(subcommands);
    }
    value
}

fn argument_value(argument: &clap::Arg) -> Value {
    json!({
        "name": argument.get_id().as_str(),
        "flag": argument.get_long().map(|long| format!("--{long}")),
        "positional": argument.is_positional(),
        "required": argument.is_required_set(),
        "help": argument
            .get_help()
            .map(|help| help.to_string())
            .unwrap_or_default(),
    })
}

fn about_text(command: &clap::Command) -> String {
    command
        .get_long_about()
        .or_else(|| command.get_about())
        .map(|about| about.to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    #[test]
    fn catalog_lists_fleet_commands_with_descriptions() {
        let catalog = commands_catalog();
        let commands = catalog["commands"].as_array().unwrap();
        let fleet = commands
            .iter()
            .find(|command| command["name"] == "fleet")
            .expect("fleet command present");
        let names: Vec<&str> = fleet["subcommands"]
            .as_array()
            .unwrap()
            .iter()
            .map(|subcommand| subcommand["name"].as_str().unwrap())
            .collect();
        for expected in ["distribute", "sync", "run"] {
            assert!(names.contains(&expected), "missing {expected}: {names:?}");
        }
        assert!(commands.iter().any(|command| command["name"] == "next"));
    }

    #[test]
    fn catalog_lists_the_complete_todo_command_family() {
        let catalog = commands_catalog();
        let todo = catalog["commands"]
            .as_array()
            .unwrap()
            .iter()
            .find(|command| command["name"] == "todo")
            .expect("todo command present");
        let names: Vec<&str> = todo["subcommands"]
            .as_array()
            .unwrap()
            .iter()
            .map(|subcommand| subcommand["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "list",
                "create-list",
                "rename-list",
                "delete-list",
                "add",
                "edit",
                "delete",
                "done",
                "undo"
            ]
        );
        for subcommand in todo["subcommands"].as_array().unwrap() {
            assert!(
                subcommand["arguments"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|argument| {
                        argument["name"] == "reporter" && argument["required"] == true
                    }),
                "missing required Reporter context: {subcommand:#}"
            );
        }
    }

    #[test]
    fn catalog_describes_workflow_pause_and_resume_contract() {
        let catalog = commands_catalog();
        let workflow = catalog["commands"]
            .as_array()
            .unwrap()
            .iter()
            .find(|command| command["name"] == "workflow")
            .expect("workflow command present");
        let subcommands = workflow["subcommands"].as_array().unwrap();
        let pause = subcommands
            .iter()
            .find(|command| command["name"] == "pause")
            .unwrap()["about"]
            .as_str()
            .unwrap();
        let resume = subcommands
            .iter()
            .find(|command| command["name"] == "resume")
            .unwrap()["about"]
            .as_str()
            .unwrap();
        for phrase in [
            "new Goal admission",
            "automatic Git sync",
            "inactive-worktree cleanup",
        ] {
            assert!(pause.contains(phrase), "pause missing {phrase}: {pause}");
            assert!(resume.contains(phrase), "resume missing {phrase}: {resume}");
        }
        assert!(pause.contains("Active Goal executions continue"));
        assert!(resume.contains("eligible again"));
        assert!(resume.contains("remain uninterrupted"));
    }

    #[test]
    fn operational_runbooks_use_current_cli_and_layout_contracts() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let runbooks_dir = root.join("docs/runbooks");
        let mut paths = fs::read_dir(&runbooks_dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("md"))
            .collect::<Vec<_>>();
        paths.sort();

        let cli = Cli::command();
        let index = fs::read_to_string(root.join("docs/runbooks/README.md")).unwrap();
        let mut all = String::new();
        for path in paths {
            let document = fs::read_to_string(&path).unwrap();
            validate_documented_refine_commands(&cli, &path, &document);
            validate_local_markdown_links(&path, &document);
            if path.parent() == Some(runbooks_dir.as_path())
                && path.file_name().and_then(|name| name.to_str()) != Some("README.md")
            {
                let file_name = path.file_name().unwrap().to_string_lossy();
                assert!(
                    index.contains(&format!("({file_name})")),
                    "docs/runbooks/README.md does not list {file_name}"
                );
            }
            all.push_str(&document);
            all.push('\n');
        }
        for retired in [
            "refine daemon ",
            "./r daemon ",
            "refine cluster ",
            "./r cluster ",
            "\nrefine status\n",
            "\n./r status\n",
        ] {
            assert!(
                !all.contains(retired),
                "operational runbooks still contain retired CLI form {retired:?}"
            );
        }
        assert!(
            !all.contains("Refine `4.0.0`"),
            "migration guidance must use the installed v4 version"
        );
        assert!(
            !all.contains("$RUNTIME_ROOT\"/*/cache"),
            "node migration cleanup must be scoped to one validated port runtime"
        );
        assert!(all.contains("--path-format=absolute --git-common-dir"));
        assert!(all.contains("runtime/goals/<shard>/<id>/logs.jsonl"));
    }

    fn validate_documented_refine_commands(cli: &clap::Command, path: &Path, document: &str) {
        for (line_index, line) in document.lines().enumerate() {
            let line = line.trim();
            let line_tokens = line.split_whitespace().collect::<Vec<_>>();
            let Some(command_index) = line_tokens
                .iter()
                .position(|token| matches!(*token, "refine" | "./r"))
            else {
                continue;
            };
            let tokens = &line_tokens[command_index + 1..];
            let Some(group_name) = tokens.first().copied() else {
                continue;
            };
            if group_name.starts_with('-') {
                continue;
            }
            let group_name = group_name.trim_matches(|character: char| {
                !character.is_ascii_alphanumeric() && !matches!(character, '-' | '_')
            });
            let group = cli.find_subcommand(group_name).unwrap_or_else(|| {
                panic!(
                    "{}:{} documents unknown Refine command group {group_name}",
                    path.display(),
                    line_index + 1
                )
            });
            let mut documented_command = group;
            let mut argument_start = 1;
            if group.has_subcommands() {
                let subcommand_name = tokens
                    .get(1)
                    .copied()
                    .filter(|token| !token.starts_with('-'))
                    .map(|token| {
                        token.trim_matches(|character: char| {
                            !character.is_ascii_alphanumeric() && !matches!(character, '-' | '_')
                        })
                    })
                    .unwrap_or_else(|| {
                        panic!(
                            "{}:{} omits a subcommand after {group_name}",
                            path.display(),
                            line_index + 1
                        )
                    });
                documented_command = group.find_subcommand(subcommand_name).unwrap_or_else(|| {
                    panic!(
                        "{}:{} documents unknown command refine {group_name} {subcommand_name}",
                        path.display(),
                        line_index + 1
                    )
                });
                argument_start = 2;
            }
            for token in &tokens[argument_start..] {
                let token = token.trim_matches(|character: char| {
                    !character.is_ascii_alphanumeric() && !matches!(character, '-' | '_' | '=')
                });
                let Some(flag) = token
                    .strip_prefix("--")
                    .map(|flag| flag.split('=').next().unwrap_or(flag))
                else {
                    continue;
                };
                assert!(
                    documented_command
                        .get_arguments()
                        .any(|argument| argument.get_long() == Some(flag)),
                    "{}:{} documents unknown flag --{flag} for refine {group_name}",
                    path.display(),
                    line_index + 1
                );
            }
        }
    }

    fn validate_local_markdown_links(path: &Path, document: &str) {
        for suffix in document.split("](").skip(1) {
            let Some(destination) = suffix.split(')').next() else {
                continue;
            };
            let destination = destination.split('#').next().unwrap_or(destination);
            if destination.is_empty()
                || destination.starts_with('#')
                || destination.contains("://")
                || destination.starts_with("mailto:")
            {
                continue;
            }
            let linked = path.parent().unwrap().join(destination);
            assert!(
                linked.exists(),
                "{} links to missing local path {destination}",
                path.display()
            );
        }
    }
}

//! Source-boundary checks for the semantic crate roots.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

fn source_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn rust_files(root: &Path) -> Vec<PathBuf> {
    fn visit(directory: &Path, files: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(&path, files);
            } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
                files.push(path);
            }
        }
    }

    let mut files = Vec::new();
    visit(root, &mut files);
    files.sort();
    files
}

fn assert_sources_exclude(root: &Path, forbidden: &[&str]) {
    for path in rust_files(root) {
        if path.file_name().and_then(|name| name.to_str()) == Some("architecture_tests.rs") {
            continue;
        }
        let source = fs::read_to_string(&path).unwrap();
        for needle in forbidden {
            assert!(
                !source.contains(needle),
                "{} contains forbidden dependency {needle}",
                path.display()
            );
        }
    }
}

#[test]
fn crate_exports_only_the_semantic_roots_and_error_boundary() {
    let root = source_root();
    let lib = fs::read_to_string(root.join("lib.rs")).unwrap();
    let exported = lib
        .lines()
        .filter_map(|line| line.trim().strip_prefix("pub mod "))
        .filter_map(|line| line.strip_suffix(';'))
        .collect::<BTreeSet<_>>();
    assert_eq!(
        exported,
        BTreeSet::from([
            "application",
            "error",
            "infrastructure",
            "model",
            "surfaces"
        ])
    );
    assert!(lib.contains("#[cfg(test)]\nmod architecture_tests;"));

    let directories = fs::read_dir(&root)
        .unwrap()
        .filter_map(|entry| {
            let entry = entry.unwrap();
            entry
                .file_type()
                .unwrap()
                .is_dir()
                .then(|| entry.file_name().to_string_lossy().into_owned())
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        directories,
        BTreeSet::from([
            "application".to_string(),
            "infrastructure".to_string(),
            "model".to_string(),
            "surfaces".to_string(),
        ])
    );
}

#[test]
fn planned_capability_directories_exist() {
    let root = source_root();
    let required = [
        "application/agent_io/contracts",
        "application/agent_io/prompt_transport",
        "application/agent_io/prompts",
        "application/agent_io/structured_output",
        "application/agents/provider_selection",
        "application/agents/sessions",
        "application/diagnostics/processes",
        "application/diagnostics/support_bundle",
        "application/fleet/node_init",
        "application/fleet/node_sync",
        "application/fleet/nodes",
        "application/fleet/service",
        "application/maintenance/worktrees",
        "application/operations/process_control",
        "application/persistence_sync/conflict_reports",
        "application/persistence_sync/health",
        "application/persistence_sync/recovery",
        "application/persistence_sync/resolution",
        "application/persistence_sync/state",
        "application/persistence_sync/state_merge",
        "application/projects/migration",
        "application/projects/projection",
        "application/projects/registry",
        "application/system/daemon_lifecycle",
        "application/system/installation",
        "application/system/release",
        "application/system/runtime_status",
        "application/system/source_promotion",
        "application/system/target_apps",
        "application/workflow/agents",
        "application/workflow/engine/behaviors",
        "application/workflow/engine/context",
        "application/workflow/engine/execution",
        "application/workflow/engine/policy",
        "application/workflow/engine/scheduling",
        "application/workflow/phases/implementation_planning",
        "application/workflow/phases/quality",
        "application/workflow/governance",
        "application/workflow/recovery/candidate_handoff",
        "application/workflow/recovery/candidate_refresh",
        "application/workflow/recovery/failure_settlement",
        "application/workflow/recovery/quality",
        "application/workflow/recovery/reconciliation",
        "infrastructure/agents/discovery",
        "infrastructure/agents/invocation",
        "infrastructure/agents/output_parser",
        "infrastructure/git/ancestry",
        "infrastructure/git/locks",
        "infrastructure/git/merge",
        "infrastructure/git/refs",
        "infrastructure/git/repository",
        "infrastructure/git/worktrees",
        "infrastructure/observability/activity",
        "infrastructure/observability/logs",
        "infrastructure/observability/metrics",
        "infrastructure/process/agent_env",
        "infrastructure/process/launch_environment",
        "infrastructure/process/subprocess",
        "infrastructure/process/supervisor/config",
        "infrastructure/process/supervisor/coordination",
        "infrastructure/process/supervisor/lifecycle",
        "infrastructure/process/supervisor/operations",
        "infrastructure/process/supervisor/runtime",
        "infrastructure/process/supervisor/security",
        "infrastructure/runtime/checkout",
        "infrastructure/runtime/host_resources",
        "infrastructure/storage/project_layout",
        "surfaces/cli",
        "surfaces/mcp",
        "surfaces/web/static",
        "surfaces/web_server",
        "surfaces/website",
    ];
    for relative in required {
        assert!(root.join(relative).is_dir(), "missing src/{relative}");
    }
}

#[test]
fn model_has_no_runtime_or_adapter_dependencies() {
    assert_sources_exclude(
        &source_root().join("model"),
        &[
            "crate::application",
            "crate::infrastructure",
            "crate::surfaces",
        ],
    );
}

#[test]
fn non_surface_code_does_not_import_surfaces() {
    let root = source_root();
    for semantic_root in ["model", "application", "infrastructure"] {
        assert_sources_exclude(
            &root.join(semantic_root),
            &["crate::surfaces", "refine::surfaces"],
        );
    }
}

#[test]
fn retired_and_generic_root_namespaces_are_absent() {
    let root = source_root();
    assert_sources_exclude(
        &root,
        &[
            "crate::process::",
            "crate::prompts::",
            "crate::structured_output::",
            "crate::tools::",
            "crate::workflow::",
            "refine::process::",
            "refine::prompts::",
            "refine::structured_output::",
            "refine::tools::",
            "refine::workflow::",
        ],
    );

    for removed in [
        "process",
        "prompts",
        "structured_output",
        "tools",
        "workflow",
    ] {
        assert!(
            !root.join(removed).exists(),
            "retired src/{removed} root exists"
        );
    }

    let root_catalogs = [
        root.join("lib.rs"),
        root.join("application/mod.rs"),
        root.join("infrastructure/mod.rs"),
        root.join("model/mod.rs"),
        root.join("surfaces/mod.rs"),
    ];
    for catalog in root_catalogs {
        let source = fs::read_to_string(&catalog).unwrap();
        for generic in ["tools", "product", "host", "actions", "lib"] {
            assert!(
                !source.contains(&format!("pub mod {generic};")),
                "{} reintroduces generic namespace {generic}",
                catalog.display()
            );
        }
    }
}

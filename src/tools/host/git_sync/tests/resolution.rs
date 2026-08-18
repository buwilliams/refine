use super::*;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use crate::tools::git::resolve::{
    CONTENTION_ATTEMPT_LIMIT, ResolutionRequest, ResolverOutcome, ScriptedResolver, ScriptedStep,
    install_resolver_override,
};

const SHARED_STATE: &str = "shared/state.json";
const SHARED_TREE_PATH: &str = ".refine/shared/state.json";
const SECOND_STATE: &str = "second/state.json";

fn write_shared(root: &Path, bytes: &str) {
    write_record(root, SHARED_STATE, bytes);
}

fn write_record(root: &Path, relative: &str, bytes: &str) {
    let destination = refine_dir_for_target_root(root).unwrap().join(relative);
    fs::create_dir_all(destination.parent().unwrap()).unwrap();
    fs::write(destination, bytes).unwrap();
}

fn read_shared(root: &Path) -> String {
    fs::read_to_string(refine_dir_for_target_root(root).unwrap().join(SHARED_STATE)).unwrap()
}

/// Shared baseline on both nodes, then the same record changed differently on
/// each: node A's version is published (remote side), node B's stays live.
fn contested_fixture(name: &str) -> SyncFixture {
    let fixture = SyncFixture::new(name);
    write_shared(&fixture.a, "{\"node\":\"base\"}\n");
    fixture.service(&fixture.a).sync().unwrap();
    fixture.service(&fixture.b).sync().unwrap();
    write_shared(&fixture.a, "{\"node\":\"remote\"}\n");
    fixture.service(&fixture.a).sync().unwrap();
    write_shared(&fixture.b, "{\"node\":\"live\"}\n");
    fixture
}

fn completing(edit: impl FnMut(&ResolutionRequest<'_>) + Send + 'static) -> ScriptedStep {
    let mut edit = edit;
    Box::new(move |request| {
        edit(request);
        Ok(ResolverOutcome::Completed)
    })
}

fn resolve_refs(root: &Path) -> String {
    git_stdout(root, &["for-each-ref", "refs/refine/resolve"])
}

fn contention_refs(root: &Path) -> Vec<String> {
    git_stdout(
        root,
        &[
            "for-each-ref",
            "--format=%(refname)",
            "refs/refine/contention",
        ],
    )
    .lines()
    .map(str::to_string)
    .collect()
}

/// A resolver step that counts the call and answers the same way every time —
/// the spend meter every contention scenario reads.
fn counting(calls: &Arc<AtomicU32>, answer: fn() -> ResolverOutcome) -> ScriptedStep {
    let counted = Arc::clone(calls);
    Box::new(move |_request: &ResolutionRequest<'_>| {
        counted.fetch_add(1, Ordering::SeqCst);
        Ok(answer())
    })
}

/// A script of `count` identical counted steps: more than any bounded run may
/// spend, so an unbounded one is measured rather than cut short.
fn counting_script(
    calls: &Arc<AtomicU32>,
    count: usize,
    answer: fn() -> ResolverOutcome,
) -> Vec<ScriptedStep> {
    (0..count).map(|_| counting(calls, answer)).collect()
}

#[test]
fn an_engaged_resolver_settles_a_contested_divergence_end_to_end() {
    let fixture = contested_fixture("resolve-clean");
    let service = fixture.service(&fixture.b);
    let _guard = install_resolver_override(
        &fixture.b,
        Arc::new(ScriptedResolver::new(vec![completing(|request| {
            assert!(
                request.context.contains(SHARED_STATE),
                "the prompt must carry the contested record"
            );
            fs::write(
                request.workspace_dir.join(SHARED_TREE_PATH),
                "{\"node\":\"merged\"}\n",
            )
            .unwrap();
        })])),
    );

    let result = service.sync().unwrap();

    assert!(result.ok && result.pushed, "{result:#?}");
    assert_eq!(read_shared(&fixture.b), "{\"node\":\"merged\"}\n");
    // The published head is a two-parent merge commit: both displaced sides
    // stay reachable as parents.
    let parents = git_stdout(
        &fixture.b,
        &["rev-list", "--parents", "-n", "1", "refine/state"],
    );
    assert_eq!(parents.split_whitespace().count(), 3, "{parents}");
    // The bounded resolve namespace is retired and the settled conflict
    // report is cleared.
    assert!(resolve_refs(&fixture.b).is_empty());
    assert!(
        latest_state_sync_conflict_report(&fixture.b.join("run"))
            .unwrap()
            .is_none()
    );
    // The other node converges onto the resolved content.
    fixture.service(&fixture.a).sync().unwrap();
    assert_eq!(read_shared(&fixture.a), "{\"node\":\"merged\"}\n");
}

#[test]
fn a_needs_decision_outcome_rewrites_the_report_with_the_question() {
    let fixture = contested_fixture("resolve-needs-decision");
    let service = fixture.service(&fixture.b);
    let _guard = install_resolver_override(
        &fixture.b,
        Arc::new(ScriptedResolver::new(vec![
            Box::new(|_request: &ResolutionRequest<'_>| {
                Ok(ResolverOutcome::Failed("no basis to choose".to_string()))
            }) as ScriptedStep,
            Box::new(|_request: &ResolutionRequest<'_>| {
                Ok(ResolverOutcome::Failed("still no basis".to_string()))
            }) as ScriptedStep,
        ])),
    );

    let error = service.sync().unwrap_err();

    let message = error.to_string();
    assert!(message.contains("needs a decision"), "{message}");
    assert!(message.contains(SHARED_STATE), "{message}");
    assert!(message.contains("--authority"), "{message}");
    let report = latest_state_sync_conflict_report(&fixture.b.join("run"))
        .unwrap()
        .expect("the escalation keeps the conflict report");
    let question = report
        .decision_question
        .expect("the report's headline is the domain-terms question");
    assert!(question.contains(SHARED_STATE), "{question}");
    assert!(question.contains("--authority"), "{question}");
    // The escalated resolution holds no result; its refs are retired.
    assert!(resolve_refs(&fixture.b).is_empty());
    // The divergence itself is untouched: heads did not move.
    assert_eq!(read_shared(&fixture.b), "{\"node\":\"live\"}\n");
}

/// The resolver runs unlocked, so a second operation — the daemon's sync
/// worker beside an operator-triggered sync — can reach the same divergence
/// mid-resolution. It must defer to the one already resolving instead of
/// re-deriving that workspace underneath it.
#[test]
fn a_second_operation_defers_to_the_resolution_already_running() {
    let fixture = contested_fixture("resolve-concurrent");
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let _guard = install_resolver_override(
        &fixture.b,
        Arc::new(ScriptedResolver::new(vec![completing(move |request| {
            entered_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(30)).unwrap();
            fs::write(
                request.workspace_dir.join(SHARED_TREE_PATH),
                "{\"node\":\"merged\"}\n",
            )
            .unwrap();
        })])),
    );

    let resolving_root = fixture.b.clone();
    let resolving = std::thread::spawn(move || {
        FileGitSyncService::new(&resolving_root, resolving_root.join("run")).sync()
    });
    entered_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("the resolver must be reached unlocked");

    let second = fixture.service(&fixture.b).sync().unwrap();

    assert!(second.deferred, "{second:#?}");
    assert!(
        second
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("Another operation is resolving"),
        "{second:#?}"
    );
    release_tx.send(()).unwrap();
    let first = resolving.join().unwrap().unwrap();
    // The resolution that owned the divergence published it, and the report
    // the deferred pass left untouched is the one it settles.
    assert!(first.ok && first.pushed, "{first:#?}");
    assert_eq!(read_shared(&fixture.b), "{\"node\":\"merged\"}\n");
    assert!(
        latest_state_sync_conflict_report(&fixture.b.join("run"))
            .unwrap()
            .is_none()
    );
    assert!(resolve_refs(&fixture.b).is_empty());
}

/// An escalation stands until something answers it. Re-running sync over an
/// unchanged divergence must not spend the agent again — there is no new
/// evidence for it to read — and must not replace the question it already
/// asked with the generic headline.
#[test]
fn a_standing_escalation_is_neither_re_asked_nor_overwritten() {
    const QUESTION: &str = "shared/state.json: this node calls it live and another calls it remote; which reading holds?";
    let fixture = contested_fixture("resolve-escalation-stands");
    let service = fixture.service(&fixture.b);
    let calls = Arc::new(AtomicU32::new(0));
    let counted = Arc::clone(&calls);
    let _guard = install_resolver_override(
        &fixture.b,
        Arc::new(ScriptedResolver::new(vec![
            Box::new(move |_request: &ResolutionRequest<'_>| {
                counted.fetch_add(1, Ordering::SeqCst);
                Ok(ResolverOutcome::NeedsDecision {
                    question: QUESTION.to_string(),
                })
            }) as ScriptedStep,
        ])),
    );

    let first = service.sync().unwrap_err().to_string();
    let second = service.sync().unwrap_err().to_string();

    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the unchanged divergence must not be handed to the agent again"
    );
    assert!(first.contains(QUESTION), "{first}");
    assert!(second.contains(QUESTION), "{second}");
    let report = latest_state_sync_conflict_report(&fixture.b.join("run"))
        .unwrap()
        .expect("the escalation keeps its report");
    assert_eq!(report.decision_question.as_deref(), Some(QUESTION));
    assert!(resolve_refs(&fixture.b).is_empty());
}

/// The node-level opt-out disengages the call-out itself, restoring the
/// fail-closed report the ladder produced before agent resolution existed.
#[test]
fn the_node_opt_out_disengages_agent_resolution() {
    let fixture = contested_fixture("resolve-opt-out");
    let service = fixture.service(&fixture.b).with_agent_resolution();
    assert!(
        service.state_conflict_resolver().is_some(),
        "agent resolution is on by default"
    );

    let refine_dir = refine_dir_for_target_root(&fixture.b).unwrap();
    FileSettingsService::with_active_root(&refine_dir, fixture.b.join("run"))
        .update(&serde_json::json!({"state_sync_agent_resolution": "off"}))
        .unwrap();

    assert!(service.state_conflict_resolver().is_none());
    let error = service.sync().unwrap_err();
    assert!(matches!(error, RefineError::Conflict(_)), "{error}");
    let report = latest_state_sync_conflict_report(&fixture.b.join("run"))
        .unwrap()
        .expect("the fail-closed conflict report is recorded");
    assert_eq!(report.decision_question, None);
    assert!(resolve_refs(&fixture.b).is_empty());
}

/// The escalated question is what the preview — and through it the dashboard
/// — shows an operator, so it must survive the round trip through the report.
#[test]
fn the_preview_surfaces_the_escalated_question() {
    let fixture = contested_fixture("resolve-preview-question");
    let service = fixture.service(&fixture.b);
    let _guard = install_resolver_override(
        &fixture.b,
        Arc::new(ScriptedResolver::new(vec![
            Box::new(|_request: &ResolutionRequest<'_>| {
                Ok(ResolverOutcome::NeedsDecision {
                    question: "which node's shared/state.json reading holds?".to_string(),
                })
            }) as ScriptedStep,
        ])),
    );
    service.sync().unwrap_err();

    let preview = service.preview_state_recovery().unwrap();

    assert_eq!(
        preview.decision_question.as_deref(),
        Some("which node's shared/state.json reading holds?"),
        "{preview:#?}"
    );
}

#[test]
fn an_unavailable_resolver_keeps_the_fail_closed_conflict_verbatim() {
    let fixture = contested_fixture("resolve-unavailable");
    let service = fixture.service(&fixture.b);
    let calls = Arc::new(AtomicU32::new(0));
    let counted = Arc::clone(&calls);
    let _guard = install_resolver_override(
        &fixture.b,
        Arc::new(ScriptedResolver::new(vec![
            Box::new(move |_request: &ResolutionRequest<'_>| {
                counted.fetch_add(1, Ordering::SeqCst);
                Ok(ResolverOutcome::Unavailable)
            }) as ScriptedStep,
        ])),
    );

    let error = service.sync().unwrap_err();

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let report = latest_state_sync_conflict_report(&fixture.b.join("run"))
        .unwrap()
        .expect("the fail-closed conflict report is recorded");
    assert_eq!(report.decision_question, None);
    assert!(error.to_string().contains(&report.report_id), "{error}");
    assert!(resolve_refs(&fixture.b).is_empty());
}

#[test]
fn a_disengaged_service_fails_closed_without_invoking_anything() {
    let fixture = contested_fixture("resolve-disengaged");
    // No override, no `with_agent_resolution`: the ladder fails closed and
    // never pins a resolution.
    let error = fixture.service(&fixture.b).sync().unwrap_err();
    assert!(matches!(error, RefineError::Conflict(_)), "{error}");
    assert!(resolve_refs(&fixture.b).is_empty());
    let report = latest_state_sync_conflict_report(&fixture.b.join("run"))
        .unwrap()
        .expect("the conflict report is recorded");
    assert_eq!(report.decision_question, None);
}

#[test]
fn a_raced_resolution_re_derives_once_and_publishes_the_second() {
    let fixture = contested_fixture("resolve-race");
    let service = fixture.service(&fixture.b);
    let calls = Arc::new(AtomicU32::new(0));
    let counted = Arc::clone(&calls);
    let racing_root = fixture.a.clone();
    let _guard = install_resolver_override(
        &fixture.b,
        Arc::new(ScriptedResolver::new(vec![
            completing(move |request| {
                counted.fetch_add(1, Ordering::SeqCst);
                // While the resolver runs UNLOCKED, another node publishes:
                // the pinned remote head goes stale, so this resolution must
                // be discarded and re-derived exactly once.
                write_shared(&racing_root, "{\"node\":\"raced\"}\n");
                FileGitSyncService::new(&racing_root, racing_root.join("run"))
                    .sync()
                    .unwrap();
                fs::write(
                    request.workspace_dir.join(SHARED_TREE_PATH),
                    "{\"node\":\"stale-merge\"}\n",
                )
                .unwrap();
            }),
            completing({
                let counted = Arc::clone(&calls);
                move |request| {
                    counted.fetch_add(1, Ordering::SeqCst);
                    fs::write(
                        request.workspace_dir.join(SHARED_TREE_PATH),
                        "{\"node\":\"final-merge\"}\n",
                    )
                    .unwrap();
                }
            }),
        ])),
    );

    let result = service.sync().unwrap();

    assert!(result.ok && result.pushed, "{result:#?}");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(read_shared(&fixture.b), "{\"node\":\"final-merge\"}\n");
    assert!(resolve_refs(&fixture.b).is_empty());
    fixture.service(&fixture.a).sync().unwrap();
    assert_eq!(read_shared(&fixture.a), "{\"node\":\"final-merge\"}\n");
}

/// A standing escalation survives the node that keeps working. Every pass
/// snapshots live state, so a node still writing records mints a NEW local
/// head — a genuinely new divergence id — while nothing the question asked
/// about has moved: the remote side is unchanged and the same record is still
/// contested. Keyed on the divergence, that churn handed the same unanswered
/// question back to the agent on every pass, forever, and overwrote the
/// agent's words with the generic headline on the way. Keyed on the
/// contention, the question is neither re-asked nor replaced.
#[test]
fn live_writes_do_not_re_ask_a_standing_escalation() {
    const QUESTION: &str = "shared/state.json: this node calls it live and another calls it remote; which reading holds?";
    fn escalate() -> ResolverOutcome {
        ResolverOutcome::NeedsDecision {
            question: QUESTION.to_string(),
        }
    }
    let fixture = contested_fixture("resolve-standing-live-writes");
    let service = fixture.service(&fixture.b);
    let calls = Arc::new(AtomicU32::new(0));
    let _guard = install_resolver_override(
        &fixture.b,
        Arc::new(ScriptedResolver::new(counting_script(&calls, 8, escalate))),
    );

    for pass in 0..5 {
        // The daemon keeps advancing live state while the question stands.
        write_shared(&fixture.b, &format!("{{\"node\":\"live-{pass}\"}}\n"));
        let error = service.sync().unwrap_err().to_string();
        assert!(error.contains(QUESTION), "pass {pass}: {error}");
    }

    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "a live write is not new evidence: the standing question must not be re-asked"
    );
    let report = latest_state_sync_conflict_report(&fixture.b.join("run"))
        .unwrap()
        .expect("the escalation keeps its report");
    assert_eq!(report.decision_question.as_deref(), Some(QUESTION));
    assert!(resolve_refs(&fixture.b).is_empty());
}

/// The contention budget bounds what a divergence with NO standing question
/// can spend. An engagement that answers nothing — an uninstalled agent, a
/// resolution superseded before it published — leaves the escalation guard
/// with nothing to hold, so the per-contended-record budget is what stops a
/// live-writing node from buying an agent attempt every pass. The hold is not
/// a fence: the conflict report stands, and one authority run converges.
#[test]
fn sustained_contention_bounds_resolution_attempts() {
    fn unavailable() -> ResolverOutcome {
        ResolverOutcome::Unavailable
    }
    let fixture = contested_fixture("resolve-contention-budget");
    let service = fixture.service(&fixture.b);
    let calls = Arc::new(AtomicU32::new(0));
    let _guard = install_resolver_override(
        &fixture.b,
        Arc::new(ScriptedResolver::new(counting_script(
            &calls,
            8,
            unavailable,
        ))),
    );

    for pass in 0..6 {
        write_shared(&fixture.b, &format!("{{\"node\":\"live-{pass}\"}}\n"));
        service.sync().unwrap_err();
    }

    assert_eq!(
        calls.load(Ordering::SeqCst),
        CONTENTION_ATTEMPT_LIMIT,
        "one contended record buys a bounded number of attempts against one remote head"
    );
    // Bounded durable accounting: one ref per attempt bought, and nothing
    // else left behind.
    assert_eq!(
        contention_refs(&fixture.b).len(),
        CONTENTION_ATTEMPT_LIMIT as usize
    );
    assert!(resolve_refs(&fixture.b).is_empty());
    // The hold leaves the question — here the fail-closed headline — on file
    // and the operator's answer immediate.
    assert!(
        latest_state_sync_conflict_report(&fixture.b.join("run"))
            .unwrap()
            .is_some()
    );
    let run = service
        .run_state_recovery(StateRecoveryDecision::uniform(StateRecoveryAuthority::Live))
        .unwrap();
    assert!(run.ok && run.recovered, "{run:#?}");
    assert_eq!(read_shared(&fixture.b), "{\"node\":\"live-5\"}\n");
    fixture.service(&fixture.a).sync().unwrap();
    assert_eq!(read_shared(&fixture.a), "{\"node\":\"live-5\"}\n");
}

/// The one property that separates a budget from a fence: the budget is per
/// contended RECORD, so a record newly dragged into the contention is decided
/// at once even while another record next to it is held. Without this, a busy
/// node's first conflict on a new record would be fenced by a budget spent on
/// an unrelated one.
#[test]
fn a_newly_contended_record_engages_while_another_is_held() {
    fn unavailable() -> ResolverOutcome {
        ResolverOutcome::Unavailable
    }
    let fixture = SyncFixture::new("resolve-contention-second-record");
    // Both nodes share a baseline for both records.
    write_shared(&fixture.a, "{\"node\":\"base\"}\n");
    write_record(&fixture.a, SECOND_STATE, "{\"node\":\"base\"}\n");
    fixture.service(&fixture.a).sync().unwrap();
    fixture.service(&fixture.b).sync().unwrap();
    // Node A changes both and publishes once; the remote head then stays put,
    // so nothing about the side that needs deciding moves for the rest of the
    // test.
    write_shared(&fixture.a, "{\"node\":\"remote\"}\n");
    write_record(&fixture.a, SECOND_STATE, "{\"node\":\"remote\"}\n");
    fixture.service(&fixture.a).sync().unwrap();

    let service = fixture.service(&fixture.b);
    let calls = Arc::new(AtomicU32::new(0));
    let _guard = install_resolver_override(
        &fixture.b,
        Arc::new(ScriptedResolver::new(counting_script(
            &calls,
            8,
            unavailable,
        ))),
    );

    // One contended record spends its whole budget and holds.
    for pass in 0..4 {
        write_shared(&fixture.b, &format!("{{\"node\":\"live-{pass}\"}}\n"));
        service.sync().unwrap_err();
    }
    assert_eq!(calls.load(Ordering::SeqCst), CONTENTION_ATTEMPT_LIMIT);
    assert_eq!(
        contention_refs(&fixture.b).len(),
        CONTENTION_ATTEMPT_LIMIT as usize
    );

    // A second record is now contested against that same unmoved remote head.
    write_record(&fixture.b, SECOND_STATE, "{\"node\":\"live\"}\n");
    service.sync().unwrap_err();

    assert_eq!(
        calls.load(Ordering::SeqCst),
        CONTENTION_ATTEMPT_LIMIT + 1,
        "a record contended for the first time brings its own budget with it"
    );
    assert_eq!(
        contention_refs(&fixture.b).len(),
        CONTENTION_ATTEMPT_LIMIT as usize + 1,
        "the new record bought one attempt; the held record bought nothing more"
    );
    // Still not a fence: the report stands and one authority run converges.
    let run = service
        .run_state_recovery(StateRecoveryDecision::uniform(StateRecoveryAuthority::Live))
        .unwrap();
    assert!(run.ok && run.recovered, "{run:#?}");
    assert_eq!(read_shared(&fixture.b), "{\"node\":\"live-3\"}\n");
}

/// A moved remote side is the change that genuinely needs deciding again: the
/// budget is keyed on it, so the next pass engages the resolver immediately
/// even though this node's contention was held a moment earlier.
#[test]
fn a_moved_remote_side_re_engages_a_held_contention() {
    fn unavailable() -> ResolverOutcome {
        ResolverOutcome::Unavailable
    }
    let fixture = contested_fixture("resolve-contention-remote-moved");
    let service = fixture.service(&fixture.b);
    let calls = Arc::new(AtomicU32::new(0));
    let mut steps = counting_script(&calls, CONTENTION_ATTEMPT_LIMIT as usize, unavailable);
    let counted = Arc::clone(&calls);
    steps.push(completing(move |request| {
        counted.fetch_add(1, Ordering::SeqCst);
        fs::write(
            request.workspace_dir.join(SHARED_TREE_PATH),
            "{\"node\":\"merged\"}\n",
        )
        .unwrap();
    }));
    let _guard = install_resolver_override(&fixture.b, Arc::new(ScriptedResolver::new(steps)));

    for pass in 0..4 {
        write_shared(&fixture.b, &format!("{{\"node\":\"live-{pass}\"}}\n"));
        service.sync().unwrap_err();
    }
    assert_eq!(calls.load(Ordering::SeqCst), CONTENTION_ATTEMPT_LIMIT);

    // The other node publishes: the side that needs deciding moved.
    write_shared(&fixture.a, "{\"node\":\"remote-2\"}\n");
    fixture.service(&fixture.a).sync().unwrap();

    let result = service.sync().unwrap();

    assert!(result.ok && result.pushed, "{result:#?}");
    assert_eq!(calls.load(Ordering::SeqCst), CONTENTION_ATTEMPT_LIMIT + 1);
    assert_eq!(read_shared(&fixture.b), "{\"node\":\"merged\"}\n");
    // The budget bought against the superseded remote head is swept with it.
    assert_eq!(contention_refs(&fixture.b).len(), 1);
    assert!(resolve_refs(&fixture.b).is_empty());
}

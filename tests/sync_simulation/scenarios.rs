use super::*;

const A: usize = 0;
const B: usize = 1;
const C: usize = 2;
const NODE_IDS: &[&str] = &["node-a", "node-b", "node-c"];

/// Convergence: a record written on one node reaches every node, and all
/// state-branch heads (including the origin's) are identical afterwards.
#[test]
fn convergence_simple() {
    let mut fleet = SimulatedFleet::new("convergence-simple", NODE_IDS);
    let outcomes = fleet.run(&[
        Event::LiveWrite {
            node: A,
            goal_id: "AACONVERGE",
            mutation: "v1",
        },
        Event::Sync { node: A },
        Event::Sync { node: B },
        Event::Sync { node: C },
    ]);
    assert!(outcomes[1].sync_ok("sync a").pushed);
    assert!(outcomes[2].sync_ok("sync b").pulled);
    assert!(outcomes[3].sync_ok("sync c").pulled);

    fleet.assert_converged("convergence_simple");
    assert_eq!(fleet.live_goal_name(B, "AACONVERGE"), "v1");
    assert_eq!(fleet.live_goal_name(C, "AACONVERGE"), "v1");
}

/// No lost work: records written concurrently on two nodes both survive the
/// given sync interleaving, and every record version any node ever committed
/// stays content-reachable from the converged head or a retained ref. The
/// record advanced on node A contributes a superseded version, so the
/// reachability check exercises history, not just the final tree.
fn no_lost_work_disjoint(name: &str, sync_order: [usize; 2]) {
    let mut fleet = SimulatedFleet::new(name, NODE_IDS);
    let [first, second] = sync_order;
    let outcomes = fleet.run(&[
        Event::LiveWrite {
            node: A,
            goal_id: "AADISJOINT",
            mutation: "v1",
        },
        Event::LiveWrite {
            node: B,
            goal_id: "BBDISJOINT",
            mutation: "v1",
        },
        Event::Sync { node: first },
        Event::Sync { node: second },
        Event::Sync { node: first },
        Event::LiveWrite {
            node: A,
            goal_id: "AADISJOINT",
            mutation: "v2",
        },
        Event::Sync { node: A },
        Event::Sync { node: B },
        Event::Sync { node: C },
    ]);
    for (index, outcome) in outcomes.iter().enumerate() {
        if let Outcome::Sync(_) = outcome {
            let result = outcome.sync_ok(&format!("event {index}"));
            assert!(result.ok, "event {index}: sync not ok: {result:#?}");
        }
    }

    let head = fleet.assert_converged("no_lost_work_disjoint");
    for node in [A, B, C] {
        assert_eq!(fleet.live_goal_name(node, "AADISJOINT"), "v2");
        assert_eq!(fleet.live_goal_name(node, "BBDISJOINT"), "v1");
    }
    fleet.assert_committed_versions_reachable(A, &head, "no_lost_work_disjoint");
}

#[test]
fn no_lost_work_disjoint_sync_order_ab() {
    no_lost_work_disjoint("no-lost-work-ab", [A, B]);
}

#[test]
fn no_lost_work_disjoint_sync_order_ba() {
    no_lost_work_disjoint("no-lost-work-ba", [B, A]);
}

/// The Insurity-fence regression: node A's state branch is strictly ahead of
/// an unchanged remote (a push was rejected after committing), the live store
/// advances again, and the next sync MUST fast-forward — no conflict report,
/// no semantic-merge rejection. Today's pipeline compares live state against
/// the stale baseline instead of classifying the ancestor-related heads, so
/// it manufactures a conflict from its own unpublished commit.
#[test]
#[ignore = "stage 2: ancestry short-circuit; field report 2026-08-17 bo2lnxnevo03"]
fn ancestor_heads_with_live_advance() {
    let mut fleet = SimulatedFleet::new("ancestor-heads", NODE_IDS);
    let outcomes = fleet.run(&[
        Event::LiveWrite {
            node: A,
            goal_id: "AAANCESTOR",
            mutation: "v1",
        },
        Event::Sync { node: A },
    ]);
    outcomes[1].sync_ok("baseline sync");

    // The advance commits locally; the origin rejects the publish.
    fleet.block_state_pushes();
    let outcomes = fleet.run(&[
        Event::LiveWrite {
            node: A,
            goal_id: "AAANCESTOR",
            mutation: "v2",
        },
        Event::Sync { node: A },
    ]);
    outcomes[1].sync_err("rejected publish");
    assert!(
        fleet.latest_report(A).is_none(),
        "a one-sided local advance must not produce a conflict report"
    );
    let origin_head = fleet.origin_state_head();
    let local_head = fleet.nodes[A].state_head();
    assert_ne!(local_head, origin_head);
    assert_eq!(
        git_stdout(
            &fleet.nodes[A].target_root,
            &["merge-base", &origin_head, &local_head]
        ),
        origin_head,
        "the premise requires the remote head to be a strict ancestor of the local head"
    );

    // A second live write lands before the next pass; the remote is still a
    // strict ancestor, so the pass must fast-forward and publish.
    fleet.allow_state_pushes();
    let outcomes = fleet.run(&[
        Event::LiveWrite {
            node: A,
            goal_id: "AAANCESTOR",
            mutation: "v3",
        },
        Event::Sync { node: A },
    ]);
    let result = outcomes[1].sync_ok("ancestor fast-forward");
    assert!(result.pushed, "{result:#?}");
    assert!(
        fleet.latest_report(A).is_none(),
        "ancestor-related heads must never produce a conflict report"
    );
    assert_eq!(fleet.origin_state_head(), fleet.nodes[A].state_head());
    assert_eq!(fleet.live_goal_name(A, "AAANCESTOR"), "v3");
}

/// Terminal recovery: a genuine two-sided divergence on one record produces a
/// conflict report; one `state-recovery run` with an authority converges the
/// fleet; running it again is a no-op that produces no new conflict report.
#[test]
fn terminal_recovery() {
    let mut fleet = SimulatedFleet::new("terminal-recovery", NODE_IDS);
    let outcomes = fleet.run(&[
        Event::LiveWrite {
            node: A,
            goal_id: "AATERMINAL",
            mutation: "base",
        },
        Event::Sync { node: A },
        Event::Sync { node: B },
        Event::LiveWrite {
            node: A,
            goal_id: "AATERMINAL",
            mutation: "a-edit",
        },
        Event::Sync { node: A },
        Event::LiveWrite {
            node: B,
            goal_id: "AATERMINAL",
            mutation: "b-edit",
        },
        Event::Sync { node: B },
    ]);
    let conflict = outcomes[6].sync_err("contested sync");
    assert!(matches!(conflict, RefineError::Conflict(_)), "{conflict}");
    let report = fleet
        .latest_report(B)
        .expect("a two-sided divergence writes a conflict report");
    assert_eq!(
        report.unresolved_paths,
        vec![goal_record_path("AATERMINAL")]
    );

    let outcomes = fleet.run(&[
        Event::RecoveryRun {
            node: B,
            authority: StateRecoveryAuthority::Remote,
        },
        Event::Sync { node: A },
        Event::Sync { node: C },
    ]);
    let recovered = outcomes[0].recovery_ok("first recovery run");
    assert!(recovered.ok && recovered.recovered, "{recovered:#?}");
    outcomes[1].sync_ok("converging sync a");
    outcomes[2].sync_ok("converging sync c");
    fleet.assert_converged("terminal_recovery");
    for node in [A, B, C] {
        assert_eq!(fleet.live_goal_name(node, "AATERMINAL"), "a-edit");
    }

    // Rerunning the command it exists to clear must be a no-op: nothing to
    // recover, and no new conflict report materializes.
    let report_before = fleet.raw_report_bytes(B);
    let outcomes = fleet.run(&[Event::RecoveryRun {
        node: B,
        authority: StateRecoveryAuthority::Remote,
    }]);
    let rerun = outcomes[0].recovery_ok("second recovery run");
    assert!(rerun.ok && !rerun.recovered, "{rerun:#?}");
    assert_eq!(rerun.attempts, 1, "{rerun:#?}");
    assert_eq!(fleet.raw_report_bytes(B), report_before);
    fleet.assert_converged("terminal_recovery rerun");
}

/// Stable identity: the same divergence reported twice with zero live changes
/// in between carries the same report id. Today the id hashes a fresh attempt
/// uuid and a wall-clock timestamp into every report, so retrying the very
/// same divergence mints a new identity each time.
#[test]
#[ignore = "stage 2: stable operands"]
fn stable_report_id() {
    let mut fleet = SimulatedFleet::new("stable-report-id", NODE_IDS);
    let outcomes = fleet.run(&[
        Event::LiveWrite {
            node: A,
            goal_id: "AASTABLEID",
            mutation: "base",
        },
        Event::Sync { node: A },
        Event::Sync { node: B },
        Event::LiveWrite {
            node: A,
            goal_id: "AASTABLEID",
            mutation: "a-edit",
        },
        Event::Sync { node: A },
        Event::LiveWrite {
            node: B,
            goal_id: "AASTABLEID",
            mutation: "b-edit",
        },
        Event::Sync { node: B },
    ]);
    outcomes[6].sync_err("first contested sync");
    let first = fleet
        .latest_report(B)
        .expect("the first attempt writes a conflict report")
        .report_id;
    let outcomes = fleet.run(&[Event::Sync { node: B }]);
    outcomes[0].sync_err("second contested sync");
    let second = fleet
        .latest_report(B)
        .expect("the second attempt writes a conflict report")
        .report_id;
    assert_eq!(
        first, second,
        "one divergence must keep one report identity across attempts"
    );
}

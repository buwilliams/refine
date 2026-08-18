//! Rolling-upgrade compatibility: a fleet upgrades node by node, in any
//! order, so an upgraded node and a not-yet-upgraded one publish to the same
//! `refine/state` branch for as long as the rollout takes. These scenarios
//! drive a `NodeKind::Legacy` node — pre-upgrade behavior emulated
//! mechanically with git, never the retired code — against the real pipeline
//! and check that the mixture converges, loses nothing, and invents nothing.

use super::*;

const A: usize = 0;
const B: usize = 1;

/// One upgraded node and one not-yet-upgraded node each publish a record.
/// Both push orders converge: the mixed fleet reaches one head, both records
/// reach both live stores, no conflict report is written, and every committed
/// version stays reachable.
fn new_and_legacy_converge(name: &str, legacy_first: bool) {
    let mut fleet = SimulatedFleet::new(name, &["node-a"]);
    let legacy = fleet.add_legacy_node("node-l");
    fleet.run(&[
        Event::LiveWrite {
            node: A,
            goal_id: "AAMIXED",
            mutation: "v1",
        },
        Event::LiveWrite {
            node: legacy,
            goal_id: "LLMIXED",
            mutation: "v1",
        },
    ]);

    if legacy_first {
        // The not-yet-upgraded node publishes a linear commit first; the
        // upgraded node's pass then merges from the real merge base.
        let outcomes = fleet.run(&[
            Event::LegacySync { node: legacy },
            Event::Sync { node: A },
            Event::LegacySync { node: legacy },
        ]);
        outcomes[0].legacy_published("legacy publish");
        let merged = outcomes[1].sync_ok("upgraded merge");
        assert!(merged.pushed && merged.pulled, "{merged:#?}");
        outcomes[2].legacy_published("legacy adopt");
    } else {
        // The upgraded node publishes first; the not-yet-upgraded node
        // rebases onto it and appends its own linear commit, and the
        // upgraded node's next pass is a plain fast-forward — an
        // ancestor-related pair never merges, whoever wrote the head.
        let outcomes = fleet.run(&[
            Event::Sync { node: A },
            Event::LegacySync { node: legacy },
            Event::Sync { node: A },
        ]);
        assert!(outcomes[0].sync_ok("upgraded publish").pushed);
        let published = outcomes[1].legacy_published("legacy publish");
        assert!(published.committed, "{published:#?}");
        let followed = outcomes[2].sync_ok("upgraded fast-forward");
        assert!(
            followed.pulled && !followed.pushed && !followed.committed,
            "a legacy head this node is strictly behind must fast-forward: {followed:#?}"
        );
    }

    assert!(
        fleet.latest_report(A).is_none(),
        "a mixed-version fleet must not manufacture a conflict report"
    );
    let head = fleet.assert_converged(name);
    for node in [A, legacy] {
        assert_eq!(fleet.live_goal_name(node, "AAMIXED"), "v1");
        assert_eq!(fleet.live_goal_name(node, "LLMIXED"), "v1");
    }
    fleet.assert_committed_versions_reachable(A, &head, name);
}

#[test]
fn new_and_legacy_converge_with_legacy_pushing_first() {
    new_and_legacy_converge("rolling-legacy-first", true);
}

#[test]
fn new_and_legacy_converge_with_new_pushing_first() {
    new_and_legacy_converge("rolling-new-first", false);
}

/// A not-yet-upgraded node publishes between an upgraded node's merge and its
/// push. The upgraded node's pass must re-merge against the legacy head
/// inside the same command: one converged head, the legacy linear commit
/// reachable as a parent, no conflict report, and every node's records
/// intact.
#[test]
fn legacy_publish_races_a_pending_merge() {
    let mut fleet = SimulatedFleet::new("rolling-legacy-race", &["node-a", "node-b"]);
    let legacy = fleet.add_legacy_node("node-l");
    let outcomes = fleet.run(&[
        Event::LiveWrite {
            node: A,
            goal_id: "AARACEMIX",
            mutation: "v1",
        },
        Event::Sync { node: A },
        Event::LegacySync { node: legacy },
        Event::LiveWrite {
            node: legacy,
            goal_id: "LLRACEMIX",
            mutation: "v1",
        },
    ]);
    assert!(outcomes[1].sync_ok("publishing sync").pushed);
    outcomes[2].legacy_published("legacy adopt");

    // The legacy node commits its record locally while the origin rejects
    // publishes; that commit then becomes the publish the gate injects
    // between the upgraded node's merge and its push.
    fleet.block_state_pushes();
    let outcomes = fleet.run(&[Event::LegacySync { node: legacy }]);
    let blocked = outcomes[0].legacy("blocked legacy publish");
    assert!(
        blocked.committed && !blocked.pushed,
        "the legacy commit must be durable and unpublished: {blocked:#?}"
    );
    fleet.allow_state_pushes();
    let staged = fleet.stage_race_commit(legacy);
    fleet.arm_state_push_race();

    let outcomes = fleet.run(&[
        Event::LiveWrite {
            node: B,
            goal_id: "BBRACEMIX",
            mutation: "v1",
        },
        Event::Sync { node: B },
    ]);
    let result = outcomes[1].sync_ok("racing sync");
    assert!(result.pushed, "{result:#?}");
    assert!(
        !fleet.state_push_race_armed(),
        "the legacy publish must have landed during the upgraded node's pass"
    );
    assert!(
        fleet.latest_report(B).is_none(),
        "a legacy publish racing a merge must not produce a conflict report"
    );

    let head = fleet.origin_state_head();
    let parents = git_stdout(
        &fleet.nodes[B].target_root,
        &["rev-list", "--parents", "-n", "1", &head],
    );
    let parents = parents.split_whitespace().skip(1).collect::<Vec<_>>();
    assert_eq!(
        parents.len(),
        2,
        "the retried publish must merge the legacy head, not replace it"
    );
    assert!(
        parents.contains(&staged.as_str()),
        "the legacy commit {staged} must be a parent of the converged head {head}"
    );

    let outcomes = fleet.run(&[Event::Sync { node: A }, Event::LegacySync { node: legacy }]);
    outcomes[0].sync_ok("converging sync a");
    outcomes[1].legacy_published("converging legacy pass");
    let head = fleet.assert_converged("legacy_publish_races_a_pending_merge");
    for node in [A, B, legacy] {
        assert_eq!(fleet.live_goal_name(node, "AARACEMIX"), "v1");
        assert_eq!(fleet.live_goal_name(node, "BBRACEMIX"), "v1");
        assert_eq!(fleet.live_goal_name(node, "LLRACEMIX"), "v1");
    }
    fleet.assert_committed_versions_reachable(B, &head, "legacy_publish_races_a_pending_merge");
}

/// The linear history a not-yet-upgraded node keeps pushing must never make
/// the current pipeline delete anything: the upgraded node's pass hydrates
/// records it has never seen instead of publishing their absence, and the
/// legacy node's records survive every later upgraded publish. The branch's
/// exact path set is asserted, so an invented deletion cannot hide behind a
/// record lookup.
#[test]
fn legacy_linear_history_invents_no_deletions() {
    let mut fleet = SimulatedFleet::new("rolling-no-deletions", &["node-a"]);
    let legacy = fleet.add_legacy_node("node-l");
    let outcomes = fleet.run(&[
        Event::LiveWrite {
            node: A,
            goal_id: "AAKEEP",
            mutation: "v1",
        },
        Event::Sync { node: A },
        Event::LegacySync { node: legacy },
        Event::LiveWrite {
            node: legacy,
            goal_id: "LLKEEP",
            mutation: "v1",
        },
        Event::LegacySync { node: legacy },
    ]);
    assert!(outcomes[1].sync_ok("publishing sync").pushed);
    outcomes[2].legacy_published("legacy adopt");
    outcomes[4].legacy_published("legacy publish");
    assert!(
        !fleet.nodes[A]
            .live_refine()
            .join(goal_record_path("LLKEEP"))
            .exists(),
        "the premise requires the upgraded node to have never observed the legacy record"
    );

    let expected = vec![goal_record_path("AAKEEP"), goal_record_path("LLKEEP")];
    let outcomes = fleet.run(&[Event::Sync { node: A }]);
    let followed = outcomes[0].sync_ok("upgraded pass over legacy history");
    assert!(followed.pulled, "{followed:#?}");
    assert_eq!(
        fleet.live_goal_name(A, "LLKEEP"),
        "v1",
        "the unobserved legacy record is hydrated, never published as a deletion"
    );
    assert_eq!(fleet.nodes[A].state_tree_paths(), expected);

    // An upgraded node advancing its own record afterwards still publishes
    // only its delta: the legacy node's record is untouched.
    let outcomes = fleet.run(&[
        Event::LiveWrite {
            node: A,
            goal_id: "AAKEEP",
            mutation: "v2",
        },
        Event::Sync { node: A },
        Event::LegacySync { node: legacy },
    ]);
    assert!(outcomes[1].sync_ok("upgraded advance").pushed);
    outcomes[2].legacy_published("legacy converge");

    let head = fleet.assert_converged("legacy_linear_history_invents_no_deletions");
    for node in [A, legacy] {
        assert_eq!(fleet.live_goal_name(node, "AAKEEP"), "v2");
        assert_eq!(fleet.live_goal_name(node, "LLKEEP"), "v1");
        assert_eq!(fleet.nodes[node].state_tree_paths(), expected);
    }
    fleet.assert_committed_versions_reachable(
        A,
        &head,
        "legacy_linear_history_invents_no_deletions",
    );
}

/// Upgrading one node of a mixed fleet retires that node's pre-upgrade
/// leftovers — the baseline fingerprint file and its anchor refs — on the
/// first pass, without losing a record: the upgraded node keeps its own work,
/// adopts what the rest of the fleet published, converges, and the node still
/// running the old machinery keeps working against the same branch.
#[test]
fn upgrading_a_node_retires_legacy_baseline_without_data_loss() {
    let mut fleet = SimulatedFleet::new("rolling-retire-baseline", &["node-a"]);
    let upgrading = fleet.add_legacy_node("node-l");
    let still_legacy = fleet.add_legacy_node("node-m");
    let outcomes = fleet.run(&[
        Event::LiveWrite {
            node: A,
            goal_id: "AARETIRE",
            mutation: "v1",
        },
        Event::Sync { node: A },
        Event::LiveWrite {
            node: upgrading,
            goal_id: "LLRETIRE",
            mutation: "v1",
        },
        Event::LegacySync { node: upgrading },
        Event::LiveWrite {
            node: still_legacy,
            goal_id: "MMRETIRE",
            mutation: "v1",
        },
        Event::LegacySync { node: still_legacy },
    ]);
    assert!(outcomes[1].sync_ok("publishing sync").pushed);
    outcomes[3].legacy_published("legacy publish");
    outcomes[5].legacy_published("second legacy publish");

    // The premise: this node carries exactly what the retired machinery left
    // behind on it.
    assert!(
        fleet.nodes[upgrading].legacy_baseline_file().exists(),
        "the pre-upgrade baseline fingerprint file must exist before the upgrade"
    );
    let anchors = fleet.nodes[upgrading].legacy_baseline_refs();
    assert!(
        !anchors.is_empty(),
        "the pre-upgrade baseline anchor refs must exist before the upgrade"
    );

    let outcomes = fleet.run(&[
        Event::Upgrade { node: upgrading },
        Event::LiveWrite {
            node: upgrading,
            goal_id: "LLRETIRE",
            mutation: "v2",
        },
        Event::Sync { node: upgrading },
    ]);
    let upgraded = outcomes[2].sync_ok("first pass after upgrade");
    assert!(upgraded.ok, "{upgraded:#?}");
    assert!(
        !fleet.nodes[upgrading].legacy_baseline_file().exists(),
        "the first pass after an upgrade retires the baseline fingerprint file"
    );
    assert!(
        fleet.nodes[upgrading].legacy_baseline_refs().is_empty(),
        "the first pass after an upgrade retires the baseline anchor refs: {:?}",
        fleet.nodes[upgrading].legacy_baseline_refs()
    );
    assert!(
        fleet.latest_report(upgrading).is_none(),
        "inheriting pre-upgrade state must not read as a conflict"
    );

    let outcomes = fleet.run(&[
        Event::Sync { node: A },
        Event::LegacySync { node: still_legacy },
        Event::Sync { node: upgrading },
    ]);
    outcomes[0].sync_ok("converging sync a");
    outcomes[1].legacy_published("converging legacy pass");
    outcomes[2].sync_ok("converging upgraded pass");
    let head = fleet.assert_converged("upgrading_a_node_retires_legacy_baseline_without_data_loss");
    for node in [A, upgrading, still_legacy] {
        assert_eq!(fleet.live_goal_name(node, "AARETIRE"), "v1");
        assert_eq!(
            fleet.live_goal_name(node, "LLRETIRE"),
            "v2",
            "the upgraded node's own work survives its upgrade"
        );
        assert_eq!(
            fleet.live_goal_name(node, "MMRETIRE"),
            "v1",
            "a node still running the old machinery keeps publishing to the same branch"
        );
    }
    fleet.assert_committed_versions_reachable(
        upgrading,
        &head,
        "upgrading_a_node_retires_legacy_baseline_without_data_loss",
    );
}

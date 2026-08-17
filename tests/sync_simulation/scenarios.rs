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

/// The Insurity-fence regression (field report 2026-08-17 bo2lnxnevo03):
/// node A's state branch is strictly ahead of an unchanged remote (a push was
/// rejected after committing), the live store advances again, and the next
/// sync MUST fast-forward — no conflict report, no merge. The stage-2 ladder
/// classifies the ancestor-related heads instead of comparing live state
/// against a stale baseline, so it can no longer manufacture a conflict from
/// its own unpublished commit.
#[test]
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
/// conflict report; one `sync --authority` run converges the
/// fleet (every head classifies Equal); running it again is a no-op that
/// moves no head and produces no new conflict report; and the losing side
/// stays reachable as a parent of the recovery merge commit.
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

    // The losing side of the authority decision: B's local head, still
    // carrying the displaced `b-edit` version.
    let losing_head = fleet.nodes[B].state_head();

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
    assert_eq!(
        recovered.recovery.as_ref().unwrap().settled_paths,
        vec![goal_record_path("AATERMINAL")]
    );
    outcomes[1].sync_ok("converging sync a");
    outcomes[2].sync_ok("converging sync c");
    // Identical heads everywhere is exactly `classify == Equal` on every
    // node pair, the read the recovery verification itself performs.
    let head = fleet.assert_converged("terminal_recovery");
    for node in [A, B, C] {
        assert_eq!(fleet.live_goal_name(node, "AATERMINAL"), "a-edit");
    }

    // The recovery commit is a true merge: both pre-recovery heads are its
    // parents, so the losing side — and its displaced record version — stays
    // reachable without any side ledger.
    let parents = git_stdout(
        &fleet.nodes[B].target_root,
        &["rev-list", "--parents", "-n", "1", &head],
    );
    let parents = parents.split_whitespace().skip(1).collect::<Vec<_>>();
    assert_eq!(parents.len(), 2, "recovery must commit a two-parent merge");
    assert!(
        parents.contains(&losing_head.as_str()),
        "the losing head {losing_head} must remain a parent of {head}"
    );
    assert_eq!(
        git_bytes_optional(
            &fleet.nodes[B].target_root,
            &[
                "show",
                &format!("{losing_head}:.refine/{}", goal_record_path("AATERMINAL")),
            ],
        )
        .expect("the displaced version stays readable from the losing parent"),
        goal_record_bytes("AATERMINAL", "b-edit")
    );

    // Rerunning the command it exists to clear must be a no-op: nothing to
    // recover, no head moves, and no new conflict report materializes.
    let report_before = fleet.raw_report_bytes(B);
    let heads_before = fleet
        .nodes
        .iter()
        .map(|node| node.state_head())
        .collect::<Vec<_>>();
    let outcomes = fleet.run(&[Event::RecoveryRun {
        node: B,
        authority: StateRecoveryAuthority::Remote,
    }]);
    let rerun = outcomes[0].recovery_ok("second recovery run");
    assert!(rerun.ok && !rerun.recovered, "{rerun:#?}");
    assert_eq!(rerun.attempts, 1, "{rerun:#?}");
    assert_eq!(fleet.raw_report_bytes(B), report_before);
    assert_eq!(
        fleet.origin_state_head(),
        head,
        "rerun must not move the origin head"
    );
    assert_eq!(
        fleet
            .nodes
            .iter()
            .map(|node| node.state_head())
            .collect::<Vec<_>>(),
        heads_before,
        "rerun must not move any node head"
    );
    fleet.assert_converged("terminal_recovery rerun");
}

/// Stable identity: the same divergence reported twice with zero live changes
/// in between carries the same report id. Stage 2 derives the id from the
/// merge base, both heads, and the sorted unresolved paths — every operand a
/// commit — so retrying the very same divergence cannot mint a new identity.
#[test]
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

/// Two nodes change DIFFERENT members of the same goal record: the structural
/// driver merges them deterministically — no conflict report, no operator —
/// and the converged record carries both changes on every node.
#[test]
fn diverged_disjoint_members() {
    let mut fleet = SimulatedFleet::new("diverged-disjoint-members", NODE_IDS);
    let outcomes = fleet.run(&[
        Event::LiveWriteMembers {
            node: A,
            goal_id: "AADISJMEMBER",
            name: "base",
            status: "todo",
        },
        Event::Sync { node: A },
        Event::Sync { node: B },
        Event::LiveWriteMembers {
            node: A,
            goal_id: "AADISJMEMBER",
            name: "a-name",
            status: "todo",
        },
        Event::Sync { node: A },
        Event::LiveWriteMembers {
            node: B,
            goal_id: "AADISJMEMBER",
            name: "base",
            status: "done",
        },
        Event::Sync { node: B },
    ]);
    let merged = outcomes[6].sync_ok("structural merge");
    assert!(merged.pushed && merged.pulled, "{merged:#?}");
    assert!(
        merged
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("Merged divergent state records structurally"),
        "the pass must attribute the merge to the structural driver: {merged:#?}"
    );
    assert!(
        fleet.latest_report(B).is_none(),
        "a provably disjoint divergence must not produce a conflict report"
    );

    let outcomes = fleet.run(&[Event::Sync { node: A }, Event::Sync { node: C }]);
    outcomes[0].sync_ok("converging sync a");
    outcomes[1].sync_ok("converging sync c");
    let head = fleet.assert_converged("diverged_disjoint_members");
    for node in [A, B, C] {
        assert_eq!(
            fleet.live_goal_member(node, "AADISJMEMBER", "name"),
            "a-name"
        );
        assert_eq!(
            fleet.live_goal_member(node, "AADISJMEMBER", "status"),
            "done"
        );
    }
    fleet.assert_committed_versions_reachable(B, &head, "diverged_disjoint_members");
}

/// Two nodes change the SAME member of the same goal record: the divergence
/// fails closed with a conflict report whose domain summary names the goal,
/// keeps one identity across attempts, and one authority run converges the
/// fleet on the decided side.
#[test]
fn diverged_contested_member() {
    let mut fleet = SimulatedFleet::new("diverged-contested-member", NODE_IDS);
    let outcomes = fleet.run(&[
        Event::LiveWrite {
            node: A,
            goal_id: "AACONTESTED",
            mutation: "base",
        },
        Event::Sync { node: A },
        Event::Sync { node: B },
        Event::LiveWrite {
            node: A,
            goal_id: "AACONTESTED",
            mutation: "a-edit",
        },
        Event::Sync { node: A },
        Event::LiveWrite {
            node: B,
            goal_id: "AACONTESTED",
            mutation: "b-edit",
        },
        Event::Sync { node: B },
    ]);
    outcomes[6].sync_err("first contested sync");
    let first = fleet
        .latest_report(B)
        .expect("a contested member writes a conflict report");
    assert_eq!(
        first.unresolved_paths,
        vec![goal_record_path("AACONTESTED")]
    );
    assert_eq!(
        first
            .conflicts
            .iter()
            .map(|conflict| conflict.summary.as_str())
            .collect::<Vec<_>>(),
        vec!["goal AACONTESTED: both nodes changed name"],
        "the report must summarize the conflict in domain terms, naming the goal"
    );

    let outcomes = fleet.run(&[Event::Sync { node: B }]);
    outcomes[0].sync_err("second contested sync");
    let second = fleet
        .latest_report(B)
        .expect("the second attempt writes a conflict report");
    assert_eq!(
        second.report_id, first.report_id,
        "one divergence must keep one report identity across attempts"
    );

    let outcomes = fleet.run(&[
        Event::RecoveryRun {
            node: B,
            authority: StateRecoveryAuthority::Live,
        },
        Event::Sync { node: A },
        Event::Sync { node: C },
    ]);
    let recovered = outcomes[0].recovery_ok("authority run");
    assert!(recovered.ok && recovered.recovered, "{recovered:#?}");
    assert_eq!(
        recovered.recovery.as_ref().unwrap().settled_paths,
        vec![goal_record_path("AACONTESTED")]
    );
    outcomes[1].sync_ok("converging sync a");
    outcomes[2].sync_ok("converging sync c");
    fleet.assert_converged("diverged_contested_member");
    for node in [A, B, C] {
        assert_eq!(fleet.live_goal_name(node, "AACONTESTED"), "b-edit");
    }
}

/// Bootstrap without a configured remote: sync commits durable state onto a
/// local `refine/state` branch, an idle rerun moves nothing, and configuring
/// a fresh remote later publishes the existing branch as first contact.
#[test]
fn bootstrap_no_remote() {
    let evidence = EvidenceDir::new("bootstrap-no-remote");
    let target_root = evidence.path().join("solo");
    fs::create_dir_all(&target_root).unwrap();
    git(&target_root, &["init", "-q"]);
    configure_repo(&target_root);
    fs::write(target_root.join("app.txt"), "base\n").unwrap();
    git(&target_root, &["add", "app.txt"]);
    git(&target_root, &["commit", "-q", "-m", "initial"]);
    let node = SimulatedNode {
        id: "solo-node",
        target_root,
    };
    write_active_node_identity(&node);
    let relative = goal_record_path("AASOLOGOAL");
    let bytes = goal_record_bytes("AASOLOGOAL", "v1");
    let destination = node.live_refine().join(&relative);
    fs::create_dir_all(destination.parent().unwrap()).unwrap();
    fs::write(&destination, &bytes).unwrap();

    let first = node.service().sync().unwrap();
    assert!(first.ok && first.committed && !first.pushed, "{first:#?}");
    assert_eq!(first.remote_configured, Some(false));
    assert_eq!(
        git_bytes_optional(
            &node.target_root,
            &["show", &format!("refine/state:.refine/{relative}")],
        )
        .expect("the record must be committed on the local state branch"),
        bytes
    );

    let head = node.state_head();
    let rerun = node.service().sync().unwrap();
    assert!(rerun.ok && !rerun.committed && !rerun.pushed, "{rerun:#?}");
    assert_eq!(
        node.state_head(),
        head,
        "an idle pass must not move the branch"
    );

    let remote = evidence.path().join("remote.git");
    git(
        evidence.path(),
        &["init", "--bare", remote.to_str().unwrap()],
    );
    git(
        &node.target_root,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    let published = node.service().sync().unwrap();
    assert!(published.ok && published.pushed, "{published:#?}");
    assert_eq!(
        git_stdout(&remote, &["rev-parse", "refine/state"]),
        node.state_head()
    );
}

/// A node joining a published fleet with only locally generated bootstrap
/// metadata adopts the remote records instead of contesting them — no
/// conflict report, no invented deletions — and its own bootstrap file joins
/// the branch additively.
#[test]
fn bootstrap_adopt_remote() {
    let mut fleet = SimulatedFleet::new("bootstrap-adopt-remote", &["node-a"]);
    let outcomes = fleet.run(&[
        Event::LiveWrite {
            node: A,
            goal_id: "AAADOPTION",
            mutation: "v1",
        },
        Event::Sync { node: A },
    ]);
    assert!(outcomes[1].sync_ok("publishing sync").pushed);

    let b = fleet.add_node("node-b");
    let bootstrap_bytes = b"{\"version\":1}\n".to_vec();
    fs::write(
        fleet.nodes[b].live_refine().join("refine.json"),
        &bootstrap_bytes,
    )
    .unwrap();

    let outcomes = fleet.run(&[Event::Sync { node: b }]);
    let adopted = outcomes[0].sync_ok("adopting sync");
    assert!(adopted.ok && adopted.pulled, "{adopted:#?}");
    assert!(
        adopted
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("Adopted synchronized Refine state"),
        "{adopted:#?}"
    );
    assert!(
        fleet.latest_report(b).is_none(),
        "a bootstrap-only join must not produce a conflict report"
    );
    assert_eq!(fleet.live_goal_name(b, "AAADOPTION"), "v1");
    assert_eq!(
        git_bytes_optional(
            &fleet.nodes[b].target_root,
            &["show", "refine/state:.refine/refine.json"],
        )
        .expect("the joining node's bootstrap metadata joins the branch additively"),
        bootstrap_bytes
    );

    let outcomes = fleet.run(&[Event::Sync { node: A }]);
    outcomes[0].sync_ok("converging sync a");
    fleet.assert_converged("bootstrap_adopt_remote");
}

/// Push race: the origin's state branch advances to a third node's commit
/// between a pass's merge and its push. The pass must retry bounded —
/// re-fetch and re-merge from the SAME committed local operand — converge in
/// the same command, write no conflict report, and keep both operands of the
/// abandoned merge attempt reachable as parents of the published merge.
#[test]
fn push_race_retry() {
    let mut fleet = SimulatedFleet::new("push-race-retry", NODE_IDS);
    let outcomes = fleet.run(&[
        Event::LiveWrite {
            node: A,
            goal_id: "AARACEGOAL",
            mutation: "v1",
        },
        Event::Sync { node: A },
        Event::Sync { node: C },
        Event::LiveWrite {
            node: C,
            goal_id: "CCRACEGOAL",
            mutation: "v1",
        },
    ]);
    assert!(outcomes[1].sync_ok("publishing sync").pushed);
    outcomes[2].sync_ok("adopting sync");

    // C commits its record locally while the origin rejects publishes; the
    // staged commit then becomes the racing publish the gate injects between
    // B's merge and B's push.
    fleet.block_state_pushes();
    let outcomes = fleet.run(&[Event::Sync { node: C }]);
    outcomes[0].sync_err("rejected publish");
    fleet.allow_state_pushes();
    let staged = fleet.stage_race_commit(C);
    fleet.arm_state_push_race();

    let outcomes = fleet.run(&[
        Event::LiveWrite {
            node: B,
            goal_id: "BBRACEGOAL",
            mutation: "v1",
        },
        Event::Sync { node: B },
    ]);
    let result = outcomes[1].sync_ok("racing sync");
    assert!(result.pushed, "{result:#?}");
    assert!(
        !fleet.state_push_race_armed(),
        "the race gate must have fired during the pass"
    );
    assert!(
        fleet.latest_report(B).is_none(),
        "a push race must not produce a conflict report"
    );

    // The published head is a merge of B's committed local operand and the
    // raced third-node head — so the abandoned first merge attempt's parents
    // (that same local operand and the pre-race remote head) stay reachable.
    let head = fleet.origin_state_head();
    let parents = git_stdout(
        &fleet.nodes[B].target_root,
        &["rev-list", "--parents", "-n", "1", &head],
    );
    let parents = parents.split_whitespace().skip(1).collect::<Vec<_>>();
    assert_eq!(
        parents.len(),
        2,
        "the retried publish must be a two-parent merge"
    );
    assert!(
        parents.contains(&staged.as_str()),
        "the raced commit {staged} must be a parent of the converged head {head}"
    );

    let outcomes = fleet.run(&[Event::Sync { node: A }, Event::Sync { node: C }]);
    outcomes[0].sync_ok("converging sync a");
    outcomes[1].sync_ok("converging sync c");
    let head = fleet.assert_converged("push_race_retry");
    for node in [A, B, C] {
        assert_eq!(fleet.live_goal_name(node, "AARACEGOAL"), "v1");
        assert_eq!(fleet.live_goal_name(node, "BBRACEGOAL"), "v1");
        assert_eq!(fleet.live_goal_name(node, "CCRACEGOAL"), "v1");
    }
    fleet.assert_committed_versions_reachable(B, &head, "push_race_retry");
}

use super::*;

pub(super) fn jira_row(goal: &Value, commits: &[GitChange]) -> RefineResult<String> {
    let goal_id = required_string(goal, "id")?;
    let summary = required_string(goal, "name")?;
    let description = jira_description(goal, commits);
    if description.chars().count() > JIRA_DESCRIPTION_LIMIT {
        return Err(RefineError::Serialization(format!(
            "Goal {goal_id} Jira description could not be rendered within Jira's \
             {JIRA_DESCRIPTION_LIMIT} character limit; report this renderer invariant failure"
        )));
    }

    let priority = title_case(nonempty_string(goal, "priority").unwrap_or("low"));
    let values = [
        summary,
        description.as_str(),
        "Task",
        priority.as_str(),
        "refine-soc2-evidence",
        goal_id,
        nonempty_string(goal, "status").unwrap_or("unknown"),
        nonempty_string(goal, "branch_name").unwrap_or(""),
        nonempty_string(goal, "base_commit").unwrap_or(""),
        nonempty_string(goal, "candidate_commit").unwrap_or(""),
    ];
    Ok(values
        .iter()
        .map(|value| csv_cell(value))
        .collect::<Vec<_>>()
        .join(","))
}

pub(super) fn jira_description(goal: &Value, commits: &[GitChange]) -> String {
    let mut sections = Vec::<EvidenceSection>::new();
    let mut overview = vec!["Refine delivery evidence".to_string()];
    push_bounded_line(
        &mut overview,
        "Goal ID",
        string_or(goal, "id", "Unknown"),
        256,
    );
    push_bounded_line(
        &mut overview,
        "Status",
        string_or(goal, "status", "Unknown"),
        128,
    );
    push_bounded_line(
        &mut overview,
        "Priority",
        string_or(goal, "priority", "Unknown"),
        128,
    );
    push_bounded_line(
        &mut overview,
        "Reporter",
        string_or(goal, "reporter", "Unreported"),
        256,
    );
    push_bounded_line(
        &mut overview,
        "Assignee",
        string_or(goal, "assignee", "Unassigned"),
        256,
    );
    push_bounded_line(
        &mut overview,
        "Created",
        string_or(goal, "created", "Unknown"),
        128,
    );
    push_bounded_line(
        &mut overview,
        "Updated",
        string_or(goal, "updated", "Unknown"),
        128,
    );
    push_bounded_optional_line(&mut overview, "Feature", goal, "feature_id", 256);
    push_bounded_optional_line(&mut overview, "Node", goal, "node_id", 256);
    sections.push(EvidenceSection::new(
        "Goal identity",
        overview.join("\n"),
        IDENTITY_RESERVE,
    ));

    let mut traceability = vec!["Branches and commit anchors".to_string()];
    push_bounded_optional_line(
        &mut traceability,
        "Target branch",
        goal,
        "target_branch",
        600,
    );
    push_bounded_optional_line(
        &mut traceability,
        "Implementation branch",
        goal,
        "branch_name",
        600,
    );
    push_bounded_optional_line(&mut traceability, "Base commit", goal, "base_commit", 600);
    push_bounded_optional_line(
        &mut traceability,
        "Candidate commit",
        goal,
        "candidate_commit",
        600,
    );
    sections.push(EvidenceSection::new(
        "branch and commit anchor evidence",
        traceability.join("\n"),
        TRACEABILITY_RESERVE,
    ));

    let mut commit_evidence = vec!["Delivered commits".to_string()];
    if commits.is_empty() {
        commit_evidence.push("Commits delivered: None recorded".to_string());
    } else {
        commit_evidence.push(format!("Commits delivered: {}", commits.len()));
        for commit in commits {
            commit_evidence.push(format!(
                "- {} | {} | {}",
                commit.commit, commit.committed_time, commit.subject
            ));
        }
    }
    sections.push(EvidenceSection::new(
        "delivered commit evidence",
        commit_evidence.join("\n"),
        COMMIT_RESERVE,
    ));

    if let Some(rounds) = goal.get("rounds").and_then(Value::as_array) {
        let round_count = rounds.len().max(1);
        let request_limit = (12_000 / round_count).max(512);
        let implementation_limit = (8_000 / round_count).max(512);
        let guidance_limit = (2_000 / round_count).max(256);
        let mut outcomes = vec!["Round outcomes, Quality, and governance".to_string()];
        let mut requested_work = vec!["Round requested work".to_string()];
        let mut implementation = vec!["Round implementation outcomes".to_string()];
        let mut guidance = vec!["Round guidance decisions".to_string()];
        for (index, round) in rounds.iter().enumerate() {
            let round_number = index + 1;
            outcomes.push(format!("Round {round_number}"));
            push_bounded_optional_line(&mut outcomes, "Reporter", round, "reporter", 256);
            push_bounded_optional_line(&mut outcomes, "Assignee", round, "assignee", 256);
            push_bounded_optional_line(&mut outcomes, "Created", round, "created", 128);
            push_bounded_optional_line(&mut outcomes, "Updated", round, "updated", 128);
            push_bounded_optional_line(
                &mut outcomes,
                "Implementation reported at",
                round,
                "implementation_reported_at",
                128,
            );
            push_quality_summary(&mut outcomes, round);
            push_bounded_optional_line(
                &mut outcomes,
                "Governance checked at",
                round,
                "governance_checked_at",
                128,
            );
            push_governance_summary(&mut outcomes, round);

            if let Some(prompt) = nonempty_string(round, "prompt") {
                requested_work.push(format!(
                    "Round {round_number}:\n{}",
                    truncate_with_marker(
                        prompt,
                        request_limit,
                        &format!("Round {round_number} requested work")
                    )
                ));
            }
            if let Some(report) = nonempty_string(round, "implementation_report") {
                implementation.push(format!(
                    "Round {round_number}:\n{}",
                    truncate_with_marker(
                        report,
                        implementation_limit,
                        &format!("Round {round_number} implementation outcome")
                    )
                ));
            }
            if let Some(decision) = nonempty_string(round, "guidance_decision") {
                guidance.push(format!(
                    "Round {round_number}:\n{}",
                    truncate_with_marker(
                        decision,
                        guidance_limit,
                        &format!("Round {round_number} guidance decision")
                    )
                ));
            }
        }
        if outcomes.len() > 1 {
            sections.push(EvidenceSection::new(
                "round outcome, Quality, and governance evidence",
                outcomes.join("\n"),
                OUTCOME_RESERVE,
            ));
        }
        if requested_work.len() > 1 {
            sections.push(EvidenceSection::new(
                "round requested work",
                requested_work.join("\n\n"),
                REQUEST_RESERVE,
            ));
        }
        if implementation.len() > 1 {
            sections.push(EvidenceSection::new(
                "round implementation outcomes",
                implementation.join("\n\n"),
                IMPLEMENTATION_RESERVE,
            ));
        }
        if guidance.len() > 1 {
            sections.push(EvidenceSection::new(
                "round guidance decisions",
                guidance.join("\n\n"),
                GUIDANCE_RESERVE,
            ));
        }
    }

    if let Some(notes) = goal.get("notes").and_then(Value::as_array)
        && !notes.is_empty()
    {
        let mut lines = vec!["Notes".to_string()];
        for note in notes {
            let author = string_or(note, "author", "Unknown");
            let created = string_or(note, "created", "Unknown");
            let body = string_or(note, "body", "");
            lines.push(format!("- {created} | {author} | {body}"));
        }
        sections.push(EvidenceSection::new(
            "Goal notes",
            lines.join("\n"),
            NOTES_RESERVE,
        ));
    }

    render_budgeted_sections(&sections, JIRA_DESCRIPTION_LIMIT)
}

fn csv_cell(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

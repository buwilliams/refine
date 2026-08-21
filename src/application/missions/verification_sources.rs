//! Repository-backed verification sources.
//!
//! The tier-2 verifier registry is a pure function of the pinned
//! `VerificationContext`; this module is the honest bridge from the target
//! repository to that context. Every check is read-only Git: reachability
//! from the pinned head, path existence at a commit, quoted text at a
//! commit. Nothing here judges truth — a passed verifier proves provenance
//! only.

use std::collections::BTreeSet;
use std::path::Path;

use crate::application::missions::reconciliation::engine::ClaimedContribution;
use crate::application::missions::reconciliation::verify::{EvidenceRef, VerificationContext};
use crate::error::RefineResult;
use crate::infrastructure::git::repository::FileGitRepository;

/// The current target head, when the repository has one.
pub fn current_target_head(repo: &FileGitRepository) -> Option<String> {
    repo.git(&["rev-parse", "HEAD"])
        .ok()
        .filter(|output| output.success)
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|head| !head.is_empty())
}

/// Enrich one verification context from the target repository: every
/// evidence reference shape the registry can check is proven against the
/// pinned target head. Unreachable or absent evidence simply fails its
/// verifier, which routes the finding to judgment — never a silent drop.
pub fn enrich_from_repository(
    context: &mut VerificationContext,
    repo: &FileGitRepository,
    claims: &[ClaimedContribution],
) -> RefineResult<()> {
    let Some(head) = context.target_head.clone() else {
        return Ok(());
    };
    let mut cited_commits = BTreeSet::new();
    let mut cited_paths = Vec::new();
    let mut cited_quotes = Vec::new();

    for claim in claims {
        for finding in &claim.contribution.findings {
            for raw in &finding.evidence {
                match EvidenceRef::parse(raw) {
                    Some(EvidenceRef::Commit { id }) => {
                        cited_commits.insert(id);
                    }
                    Some(EvidenceRef::Path { path, commit }) => {
                        cited_commits.insert(commit.clone());
                        cited_paths.push((commit, path));
                    }
                    Some(EvidenceRef::Quote { text, commit }) => {
                        cited_commits.insert(commit.clone());
                        cited_quotes.push((commit, text));
                    }
                    Some(EvidenceRef::Test { .. }) | Some(EvidenceRef::Digest { .. }) => {}
                    None => {}
                }
            }
        }
    }

    context.reachable_commits.insert(head.clone());
    for commit in cited_commits {
        let reachable = repo
            .git(&["merge-base", "--is-ancestor", &commit, &head])
            .map(|output| output.success)
            .unwrap_or(false);
        if reachable {
            context.reachable_commits.insert(commit);
        }
    }
    for (commit, path) in cited_paths {
        let exists = repo
            .git(&["cat-file", "-e", &format!("{commit}:{path}")])
            .map(|output| output.success)
            .unwrap_or(false);
        if exists {
            context.existing_paths.insert((commit, path));
        }
    }
    for (commit, text) in cited_quotes {
        let present = repo
            .git(&[
                "grep",
                "-F",
                "-e",
                text.as_str(),
                commit.as_str(),
                "--",
                ".",
            ])
            .map(|output| output.success)
            .unwrap_or(false);
        if present {
            context
                .quotes_at_commit
                .entry(commit)
                .or_default()
                .insert(text);
        }
    }
    Ok(())
}

/// Verify staged candidate bytes: a digest enters `matched_digests` only
/// when the referenced immutable contribution file exists and hashes to the
/// recorded digest.
pub fn verify_staged_candidate_bytes(
    context: &mut VerificationContext,
    refine_dir: &Path,
    mission_id: &str,
    claims: &[ClaimedContribution],
) -> RefineResult<()> {
    use sha2::Digest as Sha256Digest;
    for claim in claims {
        for candidate in &claim.contribution.artifact_candidates {
            let Some(digest) = candidate.digest.as_deref() else {
                continue;
            };
            let extension = candidate
                .media_type
                .as_deref()
                .and_then(media_extension)
                .unwrap_or("bin");
            let Some(path) = crate::application::missions::persistence::contribution_path(
                refine_dir,
                mission_id,
                &claim.goal_id,
                claim.goal_round,
                digest,
                extension,
            ) else {
                continue;
            };
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let mut hasher = sha2::Sha256::new();
            hasher.update(&bytes);
            let observed: String = hasher
                .finalize()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            if observed == digest {
                context.matched_digests.insert(digest.to_string());
            }
        }
    }
    Ok(())
}

fn media_extension(media_type: &str) -> Option<&'static str> {
    match media_type {
        "text/markdown" | "text/x-markdown" => Some("md"),
        "application/json" => Some("json"),
        "text/plain" => Some("txt"),
        _ => None,
    }
}

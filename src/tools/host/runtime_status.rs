use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use crate::tools::host::source_promotion::{
    CachedSourcePromotionStatus, FileSourcePromotionService,
};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RuntimeExecutableProvenance {
    pub path: Option<String>,
    pub source_commit: Option<String>,
    pub source_exact: bool,
    pub release_tag: Option<String>,
}

impl RuntimeExecutableProvenance {
    pub fn embedded(path: Option<String>) -> Self {
        Self {
            path,
            source_commit: option_env!("REFINE_BUILD_SOURCE_COMMIT").map(str::to_string),
            source_exact: option_env!("REFINE_BUILD_SOURCE_EXACT") == Some("true"),
            release_tag: option_env!("REFINE_BUILD_RELEASE_TAG").map(str::to_string),
        }
    }

    fn is_published_release(&self, version: &str) -> bool {
        self.source_exact
            && self.source_commit.is_some()
            && self
                .release_tag
                .as_deref()
                .is_some_and(|tag| tag == version || tag == format!("v{version}"))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RuntimeUpgradeStatus {
    pub runtime_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upgrade_available: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceRuntimeStatus>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SourceRuntimeStatus {
    pub executable: RuntimeExecutableProvenance,
    pub checkout: SourceCheckoutStatus,
    pub upstream: SourceUpstreamStatus,
    pub running_from_head: Option<bool>,
    pub relationship: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unknown_reason: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SourceCheckoutStatus {
    pub path: String,
    pub head_commit: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SourceUpstreamStatus {
    pub remote: Option<String>,
    pub branch: Option<String>,
    pub commit: Option<String>,
    pub cached_checkout_commit: Option<String>,
    pub last_successful_check_at: Option<String>,
    pub freshness: String,
    pub relationship: String,
}

#[derive(Clone, Debug)]
pub struct FileRuntimeStatusService {
    checkout_path: PathBuf,
    port_runtime_root: Option<PathBuf>,
    port: u16,
    provenance: RuntimeExecutableProvenance,
}

impl FileRuntimeStatusService {
    pub fn new(
        checkout_path: impl Into<PathBuf>,
        port_runtime_root: Option<PathBuf>,
        port: u16,
        executable_path: Option<String>,
    ) -> Self {
        Self {
            checkout_path: checkout_path.into(),
            port_runtime_root,
            port,
            provenance: RuntimeExecutableProvenance::embedded(executable_path),
        }
    }

    #[cfg(test)]
    fn with_provenance(mut self, provenance: RuntimeExecutableProvenance) -> Self {
        self.provenance = provenance;
        self
    }

    pub fn inspect(&self, current_version: &str, latest_version: &str) -> RuntimeUpgradeStatus {
        if self.provenance.is_published_release(current_version) {
            let upgrade_available = latest_version != current_version;
            return RuntimeUpgradeStatus {
                runtime_kind: "published_release".to_string(),
                current_version: Some(current_version.to_string()),
                latest_version: Some(latest_version.to_string()),
                available: Some(upgrade_available),
                upgrade_available: Some(upgrade_available),
                validation: Some("exact_version_tag".to_string()),
                command: self.provenance.path.clone(),
                source: None,
            };
        }

        let checkout_head = read_checkout_head(&self.checkout_path).ok();
        let cached = self.port_runtime_root.as_ref().and_then(|runtime_root| {
            FileSourcePromotionService::new(&self.checkout_path, runtime_root, self.port)
                .inspect_cached()
                .ok()
        });
        RuntimeUpgradeStatus {
            runtime_kind: "source".to_string(),
            current_version: None,
            latest_version: None,
            available: None,
            upgrade_available: None,
            validation: None,
            command: None,
            source: Some(classify_source_runtime(
                &self.checkout_path,
                self.provenance.clone(),
                checkout_head,
                cached.as_ref(),
            )),
        }
    }
}

fn classify_source_runtime(
    checkout_path: &Path,
    executable: RuntimeExecutableProvenance,
    checkout_head: Option<String>,
    cached: Option<&CachedSourcePromotionStatus>,
) -> SourceRuntimeStatus {
    let cached_checkout_commit =
        cached.and_then(|status| status.check.current_source_identity.clone());
    let upstream_commit = cached.and_then(|status| status.check.available_source_identity.clone());
    let freshness = cached
        .map(|status| status.check.freshness.clone())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    let cached_relationship = cached
        .map(|status| status.source.relationship.clone())
        .filter(|value| matches!(value.as_str(), "current" | "behind" | "ahead" | "diverged"))
        .unwrap_or_else(|| "unknown".to_string());
    let remote = cached
        .map(|status| status.source.remote.clone())
        .filter(|value| !value.is_empty());
    let branch = cached
        .map(|status| status.source.branch.clone())
        .filter(|value| !value.is_empty());

    let unknown_reason = source_unknown_reason(
        checkout_path,
        &executable,
        checkout_head.as_deref(),
        cached,
        &freshness,
        &cached_relationship,
    );
    let trusted = unknown_reason.is_none();
    SourceRuntimeStatus {
        executable,
        checkout: SourceCheckoutStatus {
            path: checkout_path.display().to_string(),
            head_commit: checkout_head,
        },
        upstream: SourceUpstreamStatus {
            remote,
            branch,
            commit: upstream_commit,
            cached_checkout_commit,
            last_successful_check_at: cached
                .and_then(|status| status.check.last_successful_check_at.clone()),
            freshness,
            relationship: if trusted {
                cached_relationship.clone()
            } else {
                "unknown".to_string()
            },
        },
        running_from_head: trusted.then_some(true),
        relationship: if trusted {
            cached_relationship
        } else {
            "unknown".to_string()
        },
        unknown_reason,
    }
}

fn source_unknown_reason(
    checkout_path: &Path,
    executable: &RuntimeExecutableProvenance,
    checkout_head: Option<&str>,
    cached: Option<&CachedSourcePromotionStatus>,
    freshness: &str,
    cached_relationship: &str,
) -> Option<String> {
    if !executable.source_exact || executable.source_commit.is_none() {
        return Some("executable_provenance_unverified".to_string());
    }
    let Some(checkout_head) = checkout_head else {
        return Some("checkout_head_unavailable".to_string());
    };
    if executable.source_commit.as_deref() != Some(checkout_head) {
        return Some("executable_checkout_mismatch".to_string());
    }
    let Some(cached) = cached else {
        return Some("upstream_cache_unavailable".to_string());
    };
    if Path::new(&cached.source.checkout_path) != checkout_path {
        return Some("cached_checkout_path_mismatch".to_string());
    }
    if freshness != "fresh" {
        return Some(format!("upstream_cache_{freshness}"));
    }
    let Some(cached_checkout_commit) = cached.check.current_source_identity.as_deref() else {
        return Some("cached_checkout_identity_unavailable".to_string());
    };
    if cached_checkout_commit != checkout_head || cached.source.current_commit != checkout_head {
        return Some("cached_checkout_identity_mismatch".to_string());
    }
    let Some(upstream_commit) = cached.check.available_source_identity.as_deref() else {
        return Some("cached_upstream_identity_unavailable".to_string());
    };
    if cached.source.available_commit != upstream_commit {
        return Some("cached_upstream_identity_mismatch".to_string());
    }
    if cached_relationship == "unknown" {
        return Some("cached_relationship_unavailable".to_string());
    }
    if (cached_checkout_commit == upstream_commit) != (cached_relationship == "current") {
        return Some("cached_relationship_identity_mismatch".to_string());
    }
    None
}

fn read_checkout_head(checkout_path: &Path) -> Result<String, String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(checkout_path)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let commit = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!commit.is_empty())
        .then_some(commit)
        .ok_or_else(|| "Git returned an empty checkout HEAD".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::host::source_promotion::{SourcePromotionSnapshot, SourceUpdateCheckState};

    fn provenance(commit: &str) -> RuntimeExecutableProvenance {
        RuntimeExecutableProvenance {
            path: Some("/checkout/bin/refine".to_string()),
            source_commit: Some(commit.to_string()),
            source_exact: true,
            release_tag: None,
        }
    }

    fn cached(
        local: &str,
        upstream: &str,
        freshness: &str,
        relationship: &str,
    ) -> CachedSourcePromotionStatus {
        CachedSourcePromotionStatus {
            source: SourcePromotionSnapshot {
                checkout_path: "/checkout".to_string(),
                current_commit: local.to_string(),
                remote: "origin".to_string(),
                local_branch: "main".to_string(),
                branch: "main".to_string(),
                available_commit: upstream.to_string(),
                relationship: relationship.to_string(),
                clean: true,
                fast_forward: matches!(relationship, "current" | "behind"),
                update_available: local != upstream,
                active_work: Vec::new(),
                operation: None,
            },
            check: SourceUpdateCheckState {
                last_successful_check_at: Some("2026-08-17T12:00:00Z".to_string()),
                current_source_identity: Some(local.to_string()),
                available_source_identity: Some(upstream.to_string()),
                freshness: freshness.to_string(),
                ..Default::default()
            },
        }
    }

    #[test]
    fn published_release_requires_exact_matching_version_tag() {
        let mut embedded = RuntimeExecutableProvenance {
            release_tag: Some("4.2.0".to_string()),
            ..provenance("aaa")
        };
        assert!(embedded.is_published_release("4.2.0"));
        embedded.release_tag = Some("v4.2.0".to_string());
        assert!(embedded.is_published_release("4.2.0"));
        embedded.release_tag = Some("4.1.0".to_string());
        assert!(!embedded.is_published_release("4.2.0"));
        embedded.release_tag = Some("4.2.0".to_string());
        embedded.source_exact = false;
        assert!(!embedded.is_published_release("4.2.0"));

        let service =
            FileRuntimeStatusService::new("/checkout", Some(PathBuf::from("/runtime")), 8080, None)
                .with_provenance(RuntimeExecutableProvenance {
                    release_tag: Some("4.2.0".to_string()),
                    ..provenance("aaa")
                });
        let status = service.inspect("4.2.0", "4.3.0");
        assert_eq!(status.runtime_kind, "published_release");
        assert_eq!(status.upgrade_available, Some(true));
        assert!(status.source.is_none());
    }

    #[test]
    fn source_relationship_is_trusted_only_when_all_fresh_identities_agree() {
        let cached = cached("aaa", "bbb", "fresh", "behind");
        let status = classify_source_runtime(
            Path::new("/checkout"),
            provenance("aaa"),
            Some("aaa".to_string()),
            Some(&cached),
        );
        assert_eq!(status.running_from_head, Some(true));
        assert_eq!(status.relationship, "behind");
        assert_eq!(status.upstream.relationship, "behind");
        assert_eq!(status.unknown_reason, None);
    }

    #[test]
    fn stale_or_mismatched_evidence_fails_closed_with_specific_reason() {
        let stale = cached("aaa", "bbb", "stale", "behind");
        let stale_status = classify_source_runtime(
            Path::new("/checkout"),
            provenance("aaa"),
            Some("aaa".to_string()),
            Some(&stale),
        );
        assert_eq!(stale_status.running_from_head, None);
        assert_eq!(stale_status.relationship, "unknown");
        assert_eq!(
            stale_status.unknown_reason.as_deref(),
            Some("upstream_cache_stale")
        );

        let fresh = cached("aaa", "bbb", "fresh", "behind");
        let mismatch = classify_source_runtime(
            Path::new("/checkout"),
            provenance("ccc"),
            Some("aaa".to_string()),
            Some(&fresh),
        );
        assert_eq!(
            mismatch.unknown_reason.as_deref(),
            Some("executable_checkout_mismatch")
        );

        let mut inconsistent_cache = fresh;
        inconsistent_cache.check.current_source_identity = Some("ddd".to_string());
        let inconsistent = classify_source_runtime(
            Path::new("/checkout"),
            provenance("aaa"),
            Some("aaa".to_string()),
            Some(&inconsistent_cache),
        );
        assert_eq!(
            inconsistent.unknown_reason.as_deref(),
            Some("cached_checkout_identity_mismatch")
        );

        let mut wrong_checkout = cached("aaa", "bbb", "fresh", "behind");
        wrong_checkout.source.checkout_path = "/other-checkout".to_string();
        let wrong_checkout_status = classify_source_runtime(
            Path::new("/checkout"),
            provenance("aaa"),
            Some("aaa".to_string()),
            Some(&wrong_checkout),
        );
        assert_eq!(
            wrong_checkout_status.unknown_reason.as_deref(),
            Some("cached_checkout_path_mismatch")
        );

        let contradictory = cached("aaa", "aaa", "fresh", "behind");
        let contradictory_status = classify_source_runtime(
            Path::new("/checkout"),
            provenance("aaa"),
            Some("aaa".to_string()),
            Some(&contradictory),
        );
        assert_eq!(
            contradictory_status.unknown_reason.as_deref(),
            Some("cached_relationship_identity_mismatch")
        );
    }
}

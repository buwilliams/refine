const MAX_SYNC_ERROR_CHARS: usize = 2_048;
const SENSITIVE_ASSIGNMENTS: &[&str] = &[
    "access_token=",
    "authorization=",
    "github_token=",
    "oauth_token=",
    "password=",
    "token=",
];

pub fn redact_sync_error(error: &str) -> String {
    let normalized = error
        .split_whitespace()
        .map(redact_error_token)
        .collect::<Vec<_>>()
        .join(" ");
    if normalized.chars().count() <= MAX_SYNC_ERROR_CHARS {
        return normalized;
    }
    normalized
        .chars()
        .take(MAX_SYNC_ERROR_CHARS.saturating_sub(1))
        .chain(std::iter::once('\u{2026}'))
        .collect()
}

fn redact_error_token(token: &str) -> String {
    let token = redact_url_userinfo(token);
    let lower = token.to_ascii_lowercase();
    let assignment = SENSITIVE_ASSIGNMENTS
        .iter()
        .filter_map(|marker| {
            lower.match_indices(marker).find_map(|(index, _)| {
                assignment_boundary(&lower, index).then_some((index, marker.len()))
            })
        })
        .min_by_key(|(index, marker_len)| (*index, std::cmp::Reverse(*marker_len)));
    let Some((index, marker_len)) = assignment else {
        return token;
    };
    format!("{}[REDACTED]", &token[..index + marker_len])
}

fn redact_url_userinfo(token: &str) -> String {
    let Some(scheme_end) = token.find("://").map(|index| index + 3) else {
        return token.to_string();
    };
    let Some(at) = token[scheme_end..]
        .find('@')
        .map(|index| scheme_end + index)
    else {
        return token.to_string();
    };
    format!("{}[REDACTED]{}", &token[..scheme_end], &token[at..])
}

fn assignment_boundary(value: &str, index: usize) -> bool {
    index == 0
        || (!value.as_bytes()[index - 1].is_ascii_alphanumeric()
            && value.as_bytes()[index - 1] != b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_url_userinfo_and_sensitive_assignments_anywhere_in_a_token() {
        let error = "git fetch https://user:secret@example.com/repo?access_token=secret \
                     Authorization=Basic-secret GITHUB_TOKEN=also-secret nottoken=visible";

        let redacted = redact_sync_error(error);

        assert_eq!(
            redacted,
            "git fetch https://[REDACTED]@example.com/repo?access_token=[REDACTED] \
             Authorization=[REDACTED] GITHUB_TOKEN=[REDACTED] nottoken=visible"
        );
    }

    #[test]
    fn bounds_persisted_and_projected_errors() {
        let redacted = redact_sync_error(&"x".repeat(MAX_SYNC_ERROR_CHARS + 100));

        assert_eq!(redacted.chars().count(), MAX_SYNC_ERROR_CHARS);
        assert!(redacted.ends_with('\u{2026}'));
    }
}

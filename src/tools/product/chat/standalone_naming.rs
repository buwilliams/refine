pub(super) fn derive_standalone_goal_name(prompt: &str) -> Option<String> {
    let source = prompt.trim();
    if source.is_empty() {
        return None;
    }
    let collapsed = source.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut name = collapsed.chars().take(80).collect::<String>();
    if collapsed.chars().count() > 80 {
        name = name
            .trim_end_matches(|ch: char| !ch.is_alphanumeric())
            .to_string();
    }
    (!name.trim().is_empty()).then(|| name.trim().to_string())
}

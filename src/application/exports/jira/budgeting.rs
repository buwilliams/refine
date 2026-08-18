#[derive(Debug)]
pub(super) struct EvidenceSection {
    pub(super) omission_label: &'static str,
    pub(super) text: String,
    pub(super) reserve: usize,
}

impl EvidenceSection {
    pub(super) fn new(omission_label: &'static str, text: String, reserve: usize) -> Self {
        Self {
            omission_label,
            text,
            reserve,
        }
    }
}

pub(super) fn render_budgeted_sections(sections: &[EvidenceSection], limit: usize) -> String {
    let mut rendered = String::new();
    for (index, section) in sections.iter().enumerate() {
        let separator_len = usize::from(index > 0) * 2;
        let later_reserve = sections[index + 1..]
            .iter()
            .map(|later| {
                let full_len = later.text.chars().count() + 2;
                full_len.min(later.reserve)
            })
            .sum::<usize>();
        let used = rendered.chars().count();
        let available = limit.saturating_sub(used).saturating_sub(later_reserve);
        if available <= separator_len {
            continue;
        }
        if separator_len > 0 {
            rendered.push_str("\n\n");
        }
        let content_limit = available - separator_len;
        rendered.push_str(&truncate_with_marker(
            &section.text,
            content_limit,
            section.omission_label,
        ));
    }
    rendered
}

pub(super) fn truncate_with_marker(value: &str, limit: usize, label: &str) -> String {
    let total = value.chars().count();
    if total <= limit {
        return value.to_string();
    }

    let shortest_marker = format!("[omitted: {label}]");
    if shortest_marker.chars().count() > limit {
        return shortest_marker.chars().take(limit).collect();
    }

    let mut retained = limit.saturating_sub(shortest_marker.chars().count());
    loop {
        let omitted = total.saturating_sub(retained);
        let marker = format!("\n[shortened: {label}; {omitted} characters omitted]");
        let next_retained = limit.saturating_sub(marker.chars().count());
        if next_retained == retained {
            return value.chars().take(retained).chain(marker.chars()).collect();
        }
        retained = next_retained;
    }
}

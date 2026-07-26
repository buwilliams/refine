use super::*;

pub(super) fn parse_csv_rows(text: &str) -> RefineResult<Vec<Vec<String>>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut cell = String::new();
    let mut chars = text.chars().peekable();
    let mut quoted = false;
    while let Some(ch) = chars.next() {
        match ch {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cell.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => {
                row.push(cell.trim().to_string());
                cell.clear();
            }
            '\n' if !quoted => {
                row.push(cell.trim_end_matches('\r').trim().to_string());
                cell.clear();
                rows.push(row);
                row = Vec::new();
            }
            _ => cell.push(ch),
        }
    }
    if quoted {
        return Err(RefineError::InvalidInput(
            "CSV contains an unclosed quoted field".to_string(),
        ));
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell.trim_end_matches('\r').trim().to_string());
        rows.push(row);
    }
    Ok(rows)
}

pub(super) fn import_name(name: &str, prompt: &str) -> String {
    let raw = [name.trim(), prompt.trim()]
        .into_iter()
        .find(|value| !value.is_empty())
        .unwrap_or("Imported Goal");
    let mut result: String = raw.chars().take(80).collect();
    if result.trim().is_empty() {
        result = "Imported Goal".to_string();
    }
    result
}

pub(super) fn normalized_priority(priority: &str) -> RefineResult<String> {
    let priority = priority.trim().to_lowercase();
    let priority = if priority.is_empty() {
        "low".to_string()
    } else {
        priority
    };
    match priority.as_str() {
        "low" | "medium" | "high" => Ok(priority),
        _ => Err(RefineError::InvalidInput(
            "priority must be one of low, medium, or high".to_string(),
        )),
    }
}

pub(super) fn nonempty_or<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    let value = value.trim();
    if value.is_empty() { fallback } else { value }
}

pub(super) fn nonempty_option(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

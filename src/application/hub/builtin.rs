//! Product-owned documentation ships with the executable, never in project state.
use super::*;
use pulldown_cmark::{Options, Parser, html};
pub const ID: &str = "refine";
const FILES: &[(&str, &[u8])] = include!(concat!(env!("OUT_DIR"), "/refine_hub.rs"));
pub fn site() -> Value {
    json!({"id":ID,"name":"Refine Hub","description":"Product documentation and what you need to know about each release.","builtin":true,"read_only":true,"publication":{"version":env!("CARGO_PKG_VERSION")}})
}
pub fn raw(name: &str) -> RefineResult<&'static [u8]> {
    FILES
        .iter()
        .find(|(path, _)| *path == name)
        .map(|(_, bytes)| *bytes)
        .ok_or_else(|| RefineError::NotFound("Refine Hub page not found".into()))
}
fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
fn title(name: &str, bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .find_map(|line| line.strip_prefix("# "))
        .unwrap_or(name)
        .to_string()
}
pub fn manifest() -> Value {
    let mut files = serde_json::Map::new();
    for (name, bytes) in FILES {
        files.insert(
            (*name).into(),
            json!({"bytes":bytes.len(),"hash":digest(bytes)}),
        );
    }
    Value::Object(files)
}
pub fn page(name: &str) -> RefineResult<Vec<u8>> {
    let name = if name.is_empty() || name == "index.html" {
        "index.md"
    } else {
        name
    };
    let bytes = raw(name)?;
    if !name.ends_with(".md") {
        return Ok(bytes.to_vec());
    }
    let base = name
        .rsplit_once('/')
        .map(|(parent, _)| format!("{parent}/"))
        .unwrap_or_default();
    let markdown = String::from_utf8_lossy(bytes);
    let mut content = String::new();
    html::push_html(&mut content, Parser::new_ext(&markdown, Options::all()));
    let groups = [
        ("Start here", "product/"),
        ("What's new", "releases/"),
        ("Runbooks", "docs/runbooks/"),
        ("Design intent", "docs/intent/"),
        ("About Refine", ""),
    ];
    let mut nav = String::new();
    for (label, prefix) in groups {
        nav.push_str(&format!(
            "<details {}><summary>{label}</summary>",
            if name.starts_with(prefix) && !prefix.is_empty() {
                "open"
            } else {
                ""
            }
        ));
        for (path, bytes) in FILES.iter().filter(|(path, _)| {
            path.ends_with(".md")
                && if prefix.is_empty() {
                    !path.starts_with("product/")
                        && !path.starts_with("releases/")
                        && !path.starts_with("docs/runbooks/")
                        && !path.starts_with("docs/intent/")
                        && *path != "index.md"
                } else {
                    path.starts_with(prefix)
                }
        }) {
            nav.push_str(&format!(
                "<a {} href=\"/hub/sites/refine/{}\">{}</a>",
                if *path == name {
                    "aria-current=\"page\""
                } else {
                    ""
                },
                escape(path),
                escape(&title(path, bytes))
            ));
        }
        nav.push_str("</details>");
    }
    Ok(format!(r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>{} · Refine Hub</title><base href="/hub/sites/refine/{base}"><link rel="stylesheet" href="/hub/sites/refine/hub.css"><script defer src="/hub/sites/refine/hub.js"></script></head><body><header><a href="/hub/sites/refine/">Refine Hub</a><span>Built-in · Refine {}</span></header><div class="layout"><nav aria-label="Documentation"><label for="hub-search">Find a page</label><input id="hub-search" type="search" placeholder="Search documentation"><p id="hub-search-status" role="status"></p>{nav}</nav><main><article>{content}</article><footer>Ships with Refine · <a href="/hub/sites/refine/authoring.md">Contribute to this hub</a></footer></main></div></body></html>"#,escape(&title(name,bytes)),env!("CARGO_PKG_VERSION")).into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bundled_document_links_resolve_within_the_hub() {
        use pulldown_cmark::{Event, Tag};
        for (name, bytes) in FILES.iter().filter(|(name, _)| name.ends_with(".md")) {
            let markdown = String::from_utf8_lossy(bytes);
            for event in Parser::new_ext(&markdown, Options::all()) {
                let destination = match event {
                    Event::Start(Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. }) => {
                        dest_url
                    }
                    _ => continue,
                };
                let target = destination
                    .split('#')
                    .next()
                    .unwrap()
                    .split('?')
                    .next()
                    .unwrap();
                if target.is_empty() || target.contains(':') || target.starts_with('/') {
                    continue;
                }
                let joined = Path::new(name).parent().unwrap().join(target);
                let mut parts = Vec::new();
                for component in joined.components() {
                    match component {
                        std::path::Component::Normal(value) => parts.push(value.to_str().unwrap()),
                        std::path::Component::ParentDir => {
                            assert!(parts.pop().is_some(), "{name}: {destination}");
                        }
                        std::path::Component::CurDir => {}
                        _ => panic!("{name}: invalid link {destination}"),
                    }
                }
                assert!(
                    raw(&parts.join("/")).is_ok(),
                    "{name}: broken link {destination}"
                );
            }
        }
    }
}

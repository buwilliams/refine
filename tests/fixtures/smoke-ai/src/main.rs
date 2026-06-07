use std::env;
use std::io::{self, Read, Write};

const CHAT: &str = "smoke-ai chat response\n\nI can discuss the requested Refine workflow deterministically for integration tests.\n";
const FALLBACK: &str =
    "smoke-ai fallback response\n\nThis deterministic fallback is used when no smoke-ai matcher applies.\n";
const GAP_AGENT: &str = "smoke-ai gap-agent response\n\nThe disposable refine-smoke Gap has been reviewed and no source changes are required.\n";
const GOVERNANCE: &str =
    "smoke-ai governance response\n\nUse clear ownership, reversible changes, and explicit cleanup for refine-smoke artifacts.\n";
const IMPORT: &str = "{\"kind\": \"import\", \"name\": \"refine-smoke imported gap one\", \"priority\": \"low\"}\n{\"kind\": \"import\", \"name\": \"refine-smoke imported gap two\", \"priority\": \"low\"}\n";
const PREFLIGHT: &str = "hello\n";
const TARGET_APP: &str = "{\n  \"kind\": \"target-app\",\n  \"ok\": true,\n  \"message\": \"refine-smoke target app check passed\"\n}\n";

const MATCHERS: &[(&str, &[&str], &str, &str)] = &[
    (
        "preflight",
        &[
            "single word hello",
            "exactly the single word hello",
            "say exactly",
            "preflight",
        ],
        "preflight.md",
        PREFLIGHT,
    ),
    (
        "gap-agent",
        &["gap agent", "ready gap", "work on gap"],
        "gap-agent.md",
        GAP_AGENT,
    ),
    (
        "import",
        &["import", "csv", "rows", "dedup"],
        "import.jsonl",
        IMPORT,
    ),
    (
        "target-app",
        &["target app", "health", "start command", "rebuild"],
        "target-app.json",
        TARGET_APP,
    ),
    (
        "governance",
        &["governance", "constitution", "rules"],
        "governance.md",
        GOVERNANCE,
    ),
    (
        "chat",
        &["chat", "conversation", "message", "defect"],
        "chat.md",
        CHAT,
    ),
];

fn main() {
    let prompt = env::args().skip(1).collect::<Vec<_>>().join(" ");
    let mut stdin = String::new();
    let _ = io::stdin().read_to_string(&mut stdin);
    let haystack = format!("{prompt}\n{stdin}").to_lowercase();

    let mut selected_name = "fallback";
    let mut selected_template = "fallback.md";
    let mut output = FALLBACK;
    for (name, needles, template, body) in MATCHERS {
        if needles.iter().any(|needle| haystack.contains(needle)) {
            selected_name = name;
            selected_template = template;
            output = body;
            break;
        }
    }

    if env::var("SMOKE_AI_DEBUG").as_deref() == Ok("1") {
        let _ = writeln!(
            io::stderr(),
            "smoke-ai matcher={selected_name} template={selected_template}"
        );
    }
    print!("{output}");
}

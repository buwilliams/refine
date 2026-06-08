use std::env;
use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::thread;
use std::time::Duration;

const CHAT: &str = "smoke-ai chat response\n\nI can discuss the requested Refine workflow deterministically for integration tests.\n";
const FALLBACK: &str = "smoke-ai fallback response\n\nThis deterministic fallback is used when no smoke-ai matcher applies.\n";
const GAP_AGENT: &str = "smoke-ai gap-agent response\n\nThe disposable refine-smoke Gap has been reviewed and no source changes are required.\n";
const GOVERNANCE: &str = "smoke-ai governance response\n\nUse clear ownership, reversible changes, and explicit cleanup for refine-smoke artifacts.\n";
const IMPORT: &str = "{\"kind\": \"import\", \"name\": \"refine-smoke imported gap one\", \"priority\": \"low\"}\n{\"kind\": \"import\", \"name\": \"refine-smoke imported gap two\", \"priority\": \"low\"}\n";
const PLAN: &str = "smoke-ai plan actual behavior one => smoke-ai plan target behavior one\nsmoke-ai plan actual behavior two => smoke-ai plan target behavior two\n";
const ROUND: &str = "smoke-ai round actual behavior => smoke-ai round target behavior\n";
const STANDALONE_GAP: &str =
    "smoke-ai standalone actual behavior => smoke-ai standalone target behavior\n";
const PREFLIGHT: &str = "hello\n";
const TARGET_APP: &str = "{\n  \"kind\": \"target-app\",\n  \"ok\": true,\n  \"message\": \"refine-smoke target app check passed\",\n  \"start_command\": \"printf smoke-ai-target-start\",\n  \"stop_command\": \"printf smoke-ai-target-stop\",\n  \"rebuild_command\": \"printf smoke-ai-target-rebuild\",\n  \"status_command\": \"printf smoke-ai-target-status\",\n  \"cwd\": \".\",\n  \"env\": {\"REFINE_SMOKE_TARGET\": \"1\"},\n  \"start_timeout_seconds\": 11,\n  \"stop_timeout_seconds\": 12,\n  \"rebuild_timeout_seconds\": 13,\n  \"status_timeout_seconds\": 14,\n  \"log_path\": \"target/refine-smoke-target.log\",\n  \"http_check_url\": \"http://127.0.0.1:3456/health\",\n  \"tcp_check_host\": \"127.0.0.1\",\n  \"tcp_check_port\": \"3456\",\n  \"process_check_command\": \"printf smoke-ai-target-process\",\n  \"notes\": \"smoke-ai target-app generated config\"\n}\n";

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
        "plan",
        &["draft feature", "plan draft", "planned feature"],
        "plan.txt",
        PLAN,
    ),
    (
        "standalone-gap",
        &["standalone gap", "draft one standalone gap"],
        "standalone-gap.txt",
        STANDALONE_GAP,
    ),
    (
        "round",
        &["extract round", "draft round", "gap round"],
        "round.txt",
        ROUND,
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
    if selected_name == "gap-agent" && env::var("SMOKE_AI_EDIT_APP").as_deref() == Ok("1") {
        if let Ok(mut file) = OpenOptions::new().append(true).open("app.py") {
            let _ = writeln!(file, "\n# edited by smoke-ai gap-agent");
        }
    }
    if haystack.contains("smoke-ai queue delay") {
        thread::sleep(Duration::from_millis(4_000));
    }
    print!("{output}");
}

mod provider_turns;
mod queue;
mod sessions;
mod standalone;

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use super::*;
use crate::application::work_items::FileWorkItemService;
use crate::infrastructure::process::supervisor::operations::{
    FileOperationRegistry, OperationRegistry, OperationState,
};

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = std::env::temp_dir()
        .canonicalize()
        .unwrap_or_else(|_| std::env::temp_dir());
    temp_root.join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
}

fn init_git_app(repo: &Path) {
    fs::create_dir_all(repo.join(".refine")).unwrap();
    git(repo, &["init", "-b", "main"]);
    git(repo, &["config", "user.email", "test@example.com"]);
    git(repo, &["config", "user.name", "Test User"]);
    fs::write(repo.join("app.txt"), "base\n").unwrap();
    git(repo, &["add", "app.txt"]);
    git(repo, &["commit", "-m", "initial"]);
}

fn init_unborn_git_app(repo: &Path) {
    fs::create_dir_all(repo.join(".refine")).unwrap();
    git(repo, &["init", "-b", "main"]);
}

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed\nstdout:\n{}\nstderr:\n{}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_stdout(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed\nstdout:\n{}\nstderr:\n{}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn wait_for_chat_line(service: &FileChatService, session_id: &str, needle: &str) -> ChatReadResult {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let read = service.read(session_id).unwrap();
        if read.lines.iter().any(|line| line.contains(needle))
            || read.progress_lines.iter().any(|line| line.contains(needle))
        {
            return read;
        }
        if Instant::now() >= deadline {
            return read;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn wait_for_chat_read<F>(
    service: &FileChatService,
    session_id: &str,
    predicate: F,
) -> ChatReadResult
where
    F: Fn(&ChatReadResult) -> bool,
{
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let read = service.read(session_id).unwrap();
        if predicate(&read) {
            return read;
        }
        if Instant::now() >= deadline {
            return read;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn wait_for_chat_record<F>(
    service: &FileChatService,
    session_id: &str,
    predicate: F,
) -> ChatSessionRecord
where
    F: Fn(&ChatSessionRecord) -> bool,
{
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let record = service.resume(session_id).unwrap();
        if predicate(&record) {
            return record;
        }
        if Instant::now() >= deadline {
            return record;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn write_fake_provider_script(refine_dir: &Path, name: &str, script: &str) {
    let bin_dir = refine_dir.join("provider-bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let path = bin_dir.join(name);
    fs::write(&path, script).unwrap();
    make_provider_executable(&path);
}

fn write_fake_provider(refine_dir: &Path, name: &str, exit_code: i32, output: &str) {
    let bin_dir = refine_dir.join("provider-bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let path = bin_dir.join(name);
    let mut file = fs::File::create(&path).unwrap();
    writeln!(
        file,
        "#!/bin/sh\nprintf '%s\\n' {output:?}\nexit {exit_code}"
    )
    .unwrap();
    make_provider_executable(&path);
}

fn make_provider_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }
}

fn write_cwd_provider(refine_dir: &Path, name: &str) {
    let bin_dir = refine_dir.join("provider-bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let path = bin_dir.join(name);
    let mut file = fs::File::create(&path).unwrap();
    writeln!(
        file,
        "#!/bin/sh\npwd > provider-cwd.txt\nprintf '%s\\n' 'cwd provider response'"
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
    }
}

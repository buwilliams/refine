use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

const TEST_RELEASE_TAG: &str = "99.0.0";

#[test]
#[ignore = "requires docker and builds Refine inside a disposable Linux container"]
fn shell_install_and_cli_uninstall_work_in_linux_container() {
    let fixture = DockerInstallFixture::start();

    let installer = fixture.exec_with_stdin(
        r#"
set -eu
git config --global --add safe.directory /origin/refine
cd /tmp
REFINE_REPO_URL=file:///origin/refine \
REFINE_INSTALL_CHECKOUT_DEFAULT=/home/refine/refine \
REFINE_INSTALL_ALLOW_TEST_PROVIDERS=1 \
REFINE_INSTALL_PROVIDER=smoke-ai \
REFINE_SMOKE_AI_PATH=/bin/true \
REFINE_INSTALL_TARGET_APP= \
REFINE_INSTALL_PORT=19080 \
REFINE_DEFAULT_PORT=19080 \
REFINE_INSTALL_LOG=/home/refine/install.log \
bash /origin/refine/scripts/install.sh
"#,
        "y\n/home/refine/refine\nn\nn\n19080\n",
    );
    assert_success("run shell installer in Docker", &installer);

    let installed = fixture.exec(
        r#"
set -eu
test -x /home/refine/refine/bin/refine
grep -q '^mode=deployed$' /home/refine/refine/.refine-deployed
cd /home/refine/refine
./r system status --port 19080 --runtime-root run
"#,
    );
    assert_success("verify installed binary and running daemon", &installed);

    let install_state = fixture.exec(
        r#"
set -eu
cd /home/refine/refine
metadata_path="$(
  ./r system install --target linux-cli-web --runtime-root run --version 3.0.0 \
    --port 19080 \
    | tee /tmp/refine-system-install.json \
    | sed -n 's/.*"service_metadata_path": "\([^"]*\)".*/\1/p' \
    | head -n 1
)"
test -n "$metadata_path"
test -f "$metadata_path"
test -f run/19080/install-state.json
test -f run/19080/install-backend.json
grep -q '"installed": true' run/19080/install-state.json
grep -q '"port": 19080' run/19080/install-state.json
printf '%s\n' "$metadata_path" >/tmp/refine-service-metadata-path
"#,
    );
    assert_success("install persistent metadata in Docker", &install_state);

    let uninstall = fixture.exec(
        r#"
set -eu
cd /home/refine/refine
metadata_path="$(cat /tmp/refine-service-metadata-path)"
./r system uninstall --port 19080 --runtime-root run --version 3.0.0
test -f run/19080/install-state.json
test ! -f run/19080/install-backend.json
test ! -e "$metadata_path"
grep -q '"installed": false' run/19080/install-state.json
./r system stop --port 19080 --runtime-root run || true
"#,
    );
    assert_success("uninstall persistent metadata in Docker", &uninstall);
}

struct DockerInstallFixture {
    root: PathBuf,
    image: String,
    container: String,
}

impl DockerInstallFixture {
    fn start() -> Self {
        let repo_root = repo_root();
        let root = repo_root
            .join("target/refine-integration/install-uninstall")
            .join(unique_suffix());
        let source = root.join("source");
        fs::create_dir_all(&root).expect("failed to create Docker install artifact root");

        let suffix = unique_suffix();
        let image = format!("refine-install-uninstall:{suffix}");
        let container = format!("refine-install-uninstall-{suffix}");
        let dockerfile_dir = repo_root.join("tests/fixtures/install-uninstall");

        prepare_source_repo(&repo_root, &source);

        run_command(
            Command::new("docker")
                .args(["build", "-t", &image])
                .arg(&dockerfile_dir),
            "build install/uninstall Docker fixture",
        );

        run_command(
            Command::new("docker")
                .args(["run", "-d", "--name", &container])
                .arg("-v")
                .arg(format!("{}:/origin/refine:ro", source.display()))
                .arg(&image),
            "start install/uninstall Docker fixture",
        );

        Self {
            root,
            image,
            container,
        }
    }

    fn exec(&self, script: &str) -> Output {
        self.exec_inner(script, None)
    }

    fn exec_with_stdin(&self, script: &str, stdin: &str) -> Output {
        self.exec_inner(script, Some(stdin))
    }

    fn exec_inner(&self, script: &str, stdin: Option<&str>) -> Output {
        let mut command = Command::new("docker");
        command
            .args(["exec", "-i", &self.container, "bash", "-lc", script])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if stdin.is_some() {
            command.stdin(Stdio::piped());
        }
        let mut child = command
            .spawn()
            .unwrap_or_else(|error| panic!("failed to exec Docker fixture: {error}"));
        if let Some(input) = stdin {
            let mut child_stdin = child.stdin.take().expect("docker exec stdin unavailable");
            child_stdin
                .write_all(input.as_bytes())
                .expect("failed to write docker exec stdin");
        }
        child
            .wait_with_output()
            .expect("failed to wait for docker exec")
    }
}

impl Drop for DockerInstallFixture {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "-f", &self.container])
            .output();
        let _ = Command::new("docker").args(["rmi", &self.image]).output();
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn assert_success(label: &str, output: &Output) {
    assert!(
        output.status.success(),
        "{label} failed with status {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_command(command: &mut Command, label: &str) {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("failed to {label}: {error}"));
    assert_success(label, &output);
}

fn prepare_source_repo(repo_root: &Path, source: &Path) {
    run_command(
        Command::new("git")
            .args(["clone", "--local"])
            .arg(repo_root)
            .arg(source),
        "clone Refine source for Docker installer origin",
    );
    run_command(
        Command::new("git")
            .args(["tag", "-f", TEST_RELEASE_TAG])
            .current_dir(source),
        "tag Docker installer source release",
    );
}

fn repo_root() -> PathBuf {
    let mut current = std::env::current_dir().expect("failed to inspect cwd");
    loop {
        if current.join("docs/spec/rust-spec.md").is_file() {
            return current;
        }
        assert!(current.pop(), "failed to locate repository root");
    }
}

fn unique_suffix() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("{}-{millis}", std::process::id())
}

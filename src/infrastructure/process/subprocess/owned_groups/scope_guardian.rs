//! Dedicated exec helper: establish subreaper coverage, gate launch, then reap the whole scope.
use super::launch_scope::io_error;
use super::*;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::{net::UnixStream, process::CommandExt};

#[derive(Serialize, Deserialize)]
pub(super) struct Request {
    pub workload: ManagedProcessSpec,
    pub parent_pid: u32,
    pub proof_path: PathBuf,
}
pub(super) fn send<T: Serialize>(stream: &mut UnixStream, value: &T) -> RefineResult<()> {
    let bytes = serde_json::to_vec(value).map_err(|e| RefineError::Serialization(e.to_string()))?;
    stream
        .write_all(&(bytes.len() as u64).to_ne_bytes())
        .and_then(|()| stream.write_all(&bytes))
        .map_err(io_error)
}
pub(super) fn receive<T: serde::de::DeserializeOwned>(stream: &mut UnixStream) -> RefineResult<T> {
    let mut length = [0; 8];
    stream.read_exact(&mut length).map_err(io_error)?;
    let length = u64::from_ne_bytes(length);
    if length > 16 * 1024 * 1024 {
        return Err(RefineError::InvalidInput(
            "ownership handshake exceeds limit".into(),
        ));
    }
    let mut bytes = vec![0; length as usize];
    stream.read_exact(&mut bytes).map_err(io_error)?;
    serde_json::from_slice(&bytes).map_err(|e| RefineError::Serialization(e.to_string()))
}
pub(super) fn helper_command(mode: &str, socket: &UnixStream) -> RefineResult<Command> {
    let mut command = Command::new(PathBuf::from("/proc/self/exe"));
    command.args(["--refine-owned-scope", mode]);
    #[cfg(test)]
    command.env("REFINE_SCOPE_BOOTSTRAP", mode);
    let fd = socket.as_raw_fd();
    // Only descriptor setup occurs between fork and exec; no nested fork or Rust runtime.
    unsafe {
        command.pre_exec(move || {
            if libc::dup2(fd, 3) < 0 || libc::fcntl(3, libc::F_SETFD, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(command)
}
pub fn run_if_requested() {
    let mut args = std::env::args();
    let _ = args.next();
    if args.next().as_deref() == Some("--refine-owned-scope") {
        let mode = args.next().unwrap_or_default();
        finish(&mode, args.next().as_deref());
    }
}
fn finish(mode: &str, socket: Option<&str>) -> ! {
    let result = run(mode, socket);
    if let Err(error) = result {
        let _ = writeln!(std::io::stderr(), "{error}");
    }
    std::process::exit(125)
}
fn run(mode: &str, socket: Option<&str>) -> RefineResult<()> {
    let mut connection = if mode == "guardian-socket" {
        use std::os::linux::net::SocketAddrExt;
        let address = std::os::unix::net::SocketAddr::from_abstract_name(
            socket.unwrap_or_default().as_bytes(),
        )
        .map_err(io_error)?;
        UnixStream::connect_addr(&address).map_err(io_error)?
    } else {
        unsafe {
            libc::fcntl(3, libc::F_SETFD, libc::FD_CLOEXEC);
            UnixStream::from_raw_fd(3)
        }
    };
    connection
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(io_error)?;
    connection
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(io_error)?;
    let request: Request = receive(&mut connection)?;
    if mode == "workload" {
        let mut gate = [0];
        connection.read_exact(&mut gate).map_err(io_error)?;
        if gate != *b"S" {
            return Err(RefineError::Conflict(
                "workload start was not authorized".into(),
            ));
        }
        drop(connection);
        let mut command = Command::new(&request.workload.command);
        command.args(&request.workload.args);
        #[cfg(test)]
        command
            .env_remove("REFINE_SCOPE_BOOTSTRAP")
            .env_remove("REFINE_SCOPE_SOCKET");
        if request
            .workload
            .metadata
            .get("pty_owned_scope")
            .and_then(Value::as_bool)
            == Some(true)
        {
            // Keep the controlling terminal while making workload signals target its group.
            unsafe {
                libc::signal(libc::SIGTTOU, libc::SIG_IGN);
                if libc::setpgid(0, 0) != 0 || libc::tcsetpgrp(0, libc::getpid()) != 0 {
                    return Err(io_error(std::io::Error::last_os_error()));
                }
                libc::signal(libc::SIGTTOU, libc::SIG_DFL);
                if request
                    .workload
                    .limits
                    .as_ref()
                    .is_some_and(|l| l.kill_on_parent_exit)
                {
                    libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM, 0, 0, 0);
                }
            }
        } else {
            configure_process_lifecycle(&mut command, &request.workload);
        }
        return Err(io_error(command.exec()));
    }
    if mode != "guardian" && mode != "guardian-socket" {
        return Err(RefineError::InvalidInput(
            "invalid ownership helper mode".into(),
        ));
    }
    if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } != 0 {
        return Err(io_error(std::io::Error::last_os_error()));
    }
    let (mut parent, inherited) = UnixStream::pair().map_err(io_error)?;
    parent
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(io_error)?;
    let mut child = helper_command("workload", &inherited)?
        .spawn()
        .map_err(io_error)?;
    drop(inherited);
    send(&mut parent, &request)?;
    let mut proof = OpenOptions::new()
        .write(true)
        .open(&request.proof_path)
        .map_err(io_error)?;
    proof
        .write_all(&child.id().to_ne_bytes())
        .map_err(io_error)?;
    send(&mut connection, &child.id())?;
    let mut gate = [0];
    if let Err(error) = connection.read_exact(&mut gate) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(io_error(error));
    }
    if gate != *b"S" {
        let _ = child.kill();
        let _ = child.wait();
        return Err(RefineError::Conflict(
            "registration handshake rejected".into(),
        ));
    }
    parent.write_all(&gate).map_err(io_error)?;
    drop(parent);
    drop(connection);
    // The guardian must not hold the workload's output or input streams open.
    unsafe {
        libc::close(0);
        libc::close(1);
        libc::close(2);
    }
    let mut leader_status = None;
    let watch_parent = request
        .workload
        .limits
        .as_ref()
        .is_some_and(|l| l.kill_on_parent_exit);
    let mut parent_signalled = false;
    loop {
        // The helper survives its launcher to reap the scope. Forward the requested
        // parent-death signal only while the workload is still our unreaped child.
        if watch_parent
            && !parent_signalled
            && unsafe { libc::getppid() } != request.parent_pid as i32
        {
            parent_signalled = true;
            if leader_status.is_none() {
                unsafe {
                    libc::kill(child.id() as i32, libc::SIGTERM);
                }
            }
        }
        let mut status = 0;
        let pid = unsafe {
            libc::waitpid(
                -1,
                &mut status,
                libc::__WALL | if watch_parent { libc::WNOHANG } else { 0 },
            )
        };
        if pid == child.id() as i32 {
            leader_status = Some(status);
            proof.write_all(&status.to_ne_bytes()).map_err(io_error)?;
            proof.sync_data().map_err(io_error)?;
        }
        if pid >= 0 {
            if pid == 0 {
                std::thread::sleep(Duration::from_millis(20));
            }
            continue;
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EINTR) {
            continue;
        }
        if error.raw_os_error() == Some(libc::ECHILD)
            && let Some(status) = leader_status
        {
            proof
                .write_all(b"exited\n")
                .and_then(|()| proof.sync_all())
                .map_err(io_error)?;
            // Preserve the workload's exit semantics independently of the ownership proof.
            if libc::WIFEXITED(status) {
                std::process::exit(libc::WEXITSTATUS(status));
            }
            unsafe {
                libc::signal(libc::WTERMSIG(status), libc::SIG_DFL);
                libc::raise(libc::WTERMSIG(status));
            }
            std::process::exit(128 + libc::WTERMSIG(status));
        }
        return Err(io_error(error));
    }
}
// Run the same helper before libtest initializes threads or emits harness output.
#[cfg(test)]
#[used]
#[unsafe(link_section = ".init_array")]
static TEST_BOOTSTRAP: extern "C" fn() = {
    extern "C" fn bootstrap() {
        if let Ok(mode) = std::env::var("REFINE_SCOPE_BOOTSTRAP") {
            finish(&mode, std::env::var("REFINE_SCOPE_SOCKET").ok().as_deref());
        }
    }
    bootstrap
};

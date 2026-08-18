use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::infrastructure::process::subprocess::{FileProcessSupervisor, ProcessOwner};

/// The minimum reservation for one complete managed-agent workload. Observed
/// usage may raise this reservation, but a small or incomplete sample must not
/// make automatic admission more aggressive.
const MIN_AGENT_MEMORY_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// How long a sample stays current. The scheduler asks for policy about once a
/// second; resampling that often would be wasted work, and host capacity does
/// not move meaningfully at that resolution.
const SAMPLE_TTL: Duration = Duration::from_secs(30);

static CACHED_SAMPLE: OnceLock<Mutex<Option<(Instant, HostResources)>>> = OnceLock::new();

/// What the host can actually offer right now.
///
/// Refine previously had no resource model at all: concurrency was a fixed
/// constant regardless of hardware, so the same limit applied to a two-core
/// node and a thirty-two-core one. That wastes a capable host and overcommits a
/// constrained one, and it is why scarcity showed up as failure rather than as
/// slower progress.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HostResources {
    pub cores: usize,
    /// `None` where the platform is not one this can read. Automatic admission
    /// treats an unknown value conservatively and permits one agent.
    pub available_memory_bytes: Option<u64>,
    pub free_disk_bytes: Option<u64>,
}

impl HostResources {
    pub fn sample(disk_path: &Path) -> Self {
        Self {
            cores: std::thread::available_parallelism()
                .map(|cores| cores.get())
                .unwrap_or(1),
            available_memory_bytes: available_memory_bytes(),
            free_disk_bytes: free_disk_bytes(disk_path),
        }
    }

    /// A cached sample, refreshed at most once per TTL.
    pub fn current(disk_path: &Path) -> Self {
        let cell = CACHED_SAMPLE.get_or_init(|| Mutex::new(None));
        let Ok(mut cached) = cell.lock() else {
            return Self::sample(disk_path);
        };
        if let Some((taken_at, sample)) = cached.as_ref()
            && taken_at.elapsed() < SAMPLE_TTL
        {
            return *sample;
        }
        let sample = Self::sample(disk_path);
        *cached = Some((Instant::now(), sample));
        sample
    }

    /// How many agents this host should run concurrently.
    ///
    /// Automatic admission spends the configured percentage of detected logical
    /// cores and currently available memory. A complete observed
    /// workload can raise the two-GiB per-agent reservation, but never lower it.
    /// Unknown host memory permits one agent so unsupported or unreadable host
    /// telemetry cannot restore aggressive admission. Never returns zero:
    /// making no progress is worse than making slow progress.
    pub fn recommended_agent_concurrency(
        &self,
        observed_agent_memory_bytes: Option<u64>,
        resource_budget_percent: usize,
    ) -> usize {
        let resource_budget_percent = resource_budget_percent.clamp(1, 100);
        let by_cores = percent_of_usize(self.cores, resource_budget_percent).max(1);
        let Some(available) = self.available_memory_bytes else {
            return 1;
        };
        let per_agent = observed_agent_memory_bytes
            .filter(|bytes| *bytes > 0)
            .unwrap_or(MIN_AGENT_MEMORY_BYTES)
            .max(MIN_AGENT_MEMORY_BYTES);
        let memory_budget = percent_of_u64(available, resource_budget_percent);
        let by_memory = (memory_budget / per_agent) as usize;
        by_cores.min(by_memory).max(1)
    }

    /// Whether `required_bytes` can be spent without exhausting the disk.
    ///
    /// Unknown free space permits the work: refusing on every platform this
    /// cannot measure would be worse than the exhaustion it guards against.
    pub fn has_disk_headroom(&self, required_bytes: u64) -> bool {
        let Some(free) = self.free_disk_bytes else {
            return true;
        };
        free.saturating_sub(required_bytes) >= RESERVED_DISK_BYTES
    }
}

fn percent_of_usize(capacity: usize, percent: usize) -> usize {
    ((capacity as u128).saturating_mul(percent as u128) / 100).min(usize::MAX as u128) as usize
}

fn percent_of_u64(capacity: u64, percent: usize) -> u64 {
    ((capacity as u128).saturating_mul(percent as u128) / 100).min(u64::MAX as u128) as u64
}

/// Free space kept in reserve so that filling the disk degrades into a refusal
/// to start new work rather than a half-written worktree or a truncated record.
const RESERVED_DISK_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// One process in a complete managed-agent process-tree sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ProcessMemorySample {
    pid: u32,
    parent_pid: u32,
    start_time: u64,
    resident_bytes: u64,
}

/// The largest complete resident workload among agents currently running on
/// this node.
///
/// Each workload includes the managed agent root and all of its descendants,
/// because provider CLIs commonly put their expensive work in child processes.
/// Returns `None` until at least one complete stable tree can be observed. A
/// missing PID, unsupported platform, or tree that changes while sampled is not
/// evidence that can lower the conservative reservation.
pub fn observed_agent_memory_bytes(runtime_root: &Path) -> Option<u64> {
    let processes = FileProcessSupervisor::new(runtime_root).list().ok()?;
    let agent_roots = processes
        .iter()
        .filter(|process| process.owner == ProcessOwner::Agent && process.state == "running")
        .map(|process| process.pid)
        .collect::<Option<Vec<_>>>()?;
    let sample = live_process_tree_sample(&agent_roots)?;
    maximum_agent_workload_bytes(&agent_roots, &sample)
}

#[cfg(target_os = "linux")]
fn live_process_tree_sample(agent_roots: &[u32]) -> Option<Vec<ProcessMemorySample>> {
    let mut sample = Vec::new();
    let mut visited = HashSet::new();
    for root in agent_roots {
        sample_process_tree(*root, None, &mut visited, &mut sample)?;
    }
    Some(sample)
}

#[cfg(not(target_os = "linux"))]
fn live_process_tree_sample(_agent_roots: &[u32]) -> Option<Vec<ProcessMemorySample>> {
    None
}

#[cfg(target_os = "linux")]
fn sample_process_tree(
    pid: u32,
    expected_parent: Option<u32>,
    visited: &mut HashSet<u32>,
    sample: &mut Vec<ProcessMemorySample>,
) -> Option<()> {
    if !visited.insert(pid) {
        return Some(());
    }
    let before = process_memory_sample(pid)?;
    if expected_parent.is_some_and(|parent| before.parent_pid != parent) {
        return None;
    }
    let children_before = process_children(pid)?;
    for child in &children_before {
        sample_process_tree(*child, Some(pid), visited, sample)?;
    }
    let after = process_memory_sample(pid)?;
    let children_after = process_children(pid)?;
    if before.pid != after.pid
        || before.parent_pid != after.parent_pid
        || before.start_time != after.start_time
        || children_before != children_after
    {
        return None;
    }
    sample.push(before);
    Some(())
}

#[cfg(target_os = "linux")]
fn process_children(pid: u32) -> Option<Vec<u32>> {
    // Children are owned by the thread that created them. Reading only the
    // main thread's `children` file misses subprocesses launched by provider
    // runtime threads, so require a stable snapshot across every task.
    let tasks_before = process_task_ids(pid)?;
    let mut children = Vec::new();
    for task in &tasks_before {
        let raw = std::fs::read_to_string(format!("/proc/{pid}/task/{task}/children")).ok()?;
        children.extend(
            raw.split_whitespace()
                .map(str::parse::<u32>)
                .collect::<Result<Vec<_>, _>>()
                .ok()?,
        );
    }
    if tasks_before != process_task_ids(pid)? {
        return None;
    }
    children.sort_unstable();
    children.dedup();
    Some(children)
}

#[cfg(target_os = "linux")]
fn process_task_ids(pid: u32) -> Option<Vec<u32>> {
    let mut tasks = std::fs::read_dir(format!("/proc/{pid}/task"))
        .ok()?
        .map(|entry| entry.ok()?.file_name().to_str()?.parse::<u32>().ok())
        .collect::<Option<Vec<_>>>()?;
    tasks.sort_unstable();
    Some(tasks)
}

#[cfg(target_os = "linux")]
fn process_memory_sample(pid: u32) -> Option<ProcessMemorySample> {
    let raw = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (identity, remaining) = raw.rsplit_once(") ")?;
    let sampled_pid = identity.split_once(" (")?.0.parse::<u32>().ok()?;
    let fields = remaining.split_whitespace().collect::<Vec<_>>();
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if page_size <= 0 {
        return None;
    }
    Some(ProcessMemorySample {
        pid: sampled_pid,
        parent_pid: fields.get(1)?.parse::<u32>().ok()?,
        start_time: fields.get(19)?.parse::<u64>().ok()?,
        resident_bytes: fields
            .get(21)?
            .parse::<u64>()
            .ok()?
            .saturating_mul(page_size as u64),
    })
}

fn maximum_agent_workload_bytes(
    agent_roots: &[u32],
    sample: &[ProcessMemorySample],
) -> Option<u64> {
    if agent_roots.is_empty() {
        return None;
    }
    let mut by_pid = HashMap::new();
    let mut children_by_parent: HashMap<u32, Vec<u32>> = HashMap::new();
    for process in sample {
        if by_pid.insert(process.pid, process).is_some() {
            return None;
        }
        children_by_parent
            .entry(process.parent_pid)
            .or_default()
            .push(process.pid);
    }
    agent_roots
        .iter()
        .map(|root| workload_bytes(*root, &by_pid, &children_by_parent, &mut HashSet::new()))
        .collect::<Option<Vec<_>>>()?
        .into_iter()
        .max()
}

fn workload_bytes(
    pid: u32,
    by_pid: &HashMap<u32, &ProcessMemorySample>,
    children_by_parent: &HashMap<u32, Vec<u32>>,
    visited: &mut HashSet<u32>,
) -> Option<u64> {
    if !visited.insert(pid) {
        return None;
    }
    let process = by_pid.get(&pid)?;
    children_by_parent.get(&pid).into_iter().flatten().try_fold(
        process.resident_bytes,
        |total, child| {
            Some(total.saturating_add(workload_bytes(*child, by_pid, children_by_parent, visited)?))
        },
    )
}

#[cfg(target_os = "linux")]
fn available_memory_bytes() -> Option<u64> {
    // MemAvailable is the kernel's own estimate of what a new workload can use,
    // which already accounts for reclaimable cache. MemFree would badly
    // understate it and throttle a healthy host.
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    meminfo.lines().find_map(|line| {
        let rest = line.strip_prefix("MemAvailable:")?;
        let kilobytes = rest.split_whitespace().next()?.parse::<u64>().ok()?;
        Some(kilobytes * 1024)
    })
}

#[cfg(not(target_os = "linux"))]
fn available_memory_bytes() -> Option<u64> {
    None
}

#[cfg(unix)]
fn free_disk_bytes(path: &Path) -> Option<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    // Walk up to the nearest existing ancestor: the directory being sized is
    // often the one about to be created.
    let mut probe = path;
    loop {
        if probe.exists() {
            break;
        }
        probe = probe.parent()?;
    }
    let raw = CString::new(probe.as_os_str().as_bytes()).ok()?;
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(raw.as_ptr(), &mut stats) } != 0 {
        return None;
    }
    // bavail rather than bfree: blocks reserved for root are not spendable.
    Some((stats.f_bavail as u64).saturating_mul(stats.f_frsize as u64))
}

#[cfg(not(unix))]
fn free_disk_bytes(_path: &Path) -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_concurrency_uses_default_percent_of_host_capacity() {
        let host = HostResources {
            cores: 32,
            available_memory_bytes: Some(128 * 1024 * 1024 * 1024),
            free_disk_bytes: Some(500 * 1024 * 1024 * 1024),
        };
        assert_eq!(host.recommended_agent_concurrency(None, 70), 22);
        assert_eq!(host.recommended_agent_concurrency(None, 50), 16);
    }

    #[test]
    fn automatic_concurrency_uses_default_percent_of_available_memory() {
        let host = HostResources {
            cores: 32,
            available_memory_bytes: Some(24 * 1024 * 1024 * 1024),
            free_disk_bytes: Some(250 * 1024 * 1024 * 1024),
        };
        assert_eq!(host.recommended_agent_concurrency(None, 70), 8);
        assert_eq!(host.recommended_agent_concurrency(None, 50), 6);
    }

    #[test]
    fn low_observed_usage_cannot_undercut_two_gibibyte_reservation() {
        let host = HostResources {
            cores: 64,
            available_memory_bytes: Some(64 * 1024 * 1024 * 1024),
            free_disk_bytes: None,
        };
        assert_eq!(
            host.recommended_agent_concurrency(Some(128 * 1024 * 1024), 70),
            22
        );
    }

    #[test]
    fn scarcity_slows_work_rather_than_stopping_it() {
        let host = HostResources {
            cores: 1,
            available_memory_bytes: Some(0),
            free_disk_bytes: Some(0),
        };
        assert_eq!(host.recommended_agent_concurrency(None, 70), 1);
    }

    #[test]
    fn unavailable_host_memory_falls_back_to_one_automatic_slot() {
        let host = HostResources {
            cores: 64,
            available_memory_bytes: None,
            free_disk_bytes: None,
        };
        assert_eq!(host.recommended_agent_concurrency(None, 70), 1);
        assert!(host.has_disk_headroom(u64::MAX));
    }

    #[test]
    fn descendant_memory_is_part_of_each_managed_agent_workload() {
        let gib = 1024 * 1024 * 1024;
        let sample = [
            ProcessMemorySample {
                pid: 10,
                parent_pid: 1,
                start_time: 1,
                resident_bytes: gib / 8,
            },
            ProcessMemorySample {
                pid: 11,
                parent_pid: 10,
                start_time: 2,
                resident_bytes: gib,
            },
            ProcessMemorySample {
                pid: 12,
                parent_pid: 11,
                start_time: 3,
                resident_bytes: 2 * gib,
            },
            ProcessMemorySample {
                pid: 20,
                parent_pid: 1,
                start_time: 4,
                resident_bytes: gib,
            },
        ];
        let observed = maximum_agent_workload_bytes(&[10, 20], &sample);
        assert_eq!(observed, Some(3 * gib + gib / 8));

        let host = HostResources {
            cores: 64,
            available_memory_bytes: Some(32 * gib),
            free_disk_bytes: None,
        };
        assert_eq!(host.recommended_agent_concurrency(observed, 70), 7);
    }

    #[test]
    fn incomplete_process_tree_sample_is_not_scaling_evidence() {
        let sample = [ProcessMemorySample {
            pid: 10,
            parent_pid: 1,
            start_time: 1,
            resident_bytes: 128 * 1024 * 1024,
        }];
        assert_eq!(maximum_agent_workload_bytes(&[10, 20], &sample), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_sample_includes_descendant_resident_memory() {
        use std::io::{BufRead, Write};
        use std::process::Stdio;

        let mut root = std::process::Command::new("sh")
            .args([
                "-c",
                "sleep 30 & child=$!; echo ready; read done; kill \"$child\"; wait \"$child\" 2>/dev/null || true",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut ready = String::new();
        std::io::BufReader::new(root.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready.trim(), "ready");
        let root_pid = root.id();
        let complete = (0..100).find_map(|_| {
            let observation = live_process_tree_sample(&[root_pid]).and_then(|sample| {
                let workload = maximum_agent_workload_bytes(&[root_pid], &sample)?;
                let root_only = sample
                    .iter()
                    .find(|process| process.pid == root_pid)?
                    .resident_bytes;
                (sample.len() >= 2 && workload > root_only).then_some((
                    sample.len(),
                    workload,
                    root_only,
                ))
            });
            if observation.is_none() {
                std::thread::sleep(Duration::from_millis(10));
            }
            observation
        });
        root.stdin.take().unwrap().write_all(b"done\n").unwrap();
        let _ = root.wait();

        let (process_count, workload, root_only) = complete.expect("stable root and child sample");
        assert!(process_count >= 2);
        assert!(workload > root_only);
    }

    #[test]
    fn disk_headroom_refuses_work_that_would_exhaust_the_volume() {
        let host = HostResources {
            cores: 4,
            available_memory_bytes: None,
            free_disk_bytes: Some(10 * 1024 * 1024 * 1024),
        };
        assert!(host.has_disk_headroom(4 * 1024 * 1024 * 1024));
        // Refusing to start is backpressure; a half-created worktree is
        // corruption.
        assert!(!host.has_disk_headroom(9 * 1024 * 1024 * 1024));
    }

    #[test]
    fn sampling_the_real_host_reports_usable_values() {
        let host = HostResources::sample(&std::env::temp_dir());
        assert!(host.cores >= 1);
        assert!(host.recommended_agent_concurrency(None, 70) >= 1);
    }

    #[test]
    fn percentage_arithmetic_is_overflow_safe() {
        assert_eq!(percent_of_usize(usize::MAX, 100), usize::MAX);
        assert_eq!(percent_of_u64(u64::MAX, 100), u64::MAX);
    }
}

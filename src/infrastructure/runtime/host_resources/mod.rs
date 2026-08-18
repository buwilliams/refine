use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::infrastructure::process::subprocess::{FileProcessSupervisor, ProcessOwner};

/// A default per-agent memory reservation, used only until real usage has been
/// observed. Deliberately modest: reserving pessimistically on a large host
/// wastes capacity the operator paid for, and the observed figure replaces this
/// as soon as one agent has run.
const ASSUMED_AGENT_MEMORY_BYTES: u64 = 768 * 1024 * 1024;
/// Memory left to the operating system, the daemon, and the target application's
/// own build and test processes, which are not agents but do run concurrently.
const RESERVED_HOST_MEMORY_BYTES: u64 = 2 * 1024 * 1024 * 1024;
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
    /// `None` where the platform is not one this can read. Callers must treat
    /// an unknown value as "do not constrain on this axis" rather than as zero,
    /// so an unsupported platform keeps working instead of refusing to run.
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
    /// Scales up as readily as down. Leaving one core for the daemon and the
    /// host keeps a busy fleet from starving the process supervising it, and
    /// the memory bound uses observed per-agent usage where it exists so the
    /// limit reflects measured cost rather than a guess. Never returns zero:
    /// making no progress is worse than making slow progress.
    pub fn recommended_agent_concurrency(&self, observed_agent_memory_bytes: Option<u64>) -> usize {
        let by_cores = self.cores.saturating_sub(1).max(1);
        let Some(available) = self.available_memory_bytes else {
            return by_cores;
        };
        let per_agent = observed_agent_memory_bytes
            .filter(|bytes| *bytes > 0)
            .unwrap_or(ASSUMED_AGENT_MEMORY_BYTES);
        let spendable = available.saturating_sub(RESERVED_HOST_MEMORY_BYTES);
        let by_memory = (spendable / per_agent) as usize;
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

/// Free space kept in reserve so that filling the disk degrades into a refusal
/// to start new work rather than a half-written worktree or a truncated record.
const RESERVED_DISK_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// The largest resident footprint among agents currently running on this node.
///
/// This is what lets the governor stop guessing. A reservation chosen up front
/// is either pessimistic — wasting capacity the operator paid for — or
/// optimistic, and overcommits. Measuring what agents actually cost on this
/// host, with this provider, on this target application replaces both. Returns
/// `None` until at least one agent has been observed, which is when the assumed
/// reservation still applies.
pub fn observed_agent_memory_bytes(runtime_root: &Path) -> Option<u64> {
    let processes = FileProcessSupervisor::new(runtime_root).list().ok()?;
    processes
        .iter()
        .filter(|process| process.owner == ProcessOwner::Agent && process.state == "running")
        .filter_map(|process| process.pid)
        .filter_map(resident_bytes)
        .max()
}

#[cfg(target_os = "linux")]
fn resident_bytes(pid: u32) -> Option<u64> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status.lines().find_map(|line| {
        let rest = line.strip_prefix("VmRSS:")?;
        let kilobytes = rest.split_whitespace().next()?.parse::<u64>().ok()?;
        Some(kilobytes * 1024)
    })
}

#[cfg(not(target_os = "linux"))]
fn resident_bytes(_pid: u32) -> Option<u64> {
    None
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
    fn concurrency_scales_up_with_cores_when_memory_is_plentiful() {
        let host = HostResources {
            cores: 32,
            available_memory_bytes: Some(128 * 1024 * 1024 * 1024),
            free_disk_bytes: Some(500 * 1024 * 1024 * 1024),
        };
        // A capable host must not be pinned to the old fixed constant of two.
        assert_eq!(host.recommended_agent_concurrency(None), 31);
    }

    #[test]
    fn concurrency_is_bounded_by_memory_on_a_constrained_host() {
        // The reference deployment: two cores, twenty gigabytes.
        let host = HostResources {
            cores: 2,
            available_memory_bytes: Some(20 * 1024 * 1024 * 1024),
            free_disk_bytes: Some(250 * 1024 * 1024 * 1024),
        };
        assert_eq!(host.recommended_agent_concurrency(None), 1);
    }

    #[test]
    fn observed_usage_replaces_the_assumed_reservation() {
        let host = HostResources {
            cores: 8,
            available_memory_bytes: Some(10 * 1024 * 1024 * 1024),
            free_disk_bytes: None,
        };
        // Assuming a large footprint would leave one slot; measuring a small
        // one earns the host the concurrency it can actually support.
        assert_eq!(host.recommended_agent_concurrency(None), 7);
        assert_eq!(
            host.recommended_agent_concurrency(Some(4 * 1024 * 1024 * 1024)),
            2
        );
    }

    #[test]
    fn scarcity_slows_work_rather_than_stopping_it() {
        let host = HostResources {
            cores: 1,
            available_memory_bytes: Some(0),
            free_disk_bytes: Some(0),
        };
        assert_eq!(host.recommended_agent_concurrency(None), 1);
    }

    #[test]
    fn unknown_capacity_does_not_constrain() {
        let host = HostResources {
            cores: 4,
            available_memory_bytes: None,
            free_disk_bytes: None,
        };
        assert_eq!(host.recommended_agent_concurrency(None), 3);
        assert!(host.has_disk_headroom(u64::MAX));
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
        assert!(host.recommended_agent_concurrency(None) >= 1);
    }
}

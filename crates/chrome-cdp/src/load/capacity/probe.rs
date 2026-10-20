//! What the machine running a load run's users is using (plan 23.5), read
//! once a second: its CPU use, load average and free memory, its Chromes'
//! CPU and resident memory (each Chrome's process tree), and chrome-cdp's
//! own.
//!
//! On Linux it all comes from `/proc`. On macOS, which has no `/proc` and
//! whose system calls for it would need `unsafe`, one `iostat -w 1` runs
//! for the whole run (CPU use and load average every second), and each
//! second `sysctl kern.memorystatus_level` (the free memory the system
//! counts, in %) and one `ps` (every process's parent, memory and CPU
//! time) run, about 13 ms each. Elsewhere only the run's own timers are
//! watched.
//!
//! The reads run in a task of their own ([`Sampler`]), as a machine whose
//! CPU is full takes its time to start a process: the run's second takes
//! the latest read, and never waits for one.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// What the machine used over the last second; `None` for what cannot be
/// read here.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct System {
    /// CPU use, in percent of all the machine's CPUs.
    pub(crate) cpu: Option<f64>,
    /// The 1-minute load average.
    pub(crate) load: Option<f64>,
    /// Free memory, in percent of the machine's.
    pub(crate) memory_free: Option<f64>,
    /// Every Chrome's processes together: CPU in cores, resident memory in
    /// bytes.
    pub(crate) chrome_cpu: Option<f64>,
    pub(crate) chrome_rss: Option<u64>,
    /// chrome-cdp's own.
    pub(crate) own_cpu: Option<f64>,
    pub(crate) own_rss: Option<u64>,
}

/// A [`Probe`] reading the machine every second in a task of its own,
/// until dropped.
#[derive(Debug)]
pub(crate) struct Sampler {
    /// The Chromes' pids, as the run last said them.
    pids: Arc<Mutex<Vec<u32>>>,
    latest: Arc<Mutex<System>>,
    task: tokio::task::JoinHandle<()>,
}

impl Sampler {
    /// Starts reading, now and then every second, with `chromes` the pids
    /// of the run's Chromes.
    pub(crate) fn start(chromes: Vec<u32>) -> Self {
        let pids = Arc::new(Mutex::new(chromes));
        let latest = Arc::new(Mutex::new(System::default()));
        let task = tokio::spawn({
            let pids = pids.clone();
            let latest = latest.clone();
            async move {
                let mut probe = Probe::new();
                let mut every = tokio::time::interval(Duration::from_secs(1));
                every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    every.tick().await;
                    let chromes = pids.lock().unwrap_or_else(PoisonError::into_inner).clone();
                    let read = probe.read(&chromes).await;
                    *latest.lock().unwrap_or_else(PoisonError::into_inner) = read;
                }
            }
        });
        Self { pids, latest, task }
    }

    /// The latest read, with `chromes` the pids of the run's Chromes now
    /// (for the next).
    pub(crate) fn latest(&self, chromes: Vec<u32>) -> System {
        *self.pids.lock().unwrap_or_else(PoisonError::into_inner) = chromes;
        *self.latest.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// A process, as the probe reads it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Process {
    pub(crate) pid: u32,
    pub(crate) ppid: u32,
    /// Resident memory, in bytes.
    pub(crate) rss: u64,
    /// CPU time so far, in seconds.
    pub(crate) cpu: f64,
}

/// Reads the machine once a second.
#[derive(Debug)]
pub(crate) struct Probe {
    /// Each process's CPU time at the last read, and when that was.
    seen: HashMap<u32, f64>,
    last: Option<Instant>,
    #[cfg(target_os = "linux")]
    linux: linux::State,
    #[cfg(target_os = "macos")]
    mac: mac::State,
}

impl Probe {
    /// A probe; on macOS this starts its `iostat`, so call it inside the
    /// run's runtime.
    pub(crate) fn new() -> Self {
        Self {
            seen: HashMap::new(),
            last: None,
            #[cfg(target_os = "linux")]
            linux: linux::State::default(),
            #[cfg(target_os = "macos")]
            mac: mac::State::start(),
        }
    }

    /// Whether the machine's CPU and memory can be read here.
    pub(crate) fn machine() -> bool {
        cfg!(any(target_os = "linux", target_os = "macos"))
    }

    /// What the machine used since the last read, with `chromes` the pids
    /// of the run's Chromes.
    pub(crate) async fn read(&mut self, chromes: &[u32]) -> System {
        let began = Instant::now();
        #[cfg(target_os = "linux")]
        let (mut system, processes) = {
            let mut state = std::mem::take(&mut self.linux);
            let (state, read) = tokio::task::spawn_blocking(move || {
                let read = state.read();
                (state, read)
            })
            .await
            .unwrap_or_else(|_| (linux::State::default(), (System::default(), Vec::new())));
            self.linux = state;
            read
        };
        #[cfg(target_os = "macos")]
        let (mut system, processes) = self.mac.read().await;
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let (mut system, processes) = (System::default(), Vec::<Process>::new());
        // When the processes were read: a machine whose CPU is full takes
        // its time to start `ps`, so the middle of the read.
        let at = began + began.elapsed() / 2;
        let elapsed = self.last.map(|l| at.duration_since(l).as_secs_f64());
        self.last = Some(at);
        if !processes.is_empty() {
            let usage = usage(&processes, chromes, std::process::id(), &mut self.seen);
            #[allow(clippy::cast_precision_loss)]
            let cpus =
                std::thread::available_parallelism().map_or(1, std::num::NonZero::get) as f64;
            if let Some(elapsed) = elapsed.filter(|e| *e > 0.0) {
                system.chrome_cpu = Some(round((usage.chrome_cpu / elapsed).min(cpus), 100.0));
                system.own_cpu = Some(round((usage.own_cpu / elapsed).min(cpus), 100.0));
            }
            system.chrome_rss = Some(usage.chrome_rss);
            system.own_rss = usage.own_rss;
        }
        system
    }
}

/// What a read of the processes found.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Usage {
    /// CPU seconds since the last read.
    pub(crate) chrome_cpu: f64,
    pub(crate) own_cpu: f64,
    pub(crate) chrome_rss: u64,
    pub(crate) own_rss: Option<u64>,
}

/// The Chromes' and chrome-cdp's (`own`) use among `processes`: each
/// Chrome's process and the processes under it. `seen` keeps each
/// process's CPU time from the last read; a new process counts all of its
/// time.
pub(crate) fn usage(
    processes: &[Process],
    chromes: &[u32],
    own: u32,
    seen: &mut HashMap<u32, f64>,
) -> Usage {
    let mut children: HashMap<u32, Vec<usize>> = HashMap::new();
    for (i, p) in processes.iter().enumerate() {
        children.entry(p.ppid).or_default().push(i);
    }
    let mut tree: Vec<usize> = processes
        .iter()
        .enumerate()
        .filter(|(_, p)| chromes.contains(&p.pid))
        .map(|(i, _)| i)
        .collect();
    let mut found: std::collections::HashSet<usize> = tree.iter().copied().collect();
    let mut next = 0;
    while next < tree.len() {
        let pid = processes[tree[next]].pid;
        for &kid in children.get(&pid).into_iter().flatten() {
            if found.insert(kid) {
                tree.push(kid);
            }
        }
        next += 1;
    }
    let mut usage = Usage::default();
    let mut grew = |p: &Process| {
        let before = seen.insert(p.pid, p.cpu).unwrap_or(0.0);
        (p.cpu - before).max(0.0)
    };
    for &i in &tree {
        let p = &processes[i];
        usage.chrome_cpu += grew(p);
        usage.chrome_rss += p.rss;
    }
    if let Some(p) = processes.iter().find(|p| p.pid == own) {
        usage.own_cpu = grew(p);
        usage.own_rss = Some(p.rss);
    }
    // Forget the processes that are gone.
    seen.retain(|pid, _| processes.iter().any(|p| p.pid == *pid));
    usage
}

/// `value` rounded to `1 / to`.
fn round(value: f64, to: f64) -> f64 {
    (value * to).round() / to
}

/// `ps -o pid=,ppid=,rss=,time=`'s lines (RSS in KiB; time as
/// `[[dd-]hh:]mm:ss[.cc]`).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn parse_ps(text: &str) -> Vec<Process> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let pid = parts.next()?.parse().ok()?;
            let ppid = parts.next()?.parse().ok()?;
            let rss: u64 = parts.next()?.parse().ok()?;
            let cpu = cpu_time(parts.next()?)?;
            Some(Process {
                pid,
                ppid,
                rss: rss * 1024,
                cpu,
            })
        })
        .collect()
}

/// `1:02.50` → 62.5 s; `01:02:03` → 3723 s; `1-00:00:00` → a day.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn cpu_time(text: &str) -> Option<f64> {
    let (days, rest) = match text.split_once('-') {
        Some((d, rest)) => (d.parse::<f64>().ok()?, rest),
        None => (0.0, text),
    };
    let mut seconds = 0.0;
    for part in rest.split(':') {
        seconds = seconds * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(days * 86_400.0 + seconds)
}

/// An `iostat -n 0 -w 1` line of numbers: `us sy id 1m 5m 15m` → the CPU
/// use in % and the 1-minute load average.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn parse_iostat(line: &str) -> Option<(f64, f64)> {
    let numbers: Vec<f64> = line
        .split_whitespace()
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    match numbers.as_slice() {
        [_, _, idle, load, ..] => Some(((100.0 - idle).clamp(0.0, 100.0), *load)),
        _ => None,
    }
}

/// `/proc/stat`'s first line: the CPUs' time busy and in all, in ticks.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn parse_proc_stat(text: &str) -> Option<(u64, u64)> {
    let line = text.lines().find(|l| l.starts_with("cpu "))?;
    let ticks: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|n| n.parse().ok())
        .collect();
    // user nice system idle iowait irq softirq steal (guest is in user).
    let total: u64 = ticks.iter().take(8).sum();
    let idle = ticks.get(3).copied().unwrap_or(0) + ticks.get(4).copied().unwrap_or(0);
    Some((total.saturating_sub(idle), total))
}

/// `/proc/meminfo`'s free memory (`MemAvailable`), in % of `MemTotal`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn parse_meminfo(text: &str) -> Option<f64> {
    let kib = |key: &str| -> Option<f64> {
        let line = text.lines().find(|l| l.starts_with(key))?;
        line.split_whitespace().nth(1)?.parse().ok()
    };
    let total = kib("MemTotal:")?;
    let available = kib("MemAvailable:")?;
    (total > 0.0).then(|| round(available / total * 100.0, 10.0))
}

/// A `/proc/<pid>/stat` line: its parent, CPU time in ticks and resident
/// memory in pages (after the command, which may hold spaces and
/// parentheses).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn parse_pid_stat(text: &str) -> Option<(u32, u64, u64)> {
    let (_, rest) = text.rsplit_once(')')?;
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // From the state (field 3): ppid 4, utime 14, stime 15, rss 24.
    let ppid = fields.get(1)?.parse().ok()?;
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    let rss = fields.get(21)?.parse().ok()?;
    Some((ppid, utime + stime, rss))
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{Process, System, parse_meminfo, parse_pid_stat, parse_proc_stat, round};

    /// Linux's clock ticks a second (`USER_HZ`, fixed in its ABI).
    const TICKS: f64 = 100.0;

    #[derive(Debug, Default)]
    pub(super) struct State {
        /// The CPUs' busy and total ticks at the last read.
        cpu: Option<(u64, u64)>,
        /// The size of a page, from chrome-cdp's own `VmRSS` and pages.
        page: Option<u64>,
    }

    impl State {
        pub(super) fn read(&mut self) -> (System, Vec<Process>) {
            let mut system = System::default();
            if let Some((busy, total)) = std::fs::read_to_string("/proc/stat")
                .ok()
                .as_deref()
                .and_then(parse_proc_stat)
            {
                if let Some((was_busy, was_total)) = self.cpu
                    && total > was_total
                {
                    #[allow(clippy::cast_precision_loss)]
                    let share = busy.saturating_sub(was_busy) as f64 / (total - was_total) as f64;
                    system.cpu = Some(round(share * 100.0, 10.0));
                }
                self.cpu = Some((busy, total));
            }
            system.load = std::fs::read_to_string("/proc/loadavg")
                .ok()
                .and_then(|t| t.split_whitespace().next()?.parse().ok());
            system.memory_free = std::fs::read_to_string("/proc/meminfo")
                .ok()
                .as_deref()
                .and_then(parse_meminfo);
            let page = *self.page.get_or_insert_with(page_size);
            let mut processes = Vec::new();
            if let Ok(dir) = std::fs::read_dir("/proc") {
                for entry in dir.flatten() {
                    let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse().ok()) else {
                        continue;
                    };
                    let Some((ppid, ticks, pages)) =
                        std::fs::read_to_string(entry.path().join("stat"))
                            .ok()
                            .as_deref()
                            .and_then(parse_pid_stat)
                    else {
                        continue;
                    };
                    #[allow(clippy::cast_precision_loss)]
                    processes.push(Process {
                        pid,
                        ppid,
                        rss: pages * page,
                        cpu: ticks as f64 / TICKS,
                    });
                }
            }
            (system, processes)
        }
    }

    /// The size of a page: chrome-cdp's `VmRSS` over its resident pages,
    /// to a power of two; 4 KiB when that cannot be read.
    fn page_size() -> u64 {
        let rss_kib = std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|t| {
                let line = t.lines().find(|l| l.starts_with("VmRSS:"))?.to_owned();
                line.split_whitespace().nth(1)?.parse::<u64>().ok()
            });
        let pages = std::fs::read_to_string("/proc/self/stat")
            .ok()
            .as_deref()
            .and_then(parse_pid_stat)
            .map(|(_, _, pages)| pages);
        match (rss_kib, pages) {
            (Some(kib), Some(pages)) if pages > 0 => (kib * 1024 / pages).next_power_of_two(),
            _ => 4096,
        }
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use std::sync::{Arc, Mutex, PoisonError};

    use tokio::io::AsyncBufReadExt as _;
    use tokio::process::Command;

    use super::{Process, System, parse_iostat, parse_ps};

    #[derive(Debug)]
    pub(super) struct State {
        /// The last CPU use and load average `iostat` printed.
        latest: Arc<Mutex<Option<(f64, f64)>>>,
        /// `iostat`, killed when the probe is dropped.
        _iostat: Option<tokio::process::Child>,
    }

    impl State {
        pub(super) fn start() -> Self {
            let latest = Arc::new(Mutex::new(None));
            let child = Command::new("iostat")
                .args(["-n", "0", "-w", "1"])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .stdin(std::process::Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .ok();
            let child = child.map(|mut child| {
                if let Some(out) = child.stdout.take() {
                    let latest = latest.clone();
                    tokio::spawn(async move {
                        let mut lines = tokio::io::BufReader::new(out).lines();
                        // Its first numbers are since the machine started.
                        let mut first = true;
                        while let Ok(Some(line)) = lines.next_line().await {
                            if let Some(read) = parse_iostat(&line) {
                                if !first {
                                    *latest.lock().unwrap_or_else(PoisonError::into_inner) =
                                        Some(read);
                                }
                                first = false;
                            }
                        }
                    });
                }
                child
            });
            Self {
                latest,
                _iostat: child,
            }
        }

        pub(super) async fn read(&self) -> (System, Vec<Process>) {
            let (ps, memory) = tokio::join!(
                Command::new("ps")
                    .args(["-axo", "pid=,ppid=,rss=,time="])
                    .kill_on_drop(true)
                    .output(),
                Command::new("sysctl")
                    .args(["-n", "kern.memorystatus_level"])
                    .kill_on_drop(true)
                    .output(),
            );
            let latest = *self.latest.lock().unwrap_or_else(PoisonError::into_inner);
            let system = System {
                cpu: latest.map(|(cpu, _)| cpu),
                load: latest.map(|(_, load)| load),
                memory_free: memory
                    .ok()
                    .and_then(|out| String::from_utf8_lossy(&out.stdout).trim().parse().ok()),
                ..System::default()
            };
            let processes = ps
                .map(|out| parse_ps(&String::from_utf8_lossy(&out.stdout)))
                .unwrap_or_default();
            (system, processes)
        }
    }
}

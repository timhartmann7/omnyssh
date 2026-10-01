//! Background metrics polling pool.
//!
//! Each host gets its own persistent tokio task that manages its SSH
//! connection and collects metrics at a configurable interval.
//!
//! Architecture:
//! - [`PollManager`] — created by the main app, owns abort handles.
//! - One `HostPoller` task per host — loops indefinitely until aborted.
//! - Implements exponential backoff on connection failures.
//! - One SSH connection per host, reused across polls.
//! - All data sent to the main event loop via `mpsc::Sender<CoreEvent>`.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time;

use crate::event::{CoreEvent, Metrics, ProcessInfo};
use crate::ssh::client::{ConnectionStatus, Host, MonitorMode};
use crate::ssh::identity;
use crate::ssh::metrics::{
    parse_cpu_cp_time, parse_cpu_proc_stat, parse_cpu_top, parse_cpu_top_macos, parse_disk_df,
    parse_loadavg, parse_loadavg_uptime, parse_ram_free, parse_ram_sysctl, parse_ram_vmstat,
    parse_top_processes, parse_uptime,
};
use crate::ssh::password;
use crate::ssh::session::{dial_error, passphrase_required, waiting_login, SshSession};

// ---------------------------------------------------------------------------
// Backoff schedule
// ---------------------------------------------------------------------------

const BACKOFF_SECS: [u64; 4] = [30, 60, 120, 300];

// ---------------------------------------------------------------------------
// Metric commands
// ---------------------------------------------------------------------------

// Metric output is machine-parsed, so the locale has to be pinned: a server set
// to a comma-decimal language prints "99,1 id" and "Speicher:", which the parsers
// read as garbage or not at all. `env` rather than a `VAR=value cmd` prefix, which
// is not valid csh/tcsh syntax.
const CPU_CMD: &str = "env LC_ALL=C top -bn1 2>/dev/null | head -5";
// FreeBSD has neither `free` nor `vm_stat`; its page counters come last so no
// other host pays for them.
const MEM_CMD: &str = "env LC_ALL=C free -b 2>/dev/null || env LC_ALL=C vm_stat 2>/dev/null || \
     env LC_ALL=C sysctl hw.pagesize vm.stats.vm.v_page_count vm.stats.vm.v_free_count \
     vm.stats.vm.v_inactive_count vfs.bufspace kstat.zfs.misc.arcstats.size \
     kstat.zfs.misc.arcstats.c_min 2>/dev/null";
const DISK_CMD: &str = "env LC_ALL=C df -k / 2>/dev/null";
const UPTIME_CMD: &str = "env LC_ALL=C uptime 2>/dev/null";
const CPU_MACOS_CMD: &str = "env LC_ALL=C top -l 1 -n 0 2>/dev/null | grep 'CPU usage'";
// FreeBSD's top takes neither `-bn1` nor `-l`, and its first screen averages
// since boot: sample the tick counters a second apart instead. Only there:
// OpenBSD has the counters too, in another form, and would wait for nothing.
const CPU_FREEBSD_CMD: &str =
    "env LC_ALL=C sysctl -n kern.ostype 2>/dev/null | grep -qx FreeBSD && \
     env LC_ALL=C sysctl -n kern.cp_time 2>/dev/null && sleep 1 && \
     env LC_ALL=C sysctl -n kern.cp_time 2>/dev/null";
// `ps` output reaches the user, so keep the host's LC_CTYPE: under a full
// `LC_ALL=C` GNU ps replaces every non-ASCII byte of a process name with '?'.
const PS_LOCALE: &str = "env LC_ALL= LC_NUMERIC=C LC_MESSAGES=C";

struct BackoffState {
    step: usize,
}

impl BackoffState {
    fn new() -> Self {
        Self { step: 0 }
    }

    fn next_delay(&mut self) -> Duration {
        let secs = BACKOFF_SECS[self.step];
        self.step = (self.step + 1).min(BACKOFF_SECS.len() - 1);
        Duration::from_secs(secs)
    }

    fn reset(&mut self) {
        self.step = 0;
    }
}

// ---------------------------------------------------------------------------
// PollManager — owned by App, drives all HostPoller tasks
// ---------------------------------------------------------------------------

/// Manages background metric polling for all hosts.
///
/// Drop this struct to abort all poller tasks.
pub struct PollManager {
    task_handles: Vec<JoinHandle<()>>,
    /// Per-host channel to send an immediate-refresh signal.
    refresh_txs: HashMap<String, mpsc::Sender<()>>,
}

impl PollManager {
    /// Spawn one poller task per host.
    pub fn start(hosts: Vec<Host>, tx: mpsc::Sender<CoreEvent>, poll_interval: Duration) -> Self {
        let mut task_handles = Vec::with_capacity(hosts.len());
        let mut refresh_txs = HashMap::with_capacity(hosts.len());

        for host in hosts {
            let (refresh_tx, refresh_rx) = mpsc::channel::<()>(4);
            refresh_txs.insert(host.name.clone(), refresh_tx);

            let event_tx = tx.clone();
            let interval = poll_interval;
            let handle = tokio::spawn(run_host_poller(host, event_tx, interval, refresh_rx));
            task_handles.push(handle);
        }

        Self {
            task_handles,
            refresh_txs,
        }
    }

    /// Trigger an immediate poll for all hosts (called on `r` key press).
    pub fn refresh_all(&self) {
        for (name, tx) in &self.refresh_txs {
            if tx.try_send(()).is_err() {
                tracing::debug!(host = %name, "refresh signal dropped — channel full or closed");
            }
        }
    }

    /// Abort all poller tasks. Called on app exit to allow clean shutdown.
    /// SSH sessions are dropped inside the tasks, which triggers
    /// russh's graceful disconnect.
    pub fn shutdown(self) {
        for handle in &self.task_handles {
            handle.abort();
        }
    }
}

// ---------------------------------------------------------------------------
// Per-host poller task
// ---------------------------------------------------------------------------

async fn run_host_poller(
    host: Host,
    tx: mpsc::Sender<CoreEvent>,
    poll_interval: Duration,
    refresh_rx: mpsc::Receiver<()>,
) {
    match host.monitoring {
        MonitorMode::Ssh => run_ssh_poller(host, tx, poll_interval, refresh_rx).await,
        MonitorMode::TcpPort => run_tcp_poller(host, tx, poll_interval, refresh_rx).await,
    }
}

/// How long a reachability probe waits for the port to answer.
const TCP_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Reachability-only poller: one TCP connect per cycle, no SSH session and no
/// authentication, so a device that cannot serve metrics is never logged in to.
/// Emits status only — a host in this mode reports no metrics.
async fn run_tcp_poller(
    host: Host,
    tx: mpsc::Sender<CoreEvent>,
    poll_interval: Duration,
    mut refresh_rx: mpsc::Receiver<()>,
) {
    // A bare TCP dial cannot traverse a bastion, and probing the target address
    // direct would silently report on whatever else answers it.
    if host.proxy_jump.is_some() {
        send_status(
            &tx,
            &host.name,
            ConnectionStatus::Failed(String::from(
                "a port check cannot reach a host behind ProxyJump - use SSH monitoring",
            )),
        )
        .await;
        return;
    }

    let port = host.monitor_port.filter(|&p| p != 0).unwrap_or(host.port);
    let addr = format!("{}:{}", host.hostname, port);
    let mut backoff = BackoffState::new();
    let mut last: Option<ConnectionStatus> = None;

    loop {
        if last.is_none()
            && tx
                .send(CoreEvent::HostStatusChanged(
                    host.name.clone(),
                    ConnectionStatus::Connecting,
                ))
                .await
                .is_err()
        {
            return; // App has shut down.
        }

        let status = match time::timeout(TCP_PROBE_TIMEOUT, TcpStream::connect(&addr)).await {
            Ok(Ok(_)) => ConnectionStatus::Connected,
            Ok(Err(e)) => ConnectionStatus::Failed(dial_error(&e)),
            Err(_) => ConnectionStatus::Failed(format!("no answer from {addr}")),
        };
        let reachable = matches!(status, ConnectionStatus::Connected);

        // Only on a change: re-announcing every cycle flickers the card between
        // reachable and checking and re-buckets the host in the status bar.
        if last.as_ref() != Some(&status) {
            if tx
                .send(CoreEvent::HostStatusChanged(
                    host.name.clone(),
                    status.clone(),
                ))
                .await
                .is_err()
            {
                return;
            }
            last = Some(status);
        }

        if reachable {
            backoff.reset();
            wait_or_refresh(poll_interval, &mut refresh_rx).await;
        } else {
            // Same restraint as the SSH poller: a device that is down should not
            // be re-dialled on every refresh tick.
            let delay = backoff.next_delay().max(poll_interval);
            wait_backoff(delay, &mut refresh_rx).await;
        }
    }
}

async fn run_ssh_poller(
    host: Host,
    tx: mpsc::Sender<CoreEvent>,
    poll_interval: Duration,
    mut refresh_rx: mpsc::Receiver<()>,
) {
    let mut backoff = BackoffState::new();
    let mut session: Option<SshSession> = None;
    let mut discovery_done = false; // Track if we've done Quick Scan

    loop {
        // Ensure we have a live session.
        if session.is_none() {
            send_status(&tx, &host.name, ConnectionStatus::Connecting).await;
            match SshSession::connect(&host).await {
                Ok(s) => {
                    send_status(&tx, &host.name, ConnectionStatus::Connected).await;
                    session = Some(s);
                    discovery_done = false; // Reset discovery flag on new connection
                }
                Err(e) => {
                    // The whole chain: "SSH connection failed" alone does not say
                    // whether the port was closed, the name did not resolve or the
                    // host key changed.
                    let reason = format!("{e:#}");
                    tracing::debug!(host = %host.name, error = %reason, "connection failed");
                    send_status(&tx, &host.name, ConnectionStatus::Failed(reason)).await;
                    let delay = backoff.next_delay();
                    let Some(login) = waiting_login(&e).map(str::to_owned) else {
                        // Wait with backoff, allowing early refresh.
                        wait_backoff(delay, &mut refresh_rx).await;
                        continue;
                    };
                    // A poller never asks for a password, only for a passphrase.
                    // Unlocking the key, or typing the password elsewhere (a
                    // terminal), is what changes the outcome, so either ends the wait.
                    let locked = passphrase_required(&e).map(str::to_owned);
                    if let Some(path) = &locked {
                        identity::ask_passphrase_once(&tx, &host.name, path).await;
                    }
                    tokio::select! {
                        () = wait_backoff(delay, &mut refresh_rx) => {}
                        () = password::remembered(&login) => {}
                        () = async {
                            match &locked {
                                Some(path) => identity::unlocked(path).await,
                                None => std::future::pending().await,
                            }
                        } => {}
                    }
                    continue;
                }
            }
        }

        // Run Quick Scan once per connection (don't block UI)
        // This happens right after connection before the first metrics poll
        if !discovery_done {
            if let Some(sess) = &session {
                // Run discovery asynchronously
                // Clone the session since Handle is Arc-based and cheap to clone
                let sess_clone = sess.clone();
                let host_name = host.name.clone();
                let tx_clone = tx.clone();
                tokio::spawn(async move {
                    match crate::ssh::discovery::quick_scan(
                        &sess_clone,
                        host_name.clone(),
                        tx_clone.clone(),
                    )
                    .await
                    {
                        Ok(()) => {
                            tracing::debug!(host = %host_name, "quick scan completed successfully");
                        }
                        Err(e) => {
                            tracing::warn!(host = %host_name, error = %e, "quick scan failed");
                            let _ = tx_clone
                                .send(CoreEvent::DiscoveryFailed(host_name, e.to_string()))
                                .await;
                        }
                    }
                });
                discovery_done = true;
            }
        }

        // Collect metrics using the live session.
        // SAFETY: we only reach this point if session was set to Some above
        // (either just connected or carried over from the previous iteration).
        // This is a single-task async loop with no concurrent mutation, so
        // the expect is always satisfied.
        let sess = session.as_ref().expect("session is Some here");
        match collect_metrics(sess, &host.name).await {
            Ok(metrics) => {
                // A cycle that produced data proves the host is pollable. Resetting
                // on connect instead pins a host that authenticates but cannot run
                // commands (a network appliance) to the first backoff step forever.
                backoff.reset();
                if tx
                    .send(CoreEvent::MetricsUpdate(host.name.clone(), metrics))
                    .await
                    .is_err()
                {
                    break; // App has shut down.
                }
            }
            Err(e) => {
                tracing::debug!(host = %host.name, error = %e, "metric collection failed");
                // Session is broken — drop it and reconnect next iteration.
                session.take();
                send_status(&tx, &host.name, ConnectionStatus::Failed(e.to_string())).await;
                let delay = backoff.next_delay();
                wait_backoff(delay, &mut refresh_rx).await;
                continue;
            }
        }

        // Wait for the next poll interval or a manual refresh signal.
        wait_or_refresh(poll_interval, &mut refresh_rx).await;
    }
}

/// Wait for `delay`, but return early if a refresh signal is received.
async fn wait_or_refresh(delay: Duration, refresh_rx: &mut mpsc::Receiver<()>) {
    let sleep = tokio::time::sleep(delay);
    tokio::pin!(sleep);
    tokio::select! {
        () = &mut sleep => {}
        signal = refresh_rx.recv() => {
            // `None` means every sender is gone. Returning on it would make the
            // caller's loop spin, so serve out the delay instead.
            if signal.is_none() {
                sleep.await;
            }
        }
    }
}

/// Sleep the whole `delay`, discarding refresh signals.
///
/// A reconnect must never dial faster than the backoff schedule: the GUI drives
/// `refresh_all` on its own timer, which is indistinguishable from a keypress
/// here and would otherwise retry a failing host every few seconds.
async fn wait_backoff(delay: Duration, refresh_rx: &mut mpsc::Receiver<()>) {
    let sleep = tokio::time::sleep(delay);
    tokio::pin!(sleep);
    loop {
        tokio::select! {
            () = &mut sleep => return,
            signal = refresh_rx.recv() => {
                // `None` means every sender is gone and `recv` will return it
                // immediately from now on — stop selecting on it, or the task
                // spins without ever yielding.
                if signal.is_none() {
                    sleep.await;
                    return;
                }
            }
        }
    }
}

async fn send_status(tx: &mpsc::Sender<CoreEvent>, name: &str, status: ConnectionStatus) {
    let _ = tx
        .send(CoreEvent::HostStatusChanged(name.to_string(), status))
        .await;
}

// ---------------------------------------------------------------------------
// Metric collection
// ---------------------------------------------------------------------------

/// Run all metric commands and return a [`Metrics`] snapshot.
///
/// Tries Linux commands first. If the output doesn't match the expected
/// format, falls back to macOS/BSD variants (graceful degradation per
/// the risk matrix in tech.md §10).
///
/// Returns `Err` when all commands fail simultaneously — this indicates a dead
/// session and should prompt the caller to reconnect.
async fn collect_metrics(session: &SshSession, host_name: &str) -> anyhow::Result<Metrics> {
    // Run all commands concurrently for speed.
    let (cpu_out, mem_out, disk_out, uptime_out, loadavg_out) = tokio::join!(
        session.run_command(CPU_CMD),
        session.run_command(MEM_CMD),
        session.run_command(DISK_CMD),
        session.run_command(UPTIME_CMD),
        session.run_command("cat /proc/loadavg 2>/dev/null"),
    );

    // If every command failed the session is almost certainly dead — return an
    // error so the poller drops the session and reconnects.
    if cpu_out.is_err()
        && mem_out.is_err()
        && disk_out.is_err()
        && uptime_out.is_err()
        && loadavg_out.is_err()
    {
        let err = cpu_out
            .err()
            .unwrap_or_else(|| anyhow::anyhow!("all metric commands failed"));
        return Err(anyhow::anyhow!(
            "all metric commands failed (session may be dead): {}",
            err
        ));
    }

    // Log individual command failures at debug level so operators can distinguish
    // "metric unavailable on this OS" from "command errored".
    let cpu_str = cpu_out
        .inspect_err(|e| tracing::debug!(host = %host_name, error = %e, "cpu command failed"))
        .unwrap_or_default();
    let mem_str = mem_out
        .inspect_err(|e| tracing::debug!(host = %host_name, error = %e, "mem command failed"))
        .unwrap_or_default();
    let disk_str = disk_out
        .inspect_err(|e| tracing::debug!(host = %host_name, error = %e, "disk command failed"))
        .unwrap_or_default();
    let uptime_str = uptime_out
        .inspect_err(|e| tracing::debug!(host = %host_name, error = %e, "uptime command failed"))
        .unwrap_or_default();
    let loadavg_str = loadavg_out
        .inspect_err(|e| tracing::debug!(host = %host_name, error = %e, "loadavg command failed"))
        .unwrap_or_default();

    let cpu_percent = parse_cpu_combined(&cpu_str, session).await;

    let ram_percent = parse_ram_combined(&mem_str, session).await;

    let disk_percent = parse_disk_df(&disk_str).or_else(|| {
        if !disk_str.is_empty() {
            tracing::debug!(host = %host_name, "disk output present but parse failed");
        }
        None
    });

    let uptime = parse_uptime(&uptime_str);

    let load_avg = parse_loadavg(&loadavg_str).or_else(|| parse_loadavg_uptime(&uptime_str));

    let top_processes = collect_top_processes(session).await;

    Ok(Metrics {
        cpu_percent,
        ram_percent,
        disk_percent,
        uptime,
        load_avg,
        os_info: None, // OS info is collected during discovery, not metrics polling
        top_processes,
        last_updated: Instant::now(),
    })
}

/// Collects the top 3 processes by CPU usage.
///
/// Tries GNU `ps` (Linux) with a server-side sort first, then falls back to
/// BSD `ps` (macOS, FreeBSD). Returns `None` when neither variant yields usable output.
async fn collect_top_processes(session: &SshSession) -> Option<Vec<ProcessInfo>> {
    // Linux: GNU ps with server-side sort by CPU; empty `=` headers suppressed.
    let linux_out = session
        .run_command(&top_processes_command(
            "-eo pid=,ppid=,pcpu=,pmem=,comm= --sort=-pcpu",
        ))
        .await
        .unwrap_or_default();
    if let Some(procs) = parse_top_processes(&linux_out) {
        return Some(procs);
    }
    // macOS and FreeBSD: BSD ps sorted by CPU usage (-r). One `-o` per column:
    // FreeBSD reads everything after the first `=` as that column's header.
    let bsd_out = session
        .run_command(&top_processes_command(
            "-Ac -o pid= -o ppid= -o pcpu= -o pmem= -o comm= -r",
        ))
        .await
        .unwrap_or_default();
    parse_top_processes(&bsd_out)
}

/// Builds the remote shell command that lists the top processes by CPU with
/// the monitoring connection's own process chain removed. `ps_args` selects
/// the OS-specific `ps` columns and sort order.
///
/// The pipeline runs inside an SSH-spawned shell whose own processes — and the
/// `sshd` hosting the connection — would otherwise dominate the snapshot on an
/// idle server. The `awk` filter drops them strictly by PID:
///
/// - `s` (`$$`) — the shell, and everything it forked (`ps`, `awk`, `head`);
/// - `p` (`$PPID`) — the connection's `sshd`, and its children;
/// - `g` — the privileged `sshd` one level up (parent of `$PPID`).
///
/// Filtering is by PID only, never by process name, so a genuinely busy SSH
/// session belonging to another user still appears. The one exception is
/// FreeBSD's kernel `idle` (parent 0), which `ps` charges with all idle time.
/// POSIX-sh syntax — a non-Bourne login shell simply yields no output and the
/// panel degrades to "process data unavailable".
fn top_processes_command(ps_args: &str) -> String {
    format!(
        "g=$({PS_LOCALE} ps -o ppid= -p $PPID 2>/dev/null | tr -d ' '); \
         {PS_LOCALE} ps {ps_args} 2>/dev/null | \
         awk -v s=$$ -v p=$PPID -v g=\"$g\" \
         '$1!=s && $1!=p && $1!=g && $2!=s && $2!=p && !($2==0 && $5==\"idle\") \
         {{$1=\"\";$2=\"\";sub(/^[ \\t]+/,\"\");print}}' | \
         head -n 3"
    )
}

async fn parse_cpu_combined(top_out: &str, session: &SshSession) -> Option<f64> {
    // Try Linux top format first.
    if let Some(v) = parse_cpu_top(top_out) {
        return Some(v);
    }
    // Try macOS top format.
    let macos_out = session.run_command(CPU_MACOS_CMD).await.unwrap_or_default();
    if let Some(v) = parse_cpu_top_macos(&macos_out) {
        return Some(v);
    }
    let freebsd_out = session
        .run_command(CPU_FREEBSD_CMD)
        .await
        .unwrap_or_default();
    if let Some(v) = parse_cpu_cp_time(&freebsd_out) {
        return Some(v);
    }
    // Fall back to /proc/stat.
    let stat_out = session
        .run_command("head -1 /proc/stat 2>/dev/null")
        .await
        .unwrap_or_default();
    parse_cpu_proc_stat(&stat_out)
}

async fn parse_ram_combined(mem_out: &str, session: &SshSession) -> Option<f64> {
    // Try Linux free -b output.
    if let Some(v) = parse_ram_free(mem_out) {
        return Some(v);
    }
    // vm_stat output (macOS) — also need sysctl hw.memsize.
    if mem_out.contains("Mach Virtual Memory") {
        let memsize_out = session
            .run_command("sysctl hw.memsize 2>/dev/null")
            .await
            .unwrap_or_default();
        return parse_ram_vmstat(mem_out, &memsize_out);
    }
    parse_ram_sysctl(mem_out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_escalates_and_only_a_reset_returns_it_to_the_first_step() {
        let mut backoff = BackoffState::new();
        let steps: Vec<u64> = (0..5).map(|_| backoff.next_delay().as_secs()).collect();
        assert_eq!(steps, vec![30, 60, 120, 300, 300]);

        backoff.reset();
        assert_eq!(backoff.next_delay().as_secs(), 30);
    }

    #[tokio::test(start_paused = true)]
    async fn wait_backoff_ignores_refresh_signals() {
        let (tx, mut rx) = mpsc::channel::<()>(4);
        for _ in 0..4 {
            tx.try_send(()).expect("channel has room");
        }

        let start = tokio::time::Instant::now();
        wait_backoff(Duration::from_secs(300), &mut rx).await;

        assert_eq!(start.elapsed(), Duration::from_secs(300));
    }

    #[tokio::test(start_paused = true)]
    async fn wait_backoff_serves_out_its_delay_once_every_sender_is_gone() {
        let (tx, mut rx) = mpsc::channel::<()>(4);
        drop(tx);

        let start = tokio::time::Instant::now();
        wait_backoff(Duration::from_secs(300), &mut rx).await;

        assert_eq!(start.elapsed(), Duration::from_secs(300));
    }

    #[tokio::test(start_paused = true)]
    async fn wait_or_refresh_serves_out_its_delay_once_every_sender_is_gone() {
        let (tx, mut rx) = mpsc::channel::<()>(4);
        drop(tx);

        let start = tokio::time::Instant::now();
        wait_or_refresh(Duration::from_secs(300), &mut rx).await;

        assert_eq!(start.elapsed(), Duration::from_secs(300));
    }

    #[tokio::test(start_paused = true)]
    async fn wait_or_refresh_still_returns_early() {
        let (tx, mut rx) = mpsc::channel::<()>(4);
        tx.try_send(()).expect("channel has room");

        let start = tokio::time::Instant::now();
        wait_or_refresh(Duration::from_secs(300), &mut rx).await;

        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn metric_commands_pin_the_locale() {
        for cmd in [
            CPU_CMD,
            MEM_CMD,
            DISK_CMD,
            UPTIME_CMD,
            CPU_MACOS_CMD,
            CPU_FREEBSD_CMD,
        ] {
            assert!(cmd.starts_with("env LC_ALL=C "), "unpinned command: {cmd}");
        }
        // Every `||` fallback and both samples need the prefix too.
        assert_eq!(MEM_CMD.matches("env LC_ALL=C ").count(), 3);
        assert_eq!(CPU_FREEBSD_CMD.matches("env LC_ALL=C ").count(), 3);
    }

    #[test]
    fn top_processes_command_pins_numbers_but_keeps_the_host_ctype() {
        let cmd = top_processes_command("-eo pcpu=");

        // Both `ps` invocations are pinned, so a comma-decimal host still parses.
        assert_eq!(cmd.matches(PS_LOCALE).count(), 2);
        // LC_ALL is cleared rather than set: it outranks LC_NUMERIC, so leaving a
        // host's own LC_ALL in place would defeat the pin. LC_CTYPE still falls
        // through to the host, so process names reach the user verbatim.
        assert!(cmd.contains("LC_ALL="));
        assert!(!cmd.contains("LC_ALL=C"));
        assert!(!cmd.contains("LC_CTYPE"));
    }

    #[test]
    fn top_processes_command_excludes_monitor_pid_chain() {
        let cmd = top_processes_command("-eo pid=,ppid=,pcpu=,pmem=,comm= --sort=-pcpu");

        // The grandparent PID is resolved in a substitution that runs before
        // the pipeline does.
        assert!(cmd.starts_with("g=$("));
        assert!(cmd.contains("ps -o ppid= -p $PPID 2>/dev/null | tr -d ' ')"));
        // The awk filter binds the shell, its parent sshd and the grandparent.
        assert!(cmd.contains("-v s=$$"));
        assert!(cmd.contains("-v p=$PPID"));
        assert!(cmd.contains("-v g=\"$g\""));
        // It drops all three PIDs (and the children of the shell and sshd).
        assert!(cmd.contains("$1!=s && $1!=p && $1!=g && $2!=s && $2!=p"));
        // Filtering is by PID only — never by process name.
        assert!(!cmd.contains("sshd"));
        // Output is capped server-side.
        assert!(cmd.trim_end().ends_with("head -n 3"));
    }

    #[test]
    fn top_processes_command_splices_ps_args_verbatim() {
        let linux = top_processes_command("-eo pid=,ppid=,pcpu=,pmem=,comm= --sort=-pcpu");
        assert!(linux.contains("ps -eo pid=,ppid=,pcpu=,pmem=,comm= --sort=-pcpu 2>/dev/null"));

        let bsd = top_processes_command("-Ac -o pid= -o ppid= -o pcpu= -o pmem= -o comm= -r");
        assert!(bsd.contains("ps -Ac -o pid= -o ppid= -o pcpu= -o pmem= -o comm= -r 2>/dev/null"));
    }

    #[test]
    fn top_processes_command_drops_only_the_kernel_idle_process() {
        let cmd = top_processes_command("-Ac -o pid= -o ppid= -o pcpu= -o pmem= -o comm= -r");
        assert!(cmd.contains("!($2==0 && $5==\"idle\")"));
    }
}

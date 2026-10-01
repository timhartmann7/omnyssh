//! Metric collection and parsing for remote servers.
//!
//! Metrics are gathered via SSH commands and parsed into typed
//! values. All parsers return `Option<f32>` — they never panic.
//! Unknown or truncated output results in `None`, which the UI
//! renders as "N/A".
//!
//! Commands used per OS family:
//! - CPU:    `top -bn1` (Linux) / `top -l 1 -n 0` (macOS) / `sysctl kern.cp_time` (FreeBSD)
//! - RAM:    `free -b` (Linux) / `vm_stat` + `sysctl hw.memsize` (macOS) / `sysctl` (FreeBSD)
//! - Disk:   `df -k /`
//! - Uptime: `uptime`
//! - Load:   `cat /proc/loadavg` (Linux), else the tail of `uptime`
//!
//! Severity thresholds:
//!   Ok    < 60 %
//!   Warn  60–85 %
//!   Crit  > 85 %

use crate::event::ProcessInfo;

// ---------------------------------------------------------------------------
// Severity threshold helper (used by card renderer)
// ---------------------------------------------------------------------------

/// Severity of a metric percentage. The UI maps each level to a colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThresholdLevel {
    Ok,
    Warn,
    Crit,
}

/// Returns the severity level for a metric percentage.
pub fn threshold_level(percent: f64) -> ThresholdLevel {
    if percent < 60.0 {
        ThresholdLevel::Ok
    } else if percent <= 85.0 {
        ThresholdLevel::Warn
    } else {
        ThresholdLevel::Crit
    }
}

// ---------------------------------------------------------------------------
// CPU parsers
// ---------------------------------------------------------------------------

/// Parse CPU idle percentage from Linux `top -bn1` output.
///
/// Handles both modern procps-ng format (`%Cpu(s): 2.3 us, ... 96.7 id`)
/// and older format (`Cpu(s):  2.3%us, ... 96.7%id`).
/// Also handles Alpine BusyBox format (`CPU:   4% usr   1% sys  94% idle`).
///
/// Returns `100 - idle_percent` as the used percentage.
pub fn parse_cpu_top(output: &str) -> Option<f64> {
    for line in output.lines() {
        let trimmed = line.trim();

        // Alpine BusyBox: "CPU:   4% usr   1% sys   0% nic  94% idle ..."
        if trimmed.starts_with("CPU:") {
            return parse_cpu_busybox(trimmed);
        }

        // Modern procps-ng: "%Cpu(s):  2.3 us,  0.7 sy,  0.0 ni, 96.7 id, ..."
        // Older format:    "Cpu(s):  2.3%us,  0.7%sy,  0.0%ni, 96.7%id, ..."
        let lower = trimmed.to_lowercase();
        if lower.starts_with("%cpu") || lower.starts_with("cpu(s)") {
            return parse_cpu_linux_top_line(trimmed);
        }
    }
    None
}

fn parse_cpu_busybox(line: &str) -> Option<f64> {
    // Format: "CPU:   4% usr   1% sys   0% nic  94% idle   0% io   0% irq   1% sirq"
    // Find "idle" preceded by a percentage.
    let words: Vec<&str> = line.split_whitespace().collect();
    for i in 0..words.len().saturating_sub(1) {
        if words[i + 1] == "idle" {
            let pct_str = words[i].trim_end_matches('%');
            if let Ok(idle) = pct_str.parse::<f64>() {
                return Some((100.0 - idle).clamp(0.0, 100.0));
            }
        }
    }
    None
}

/// Rewrites a decimal comma as a decimal point.
///
/// The commands are run under a pinned locale, but that is best-effort: a host
/// without `env`, or a wrapper that re-exports a locale, still reaches the
/// parsers with `99,1 id`. Only a comma between two digits is a radix point —
/// every other comma separates fields and must survive.
fn normalize_decimal_commas(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    chars
        .iter()
        .enumerate()
        .map(|(i, &c)| {
            let radix = c == ','
                && i > 0
                && chars[i - 1].is_ascii_digit()
                && chars.get(i + 1).is_some_and(char::is_ascii_digit);
            if radix {
                '.'
            } else {
                c
            }
        })
        .collect()
}

fn parse_cpu_linux_top_line(line: &str) -> Option<f64> {
    let line = normalize_decimal_commas(line);
    // Strip leading label (everything up to and including the colon).
    let after_colon = line.split_once(':')?.1;

    // Find the idle field — it's after "id" or "idle" token.
    // Fields are comma-separated: "  2.3 us,  0.7 sy, ..., 96.7 id, ..."
    // or "%Cpu(s):  2.3 us,  0.7 sy,  0.0 ni, 96.7 id,  0.3 wa, ..."
    let fields: Vec<&str> = after_colon.split(',').collect();
    for field in fields {
        let field = field.trim();
        // Field looks like "96.7 id" or "96.7%id"
        let parts: Vec<&str> = field.splitn(2, [' ', '%']).collect();
        if parts.len() == 2 {
            let label = parts[1].trim().to_lowercase();
            if label == "id" || label == "idle" || label.starts_with("id,") {
                let val_str = parts[0].trim_end_matches('%');
                if let Ok(idle) = val_str.parse::<f64>() {
                    return Some((100.0 - idle).clamp(0.0, 100.0));
                }
            }
        }
    }
    None
}

/// Parse CPU usage from macOS `top -l 1 -n 0` output.
///
/// Format: `CPU usage: 3.17% user, 1.56% sys, 95.26% idle`
pub fn parse_cpu_top_macos(output: &str) -> Option<f64> {
    for line in output.lines() {
        let lower = line.trim().to_lowercase();
        if lower.starts_with("cpu usage:") {
            let line = normalize_decimal_commas(line);
            // Find "idle" value
            for part in line.split(',') {
                let part = part.trim();
                if part.to_lowercase().ends_with("idle") {
                    // "95.26% idle" → 95.26
                    let token = part.split_whitespace().next()?;
                    let idle: f64 = token.trim_end_matches('%').parse().ok()?;
                    return Some((100.0 - idle).clamp(0.0, 100.0));
                }
            }
        }
    }
    None
}

/// Parse CPU usage from Linux `/proc/stat`.
///
/// First line: `cpu  user nice system idle iowait irq softirq steal guest guest_nice`
/// Returns (user+system+...) / total * 100.
pub fn parse_cpu_proc_stat(output: &str) -> Option<f64> {
    let line = output.lines().next()?;
    let mut parts = line.split_whitespace();
    let label = parts.next()?;
    if !label.starts_with("cpu") {
        return None;
    }
    // columns: user nice system idle iowait irq softirq steal guest guest_nice
    let values: Vec<u64> = parts.filter_map(|s| s.parse().ok()).collect();
    if values.len() < 4 {
        return None;
    }
    let idle = values[3] + values.get(4).copied().unwrap_or(0); // idle + iowait
    let total: u64 = values.iter().sum();
    if total == 0 {
        return None;
    }
    // Use f64 to preserve precision for large counters (u64 values can exceed f32 range).
    Some(((total - idle) as f64 / total as f64 * 100.0).clamp(0.0, 100.0))
}

/// Parse CPU usage from two FreeBSD `kern.cp_time` samples taken a moment apart.
///
/// Each line holds the cumulative ticks `user nice sys intr idle`. One sample only
/// gives the average since boot, so the busy share is taken over the difference.
/// The counters are C `long`s, printed signed: on a 32-bit kernel they turn
/// negative and wrap at 2^32.
pub fn parse_cpu_cp_time(output: &str) -> Option<f64> {
    let mut samples = output.lines().filter_map(|line| {
        let ticks: Vec<i64> = line
            .split_whitespace()
            .map(|t| t.parse().ok())
            .collect::<Option<_>>()?;
        (ticks.len() == 5).then_some(ticks)
    });
    let (first, second) = (samples.next()?, samples.next()?);
    let delta: Vec<u64> = second
        .iter()
        .zip(&first)
        .map(|(now, then)| (now - then).rem_euclid(1 << 32) as u64)
        .collect();
    let total: u64 = delta.iter().sum();
    if total == 0 {
        return None;
    }
    Some(((total - delta[4]) as f64 / total as f64 * 100.0).clamp(0.0, 100.0))
}

// ---------------------------------------------------------------------------
// RAM parsers
// ---------------------------------------------------------------------------

/// Parse RAM usage from Linux `free -b` output.
///
/// Uses the `available` column when present (modern `free`), otherwise falls
/// back to `free` column (busybox / older `free`).
///
/// Formula: `(total - available) / total * 100`
pub fn parse_ram_free(output: &str) -> Option<f64> {
    // Skip header line, parse "Mem:" line.
    let mem_line = output
        .lines()
        .find(|l| l.trim_start().starts_with("Mem:"))?;
    let fields: Vec<&str> = mem_line.split_whitespace().collect();
    // Modern: Mem: total used free shared buff/cache available  (7 columns)
    // Busybox: Mem: total used free  (4 columns)
    // Use f64 — RAM values are in bytes (u64 range); f32 loses precision above 16 MiB.
    let total: f64 = fields.get(1)?.parse().ok()?;
    if total == 0.0 {
        return None;
    }
    if let Some(available_str) = fields.get(6) {
        // 7-column format: available in column 6
        let available: f64 = available_str.parse().ok()?;
        Some(((total - available) / total * 100.0).clamp(0.0, 100.0))
    } else if let Some(free_str) = fields.get(3) {
        // 4-column busybox: free in column 3
        let free: f64 = free_str.parse().ok()?;
        Some(((total - free) / total * 100.0).clamp(0.0, 100.0))
    } else {
        None
    }
}

/// Parse RAM usage from macOS `vm_stat` + `sysctl hw.memsize` output.
///
/// `vm_stat_output` is the output of `vm_stat`.
/// `memsize_output` is the output of `sysctl hw.memsize` (e.g. `hw.memsize: 8589934592`).
pub fn parse_ram_vmstat(vm_stat_output: &str, memsize_output: &str) -> Option<f64> {
    // Parse total memory from sysctl
    let total_bytes: f64 = memsize_output.split(':').nth(1)?.trim().parse().ok()?;
    if total_bytes == 0.0 {
        return None;
    }

    // Parse page size from vm_stat (first line: "Mach Virtual Memory Statistics: (page size of 16384 bytes)")
    let page_size: f64 = vm_stat_output
        .lines()
        .next()
        .and_then(|l| {
            let idx = l.find("page size of")?;
            let rest = &l[idx + "page size of".len()..];
            rest.split_whitespace().next()?.parse().ok()
        })
        .unwrap_or(4096.0);

    // Count free + speculative pages
    let mut free_pages: f64 = 0.0;
    let mut speculative_pages: f64 = 0.0;
    for line in vm_stat_output.lines() {
        if let Some(val) = parse_vmstat_line(line, "Pages free:") {
            free_pages = val;
        } else if let Some(val) = parse_vmstat_line(line, "Pages speculative:") {
            speculative_pages = val;
        }
    }
    let available_bytes = (free_pages + speculative_pages) * page_size;
    Some(((total_bytes - available_bytes) / total_bytes * 100.0).clamp(0.0, 100.0))
}

/// Parse RAM usage from FreeBSD `sysctl` counters (`name: value` lines).
///
/// Free and inactive pages, the buffer cache and the ZFS ARC above its floor are
/// what the kernel hands back without swapping — htop's split — so they count as
/// available. Without that, a ZFS host reads as nearly full: the ARC is wired.
/// Formula: `(total - available) / total * 100`
pub fn parse_ram_sysctl(output: &str) -> Option<f64> {
    let value = |key: &str| -> Option<f64> {
        output.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.trim() == key {
                value.trim().parse().ok()
            } else {
                None
            }
        })
    };
    let total = value("vm.stats.vm.v_page_count")?;
    let page_size = value("hw.pagesize")?;
    if total == 0.0 || page_size == 0.0 {
        return None;
    }
    // Absent on a host without ZFS.
    let arc = value("kstat.zfs.misc.arcstats.size").unwrap_or(0.0)
        - value("kstat.zfs.misc.arcstats.c_min").unwrap_or(0.0);
    let available = value("vm.stats.vm.v_free_count")?
        + value("vm.stats.vm.v_inactive_count")?
        + (value("vfs.bufspace").unwrap_or(0.0) + arc.max(0.0)) / page_size;
    Some(((total - available) / total * 100.0).clamp(0.0, 100.0))
}

fn parse_vmstat_line(line: &str, prefix: &str) -> Option<f64> {
    if line.trim_start().starts_with(prefix) {
        line.split(':')
            .nth(1)?
            .trim()
            .trim_end_matches('.')
            .parse()
            .ok()
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Disk parsers
// ---------------------------------------------------------------------------

/// Parse disk usage of `/` from `df -k /` output.
///
/// Handles Linux format (`Use%` column) and macOS format (`Capacity` column).
/// Returns the numeric percentage as `f64`.
pub fn parse_disk_df(output: &str) -> Option<f64> {
    // The percentage is in a column labelled "Use%" or "Capacity".
    // Skip the header and take the first data line.
    let mut lines = output.lines();
    let header = lines.next()?;
    let data_line = lines.next()?;

    let header_fields: Vec<&str> = header.split_whitespace().collect();
    let data_fields: Vec<&str> = data_line.split_whitespace().collect();

    // Find the index of Use% or Capacity in the header.
    let pct_col = header_fields.iter().position(|h| {
        *h == "Use%" || *h == "Capacity" || h.ends_with("Use%") || h.ends_with("Capacity")
    })?;

    let pct_str = data_fields.get(pct_col)?;
    let pct_str = pct_str.trim_end_matches('%');
    pct_str.parse::<f64>().ok().map(|p| p.clamp(0.0, 100.0))
}

// ---------------------------------------------------------------------------
// Uptime parser
// ---------------------------------------------------------------------------

/// Extract the human-readable uptime string from `uptime` output.
///
/// Works on Linux (`14:23:45 up 2 days, 3:45, 2 users`) and
/// macOS (`14:23  up 2 days, 3:45, 2 users`).
/// Returns the portion after "up" and before the next comma-delimited field
/// that doesn't look like a time.
pub fn parse_uptime(output: &str) -> Option<String> {
    let line = output.lines().next()?;
    // Find "up " and take everything up to "user" or end of meaningful part.
    let up_idx = line.to_lowercase().find(" up ")?;
    let after_up = line[up_idx + 4..].trim();

    // The uptime is everything before the user count ("N users" or "N user").
    // Split on ", " and take parts until we hit something with "user".
    let mut parts: Vec<&str> = Vec::new();
    for part in after_up.split(", ") {
        if part.trim().contains("user") {
            break;
        }
        parts.push(part.trim());
    }

    let uptime_str = if parts.is_empty() {
        after_up.to_string()
    } else {
        parts.join(", ")
    };

    // If uptime contains days, remove time portion (everything after "days")
    // User request: show only "251 days" without ", 1:13"
    if let Some(days_idx) = uptime_str.find(" days") {
        let end = days_idx + 5; // " days".len()
        Some(uptime_str[..end].to_string())
    } else if let Some(day_idx) = uptime_str.find(" day,") {
        let end = day_idx + 4; // " day".len()
        Some(uptime_str[..end].to_string())
    } else {
        // For short uptimes (hours/minutes), keep as-is
        Some(uptime_str)
    }
}

/// Extract load averages from `cat /proc/loadavg` output.
///
/// Format: `0.15 0.10 0.08 1/423 12345`
/// Returns the first three space-separated values as a display string.
pub fn parse_loadavg(output: &str) -> Option<String> {
    let line = output.lines().next()?;
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() >= 3 {
        Some(format!("{} {} {}", parts[0], parts[1], parts[2]))
    } else {
        None
    }
}

/// Extract load averages from `uptime` output, for hosts without `/proc/loadavg`.
///
/// Linux and FreeBSD print `load average(s): 0.44, 0.49, 0.27`; macOS separates
/// the values with spaces only.
pub fn parse_loadavg_uptime(output: &str) -> Option<String> {
    let line = normalize_decimal_commas(output.lines().next()?);
    let (_, tail) = line.split_once("load average")?;
    let values: Vec<&str> = tail
        .split_once(':')?
        .1
        .split([',', ' '])
        .filter(|v| !v.is_empty())
        .take(3)
        .collect();
    if values.len() == 3 && values.iter().all(|v| v.parse::<f64>().is_ok()) {
        Some(values.join(" "))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Process list parser
// ---------------------------------------------------------------------------

/// Parse the top processes by CPU usage from `ps` output.
///
/// Expects lines of `%CPU %MEM COMMAND`, sorted by the remote `ps`. An optional
/// header line and any malformed line are skipped (their first two columns are
/// not numeric). Returns at most 3 entries in input order, or `None` if empty.
pub fn parse_top_processes(output: &str) -> Option<Vec<ProcessInfo>> {
    let mut procs: Vec<ProcessInfo> = Vec::new();
    for line in output.lines() {
        if procs.len() == 3 {
            break;
        }
        let mut fields = line.split_whitespace();
        let (Some(cpu_str), Some(mem_str)) = (fields.next(), fields.next()) else {
            continue;
        };
        // The first two columns must be numeric — this skips the header row.
        // `ps` prints them in the server's locale, so a decimal comma is normalised
        // here too; a non-numeric token still fails to parse.
        let (Ok(cpu), Ok(mem)) = (
            normalize_decimal_commas(cpu_str).parse::<f64>(),
            normalize_decimal_commas(mem_str).parse::<f64>(),
        ) else {
            continue;
        };
        let name = fields.collect::<Vec<&str>>().join(" ");
        if name.is_empty() {
            continue;
        }
        procs.push(ProcessInfo {
            name,
            cpu_percent: cpu,
            mem_percent: mem,
        });
    }
    if procs.is_empty() {
        None
    } else {
        Some(procs)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- CPU ----

    #[test]
    fn test_cpu_top_ubuntu_procps_ng() {
        let out = "%Cpu(s):  2.3 us,  0.7 sy,  0.0 ni, 96.7 id,  0.3 wa,  0.0 hi,  0.0 si,  0.0 st";
        let result = parse_cpu_top(out).expect("should parse");
        assert!((result - 3.3).abs() < 0.1, "got {result}");
    }

    #[test]
    fn test_cpu_top_centos_old_format() {
        let out = "Cpu(s):  2.3%us,  0.7%sy,  0.0%ni, 96.7%id,  0.3%wa,  0.0%hi,  0.0%si,  0.0%st";
        let result = parse_cpu_top(out).expect("should parse");
        assert!((result - 3.3).abs() < 0.1, "got {result}");
    }

    #[test]
    fn test_cpu_top_alpine_busybox() {
        let out = "CPU:   4% usr   1% sys   0% nic  94% idle   0% io   0% irq   1% sirq";
        let result = parse_cpu_top(out).expect("should parse");
        assert!((result - 6.0).abs() < 0.1, "got {result}");
    }

    #[test]
    fn test_cpu_top_macos() {
        let out = "CPU usage: 3.17% user, 1.56% sys, 95.26% idle";
        let result = parse_cpu_top_macos(out).expect("should parse");
        assert!((result - 4.74).abs() < 0.1, "got {result}");
    }

    #[test]
    fn test_cpu_top_macos_full_output() {
        let out = "Processes: 412 total, 2 running, 410 sleeping, 2178 threads\n\
                   2024/01/15 14:23:05\n\
                   Load Avg: 1.52, 1.74, 1.89\n\
                   CPU usage: 5.71% user, 2.57% sys, 91.71% idle\n\
                   SharedLibs: 438M resident, 108M data, 24M linkedit.";
        let result = parse_cpu_top_macos(out).expect("should parse");
        assert!((result - 8.29).abs() < 0.1, "got {result}");
    }

    #[test]
    fn test_cpu_proc_stat() {
        // cpu  74608 2520 24433 1117073 6176 4054 0 0 0 0
        let out = "cpu  74608 2520 24433 1117073 6176 4054 0 0 0 0";
        let result = parse_cpu_proc_stat(out).expect("should parse");
        assert!(result > 0.0 && result < 100.0, "got {result}");
    }

    #[test]
    fn test_cpu_cp_time_freebsd_takes_the_delta() {
        // FreeBSD 14.5, idle: one sample alone would read 4.4 % (since boot).
        let out = "3235 2 4258 45 162316\n3237 2 4259 45 162586\n";
        let result = parse_cpu_cp_time(out).expect("should parse");
        assert!((result - 1.1).abs() < 0.05, "got {result}");
    }

    #[test]
    fn test_cpu_cp_time_freebsd_one_of_two_cores_busy() {
        // `yes` on one of two cores; top read 50.0 % idle.
        let out = "8513 2 5182 52 187738\n8621 2 5204 52 187866\n";
        let result = parse_cpu_cp_time(out).expect("should parse");
        assert!((result - 50.4).abs() < 0.05, "got {result}");
    }

    #[test]
    fn test_cpu_cp_time_32_bit_counters_wrap() {
        // A 32-bit kernel prints the counters signed; idle crossed 2^31 here.
        let out = "8513 2 5182 52 2147483600\n8621 2 5204 52 -2147483568\n";
        let result = parse_cpu_cp_time(out).expect("should parse");
        assert!((result - 50.4).abs() < 0.05, "got {result}");
    }

    #[test]
    fn test_cpu_cp_time_rejects_one_sample_or_other_shapes() {
        assert!(parse_cpu_cp_time("3235 2 4258 45 162316\n").is_none());
        assert!(parse_cpu_cp_time("3235 2 4258 45 162316\n3235 2 4258 45 162316\n").is_none());
        // OpenBSD's six comma-separated states are not FreeBSD's five.
        assert!(parse_cpu_cp_time("1,0,2,0,0,9\n2,0,3,0,0,19\n").is_none());
    }

    #[test]
    fn test_cpu_empty_returns_none() {
        assert!(parse_cpu_top("").is_none());
        assert!(parse_cpu_top_macos("").is_none());
        assert!(parse_cpu_proc_stat("").is_none());
        assert!(parse_cpu_cp_time("").is_none());
    }

    #[test]
    fn test_cpu_unrecognized_returns_none() {
        assert!(parse_cpu_top("no cpu info here").is_none());
    }

    // ---- RAM ----

    #[test]
    fn test_ram_free_modern_7col() {
        // free -b output with available column
        let out =
            "              total        used        free      shared  buff/cache   available\n\
                   Mem:    8192000000  3145728000   512000000   134217728  4534272000  4915200000\n\
                   Swap:   2147483648           0  2147483648";
        let result = parse_ram_free(out).expect("should parse");
        // (8192000000 - 4915200000) / 8192000000 * 100 ≈ 39.99
        assert!((result - 40.0).abs() < 1.0, "got {result}");
    }

    #[test]
    fn test_ram_free_busybox_4col() {
        // Alpine / busybox free output (no buff/cache, no available)
        let out = "              total        used        free\n\
                   Mem:       1018736      524288      494448";
        let result = parse_ram_free(out).expect("should parse");
        // (1018736 - 494448) / 1018736 ≈ 51.5
        assert!((result - 51.5).abs() < 1.0, "got {result}");
    }

    #[test]
    fn test_ram_empty_returns_none() {
        assert!(parse_ram_free("").is_none());
        assert!(parse_ram_vmstat("", "").is_none());
        assert!(parse_ram_sysctl("").is_none());
    }

    #[test]
    fn test_ram_sysctl_freebsd_ufs() {
        // FreeBSD 14.5 without ZFS: the unknown kstat names print nothing.
        let out = "hw.pagesize: 4096\n\
                   vm.stats.vm.v_page_count: 504349\n\
                   vm.stats.vm.v_free_count: 475781\n\
                   vm.stats.vm.v_inactive_count: 489\n\
                   vfs.bufspace: 20492800\n";
        let result = parse_ram_sysctl(out).expect("should parse");
        // (504349 - 475781 - 489 - 5003) / 504349
        assert!((result - 4.58).abs() < 0.01, "got {result}");
    }

    #[test]
    fn test_ram_sysctl_freebsd_counts_the_arc_above_its_floor_as_available() {
        // FreeBSD 14.5 with a 518M ARC; top: 11M Active, 329M Inact, 863M Wired,
        // 201M Buf, 767M Free.
        let out = "hw.pagesize: 4096\n\
                   vm.stats.vm.v_page_count: 504349\n\
                   vm.stats.vm.v_free_count: 196859\n\
                   vm.stats.vm.v_inactive_count: 84151\n\
                   vfs.bufspace: 210890752\n\
                   kstat.zfs.misc.arcstats.size: 543477304\n\
                   kstat.zfs.misc.arcstats.c_min: 66203136\n";
        let result = parse_ram_sysctl(out).expect("should parse");
        assert!((result - 10.97).abs() < 0.01, "got {result}");
    }

    #[test]
    fn test_ram_sysctl_needs_the_page_counters() {
        assert!(
            parse_ram_sysctl("hw.pagesize: 4096\nvm.stats.vm.v_page_count: 504349\n").is_none()
        );
        // OpenBSD knows the page size but not FreeBSD's vm.stats tree.
        assert!(parse_ram_sysctl("hw.pagesize: 4096\nhw.physmem: 2147483648\n").is_none());
    }

    #[test]
    fn test_ram_vmstat_macos() {
        let vm_stat = "Mach Virtual Memory Statistics: (page size of 16384 bytes)\n\
                       Pages free:                               23456.\n\
                       Pages active:                           456789.\n\
                       Pages inactive:                         123456.\n\
                       Pages speculative:                        12345.\n\
                       Pages throttled:                              0.\n\
                       Pages wired down:                        98765.\n";
        let memsize = "hw.memsize: 17179869184";
        let result = parse_ram_vmstat(vm_stat, memsize).expect("should parse");
        assert!(result > 0.0 && result <= 100.0, "got {result}");
    }

    // ---- Disk ----

    #[test]
    fn test_disk_df_linux() {
        let out = "Filesystem     1K-blocks    Used Available Use% Mounted on\n\
                   /dev/sda1       51475068 9000000  39841436  19% /";
        let result = parse_disk_df(out).expect("should parse");
        assert!((result - 19.0).abs() < 0.1, "got {result}");
    }

    #[test]
    fn test_disk_df_macos() {
        let out = "Filesystem   1024-blocks      Used Available Capacity iused ifree %iused  Mounted on\n\
                   /dev/disk3s5   994662584 516879368 400765064    57% 5488234 4293478045    0%   /";
        let result = parse_disk_df(out).expect("should parse");
        assert!((result - 57.0).abs() < 0.1, "got {result}");
    }

    #[test]
    fn test_disk_df_100pct() {
        let out = "Filesystem     1K-blocks    Used Available Use% Mounted on\n\
                   /dev/sda1       51475068 51475068          0 100% /";
        let result = parse_disk_df(out).expect("should parse");
        assert!((result - 100.0).abs() < 0.1, "got {result}");
    }

    #[test]
    fn test_disk_empty_returns_none() {
        assert!(parse_disk_df("").is_none());
        assert!(parse_disk_df("only header\n").is_none());
    }

    // ---- Uptime ----

    #[test]
    fn test_uptime_linux_days() {
        let out = " 14:23:45 up 2 days,  3:45,  2 users,  load average: 0.15, 0.10, 0.08";
        let result = parse_uptime(out).expect("should parse");
        // When uptime includes days, only show days portion (user request: no time after days)
        assert_eq!(result, "2 days", "expected '2 days' only, got: {result}");
    }

    #[test]
    fn test_uptime_linux_hours() {
        let out = " 10:00:00 up  3:45,  1 user,  load average: 0.00, 0.01, 0.05";
        let result = parse_uptime(out).expect("should parse");
        assert!(result.contains("3:45"), "missing '3:45': {result}");
    }

    #[test]
    fn test_uptime_empty_returns_none() {
        assert!(parse_uptime("").is_none());
    }

    // ---- Load avg ----

    #[test]
    fn test_loadavg() {
        let out = "0.15 0.10 0.08 1/423 12345\n";
        let result = parse_loadavg(out).expect("should parse");
        assert_eq!(result, "0.15 0.10 0.08");
    }

    #[test]
    fn test_loadavg_uptime() {
        let freebsd = " 6:29AM  up 11 mins, 1 user, load averages: 0.35, 0.38, 0.27";
        assert_eq!(
            parse_loadavg_uptime(freebsd).as_deref(),
            Some("0.35 0.38 0.27")
        );
        let linux = " 14:23:45 up 2 days,  3:45,  2 users,  load average: 0.15, 0.10, 0.08";
        assert_eq!(
            parse_loadavg_uptime(linux).as_deref(),
            Some("0.15 0.10 0.08")
        );
        let macos = "14:23  up 2 days,  3:45, 2 users, load averages: 1.52 1.74 1.89";
        assert_eq!(
            parse_loadavg_uptime(macos).as_deref(),
            Some("1.52 1.74 1.89")
        );
        let comma = " 14:23:45 up 2 days,  2 users,  load average: 0,15, 0,10, 0,08";
        assert_eq!(
            parse_loadavg_uptime(comma).as_deref(),
            Some("0.15 0.10 0.08")
        );
        assert!(parse_loadavg_uptime("").is_none());
        assert!(parse_loadavg_uptime(" 6:29AM  up 11 mins, 1 user").is_none());
    }

    // ---- Top processes ----

    #[test]
    fn test_top_processes_linux() {
        let out = "%CPU %MEM COMMAND\n\
                   12.3  4.5 firefox\n\
                    8.1  2.0 node\n\
                    3.0  1.1 sshd";
        let procs = parse_top_processes(out).expect("should parse");
        assert_eq!(procs.len(), 3);
        assert_eq!(procs[0].name, "firefox");
        assert!((procs[0].cpu_percent - 12.3).abs() < 0.01);
        assert!((procs[0].mem_percent - 4.5).abs() < 0.01);
        assert_eq!(procs[2].name, "sshd");
    }

    #[test]
    fn test_top_processes_macos_header() {
        let out = "%CPU %MEM COMM\n  5.0  3.2 WindowServer";
        let procs = parse_top_processes(out).expect("should parse");
        assert_eq!(procs.len(), 1);
        assert_eq!(procs[0].name, "WindowServer");
    }

    #[test]
    fn test_top_processes_command_with_spaces() {
        let out = "%CPU %MEM COMMAND\n 2.5  9.0 postgres: writer process";
        let procs = parse_top_processes(out).expect("should parse");
        assert_eq!(procs[0].name, "postgres: writer process");
    }

    #[test]
    fn test_top_processes_skips_malformed_lines() {
        let out = "%CPU %MEM COMMAND\n\
                   garbage line\n\
                   10.0  1.0 redis\n\
                   \n\
                   only-one-field";
        let procs = parse_top_processes(out).expect("should parse");
        assert_eq!(procs.len(), 1);
        assert_eq!(procs[0].name, "redis");
    }

    #[test]
    fn test_top_processes_caps_at_three() {
        let out = "%CPU %MEM COMMAND\n\
                   5.0 1.0 a\n4.0 1.0 b\n3.0 1.0 c\n2.0 1.0 d\n1.0 1.0 e";
        let procs = parse_top_processes(out).expect("should parse");
        assert_eq!(procs.len(), 3);
        assert_eq!(procs[2].name, "c");
    }

    #[test]
    fn test_top_processes_no_header() {
        // The collector now suppresses the `ps` header, so every line is data.
        let out = "12.3  4.5 firefox\n 3.0  1.1 sshd";
        let procs = parse_top_processes(out).expect("should parse");
        assert_eq!(procs.len(), 2);
        assert_eq!(procs[0].name, "firefox");
        assert_eq!(procs[1].name, "sshd");
    }

    #[test]
    fn test_top_processes_freebsd() {
        // FreeBSD 14.5 `ps -Ac -o pid= ... -r` after the collector's awk filter.
        let out = "0.4 0.4 sshd\n0.1 0.0 intr\n0.0 0.1 kernel\n";
        let procs = parse_top_processes(out).expect("should parse");
        assert_eq!(procs.len(), 3);
        assert_eq!(procs[0].name, "sshd");
        assert!((procs[0].cpu_percent - 0.4).abs() < 0.01);
    }

    #[test]
    fn test_top_processes_empty_returns_none() {
        assert!(parse_top_processes("").is_none());
        assert!(parse_top_processes("%CPU %MEM COMMAND\n").is_none());
    }

    // ---- Threshold level ----

    #[test]
    fn test_threshold_level() {
        assert_eq!(threshold_level(0.0), ThresholdLevel::Ok);
        assert_eq!(threshold_level(59.9), ThresholdLevel::Ok);
        assert_eq!(threshold_level(60.0), ThresholdLevel::Warn);
        assert_eq!(threshold_level(85.0), ThresholdLevel::Warn);
        assert_eq!(threshold_level(85.1), ThresholdLevel::Crit);
        assert_eq!(threshold_level(100.0), ThresholdLevel::Crit);
    }
}

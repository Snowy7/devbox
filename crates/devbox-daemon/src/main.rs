use devbox_snapshot::{scan_local_change_feed, LocalChangeFeedScan, LocalChangeFeedScanOptions};
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::{Duration, Instant};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const DEFAULT_DEBOUNCE_MS: u64 = 500;

fn main() -> ExitCode {
    let args = std::env::args().skip(1).collect::<Vec<_>>();

    match args.first().map(String::as_str) {
        Some("--version") | Some("-V") | Some("version") => {
            println!("devbox-daemon {VERSION}");
            ExitCode::SUCCESS
        }
        Some("watch") => match parse_watch_args(&args[1..]).and_then(run_watch) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("watch error={}", script_value(&error));
                ExitCode::from(1)
            }
        },
        Some("--help") | Some("-h") | None => {
            print_help();
            ExitCode::SUCCESS
        }
        Some(command) => {
            eprintln!("devbox-daemon: unknown command '{command}'");
            eprintln!("Run 'devbox-daemon --help' for usage.");
            ExitCode::from(2)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WatchArgs {
    db_path: PathBuf,
    cache_root: PathBuf,
    project_root: PathBuf,
    once: bool,
    debounce_ms: u64,
    exit_after_idle_ms: Option<u64>,
    max_scans: Option<usize>,
}

fn parse_watch_args(args: &[String]) -> Result<WatchArgs, String> {
    let mut db_path = None;
    let mut cache_root = None;
    let mut project_root = None;
    let mut once = false;
    let mut debounce_ms = DEFAULT_DEBOUNCE_MS;
    let mut exit_after_idle_ms = None;
    let mut max_scans = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--db" => {
                index += 1;
                db_path = Some(PathBuf::from(
                    args.get(index)
                        .ok_or_else(|| "--db requires a path".to_string())?,
                ));
            }
            "--cache" => {
                index += 1;
                cache_root = Some(PathBuf::from(
                    args.get(index)
                        .ok_or_else(|| "--cache requires a path".to_string())?,
                ));
            }
            "--once" => once = true,
            "--debounce-ms" => {
                index += 1;
                debounce_ms = parse_u64_flag(
                    "--debounce-ms",
                    args.get(index)
                        .ok_or_else(|| "--debounce-ms requires a value".to_string())?,
                )?;
            }
            "--exit-after-idle-ms" => {
                index += 1;
                exit_after_idle_ms = Some(parse_u64_flag(
                    "--exit-after-idle-ms",
                    args.get(index)
                        .ok_or_else(|| "--exit-after-idle-ms requires a value".to_string())?,
                )?);
            }
            "--max-scans" => {
                index += 1;
                max_scans = Some(parse_usize_flag(
                    "--max-scans",
                    args.get(index)
                        .ok_or_else(|| "--max-scans requires a value".to_string())?,
                )?);
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown watch option '{value}'"));
            }
            value => {
                if project_root.replace(PathBuf::from(value)).is_some() {
                    return Err("watch accepts exactly one project root".to_string());
                }
            }
        }

        index += 1;
    }

    Ok(WatchArgs {
        db_path: db_path.ok_or_else(|| "watch requires --db <DB_PATH>".to_string())?,
        cache_root: cache_root.ok_or_else(|| "watch requires --cache <CACHE_ROOT>".to_string())?,
        project_root: project_root.ok_or_else(|| "watch requires a project root".to_string())?,
        once,
        debounce_ms,
        exit_after_idle_ms,
        max_scans,
    })
}

fn parse_u64_flag(flag: &str, value: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .map_err(|_| format!("{flag} requires a non-negative integer"))
}

fn parse_usize_flag(flag: &str, value: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .map_err(|_| format!("{flag} requires a non-negative integer"))
}

fn run_watch(args: WatchArgs) -> Result<(), String> {
    println!(
        "watch status=start db={} cache={} project={} debounce_ms={} once={} max_scans={}",
        script_value(&args.db_path.display().to_string()),
        script_value(&args.cache_root.display().to_string()),
        script_value(&args.project_root.display().to_string()),
        args.debounce_ms,
        args.once,
        args.max_scans
            .map(|scans| scans.to_string())
            .unwrap_or_else(|| "-".to_string())
    );

    if args.once {
        println!("watch event=batched reason=once events=0");
        run_scan(&args, 1)?;
        println!("watch status=idle scans=1");
        return Ok(());
    }

    let (tx, rx) = mpsc::channel();
    let mut watcher = RecommendedWatcher::new(
        move |event| {
            let _ = tx.send(event);
        },
        Config::default(),
    )
    .map_err(|error| format!("watcher_create_failed:{error}"))?;
    watcher
        .watch(&args.project_root, RecursiveMode::Recursive)
        .map_err(|error| format!("watcher_watch_failed:{error}"))?;

    let start = Instant::now();
    let mut planner = DebouncePlanner::new(args.debounce_ms);
    let mut scans = 0usize;
    let mut idle_since = Instant::now();

    loop {
        let timeout = receive_timeout(&planner, start, args.exit_after_idle_ms, idle_since);
        match rx.recv_timeout(timeout) {
            Ok(Ok(event)) => {
                let pending = planner.record_event(elapsed_ms(start));
                idle_since = Instant::now();
                println!(
                    "watch event=received pending_batch={} kind={:?} paths={}",
                    pending,
                    event.kind,
                    event.paths.len()
                );
            }
            Ok(Err(error)) => {
                eprintln!("watch error={}", script_value(&format!("notify:{error}")));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("watch event channel disconnected".to_string());
            }
        }

        if let Some(batch_size) = planner.take_due_batch(elapsed_ms(start)) {
            println!(
                "watch event=batched reason=debounce events={} debounce_ms={}",
                batch_size, args.debounce_ms
            );
            scans += 1;
            run_scan(&args, scans)?;
            idle_since = Instant::now();
            println!("watch status=idle scans={scans}");

            if args.max_scans.is_some_and(|max_scans| scans >= max_scans) {
                return Ok(());
            }
        }

        if !planner.has_pending()
            && args
                .exit_after_idle_ms
                .is_some_and(|idle_ms| idle_since.elapsed() >= Duration::from_millis(idle_ms))
        {
            println!("watch status=idle_timeout scans={scans}");
            return Ok(());
        }
    }
}

fn run_scan(args: &WatchArgs, scan_index: usize) -> Result<LocalChangeFeedScan, String> {
    let options =
        LocalChangeFeedScanOptions::new(&args.db_path, &args.cache_root, &args.project_root);
    let scan = scan_local_change_feed(&options).map_err(|error| error.to_string())?;
    print_scan_summary(scan_index, &scan);
    Ok(scan)
}

fn print_scan_summary(scan_index: usize, scan: &LocalChangeFeedScan) {
    let summary = scan.summary();
    println!(
        "watch scan={} project_id={} base_snapshot_id={} created={} modified={} deleted={} unchanged={} skipped_deferred={} pending_operations={} bytes_to_upload={} bytes_deleted={}",
        scan_index,
        script_value(scan.project_id()),
        script_value(scan.base_snapshot_id().unwrap_or("-")),
        summary.created(),
        summary.modified(),
        summary.deleted(),
        summary.unchanged(),
        summary.skipped_deferred(),
        scan.pending_operations(),
        summary.bytes_to_upload(),
        summary.bytes_deleted()
    );
}

fn receive_timeout(
    planner: &DebouncePlanner,
    start: Instant,
    exit_after_idle_ms: Option<u64>,
    idle_since: Instant,
) -> Duration {
    let debounce_timeout = planner.next_scan_at_ms().map(|deadline| {
        let now = elapsed_ms(start);
        Duration::from_millis(deadline.saturating_sub(now))
    });
    let idle_timeout = exit_after_idle_ms
        .map(|idle_ms| Duration::from_millis(idle_ms).saturating_sub(idle_since.elapsed()));

    match (debounce_timeout, idle_timeout) {
        (Some(left), Some(right)) => left.min(right),
        (Some(timeout), None) | (None, Some(timeout)) => timeout,
        (None, None) => Duration::from_secs(60),
    }
}

fn elapsed_ms(start: Instant) -> u64 {
    start.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

fn script_value(value: &str) -> String {
    value.replace('\\', "/").replace(' ', "%20")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DebouncePlanner {
    debounce_ms: u64,
    pending_events: usize,
    next_scan_at_ms: Option<u64>,
}

impl DebouncePlanner {
    fn new(debounce_ms: u64) -> Self {
        Self {
            debounce_ms,
            pending_events: 0,
            next_scan_at_ms: None,
        }
    }

    fn record_event(&mut self, now_ms: u64) -> usize {
        self.pending_events += 1;
        self.next_scan_at_ms = Some(now_ms.saturating_add(self.debounce_ms));
        self.pending_events
    }

    fn take_due_batch(&mut self, now_ms: u64) -> Option<usize> {
        let due = self
            .next_scan_at_ms
            .is_some_and(|deadline| deadline <= now_ms);
        if !due {
            return None;
        }

        let batch_size = self.pending_events;
        self.pending_events = 0;
        self.next_scan_at_ms = None;
        Some(batch_size)
    }

    fn next_scan_at_ms(&self) -> Option<u64> {
        self.next_scan_at_ms
    }

    fn has_pending(&self) -> bool {
        self.pending_events > 0
    }
}

fn print_help() {
    println!("devbox-daemon {VERSION}");
    println!();
    println!("Usage: devbox-daemon <COMMAND>");
    println!();
    println!("Commands:");
    println!("  watch    Watch a project tree and feed the local pending change log");
    println!();
    println!("Watch usage:");
    println!(
        "  devbox-daemon watch --db <DB_PATH> --cache <CACHE_ROOT> <PROJECT_ROOT> [--once] [--debounce-ms <MS>] [--exit-after-idle-ms <MS>] [--max-scans <N>]"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debounce_batches_bursts_until_quiet_window_elapses() {
        let mut planner = DebouncePlanner::new(100);

        assert_eq!(planner.record_event(0), 1);
        assert_eq!(planner.next_scan_at_ms(), Some(100));
        assert_eq!(planner.record_event(50), 2);
        assert_eq!(planner.next_scan_at_ms(), Some(150));
        assert_eq!(planner.take_due_batch(149), None);
        assert_eq!(planner.take_due_batch(150), Some(2));
        assert_eq!(planner.next_scan_at_ms(), None);
    }

    #[test]
    fn parse_watch_args_defaults_to_debounced_long_running_watch() {
        let args = vec![
            "--db".to_string(),
            "devbox.sqlite3".to_string(),
            "--cache".to_string(),
            "cache".to_string(),
            "project".to_string(),
        ];

        let parsed = parse_watch_args(&args).expect("args parse");

        assert!(!parsed.once);
        assert_eq!(parsed.debounce_ms, DEFAULT_DEBOUNCE_MS);
        assert_eq!(parsed.max_scans, None);
        assert_eq!(parsed.project_root, PathBuf::from("project"));
    }

    #[test]
    fn parse_watch_args_accepts_deterministic_test_flags() {
        let args = vec![
            "--db".to_string(),
            "devbox.sqlite3".to_string(),
            "--cache".to_string(),
            "cache".to_string(),
            "--once".to_string(),
            "--debounce-ms".to_string(),
            "25".to_string(),
            "--exit-after-idle-ms".to_string(),
            "50".to_string(),
            "--max-scans".to_string(),
            "2".to_string(),
            "project".to_string(),
        ];

        let parsed = parse_watch_args(&args).expect("args parse");

        assert!(parsed.once);
        assert_eq!(parsed.debounce_ms, 25);
        assert_eq!(parsed.exit_after_idle_ms, Some(50));
        assert_eq!(parsed.max_scans, Some(2));
    }
}

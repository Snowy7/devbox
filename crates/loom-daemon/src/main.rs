use std::process::ExitCode;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> ExitCode {
    let args = std::env::args().skip(1).collect::<Vec<_>>();

    match args.first().map(String::as_str) {
        Some("--version") | Some("-V") | Some("version") => {
            println!("loom-daemon {VERSION}");
            ExitCode::SUCCESS
        }
        Some("run") => result_to_exit(parse_run_args(&args[1..]).and_then(|options| {
            loom_daemon::run_loop(&options).map_err(|error| error.to_string())
        })),
        Some("--help") | Some("-h") | None => {
            print_help();
            ExitCode::SUCCESS
        }
        Some(command) => {
            eprintln!("loom-daemon: unknown command '{command}'");
            eprintln!("Run 'loom-daemon --help' for usage.");
            ExitCode::from(2)
        }
    }
}

fn parse_run_args(args: &[String]) -> Result<loom_daemon::DaemonLoopOptions, String> {
    let mut folder = None;
    let mut debounce_ms = loom_daemon::DEFAULT_DEBOUNCE_MS;
    let mut poll_ms = loom_daemon::DEFAULT_POLL_MS;
    let mut max_cycles = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--debounce-ms" => {
                index += 1;
                debounce_ms = parse_u64(
                    "--debounce-ms",
                    args.get(index)
                        .ok_or_else(|| "--debounce-ms requires a value".to_string())?,
                )?;
            }
            "--poll-ms" => {
                index += 1;
                poll_ms = parse_u64(
                    "--poll-ms",
                    args.get(index)
                        .ok_or_else(|| "--poll-ms requires a value".to_string())?,
                )?;
            }
            "--max-cycles" => {
                index += 1;
                max_cycles = Some(parse_usize(
                    "--max-cycles",
                    args.get(index)
                        .ok_or_else(|| "--max-cycles requires a value".to_string())?,
                )?);
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown run option '{value}'"));
            }
            value => {
                if folder.replace(std::path::PathBuf::from(value)).is_some() {
                    return Err("run accepts exactly one folder".to_string());
                }
            }
        }

        index += 1;
    }

    let mut options = loom_daemon::DaemonLoopOptions::new(
        folder.ok_or_else(|| "run requires a folder".to_string())?,
    );
    options.debounce_ms = debounce_ms;
    options.poll_ms = poll_ms;
    options.max_cycles = max_cycles;
    Ok(options)
}

fn parse_u64(flag: &str, value: &str) -> Result<u64, String> {
    value
        .parse()
        .map_err(|_| format!("{flag} requires a non-negative integer"))
}

fn parse_usize(flag: &str, value: &str) -> Result<usize, String> {
    value
        .parse()
        .map_err(|_| format!("{flag} requires a non-negative integer"))
}

fn result_to_exit(result: Result<(), String>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("loom-daemon: {error}");
            ExitCode::from(1)
        }
    }
}

fn print_help() {
    println!("loom-daemon {VERSION}");
    println!();
    println!("Usage: loom-daemon <COMMAND>");
    println!();
    println!("Commands:");
    println!("  run      Watch a shared folder and run background sync");
    println!();
    println!("Run usage:");
    println!("  loom-daemon run <FOLDER> [--debounce-ms <MS>] [--poll-ms <MS>] [--max-cycles <N>]");
    println!();
    println!("Options:");
    println!("  -h, --help     Print help");
    println!("  -V, --version  Print version");
}

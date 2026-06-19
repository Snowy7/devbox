use std::process::ExitCode;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> ExitCode {
    let args = std::env::args().skip(1).collect::<Vec<_>>();

    match args.first().map(String::as_str) {
        Some("--version") | Some("-V") | Some("version") => {
            println!("loom-daemon {VERSION}");
            ExitCode::SUCCESS
        }
        Some("run") => {
            println!("loom-daemon run: not implemented yet");
            println!(
                "Planned behavior: watch shared folders, capture file versions, coalesce folder revisions, and run background sync."
            );
            ExitCode::SUCCESS
        }
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

fn print_help() {
    println!("loom-daemon {VERSION}");
    println!();
    println!("Usage: loom-daemon <COMMAND>");
    println!();
    println!("Commands:");
    println!("  run      Watch shared folders and run background sync (placeholder)");
    println!();
    println!("Options:");
    println!("  -h, --help     Print help");
    println!("  -V, --version  Print version");
}

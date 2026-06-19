use std::process::ExitCode;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Copy)]
struct CommandSpec {
    name: &'static str,
    usage: &'static str,
    summary: &'static str,
    planned_behavior: &'static str,
}

const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        name: "track",
        usage: "loom track <FOLDER>",
        summary: "Start tracking a shared folder",
        planned_behavior: "capture file versions and make the folder durable locally",
    },
    CommandSpec {
        name: "status",
        usage: "loom status [FOLDER]",
        summary: "Show local folder state",
        planned_behavior:
            "summarize pending file versions, current folder revision, and sync cursor state",
    },
    CommandSpec {
        name: "history",
        usage: "loom history [FOLDER]",
        summary: "List folder revisions and checkpoints",
        planned_behavior: "show automatic folder revisions plus human checkpoints",
    },
    CommandSpec {
        name: "checkpoint",
        usage: "loom checkpoint [FOLDER] -m <MESSAGE>",
        summary: "Name the current folder revision",
        planned_behavior: "attach a human message to a durable folder revision",
    },
    CommandSpec {
        name: "restore",
        usage: "loom restore <REVISION>",
        summary: "Restore a folder revision",
        planned_behavior: "materialize a previous folder revision with safety checks",
    },
    CommandSpec {
        name: "sync",
        usage: "loom sync [FOLDER]",
        summary: "Synchronize a shared folder",
        planned_behavior: "reconcile local and remote folder revisions through Loom cursors",
    },
    CommandSpec {
        name: "clone",
        usage: "loom clone <REMOTE> [FOLDER]",
        summary: "Materialize a shared folder on this machine",
        planned_behavior: "create a local shared folder from a Loom remote without assuming Git",
    },
];

fn main() -> ExitCode {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    run(args)
}

fn run(args: Vec<String>) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("--version") | Some("-V") | Some("version") => {
            println!("loom {VERSION}");
            ExitCode::SUCCESS
        }
        Some("--help") | Some("-h") | None => {
            print_help();
            ExitCode::SUCCESS
        }
        Some(command_name) => match COMMANDS.iter().find(|command| command.name == command_name) {
            Some(command)
                if args
                    .get(1)
                    .is_some_and(|arg| arg == "--help" || arg == "-h") =>
            {
                print_command_help(command);
                ExitCode::SUCCESS
            }
            Some(command) => {
                run_placeholder(command);
                ExitCode::SUCCESS
            }
            None => {
                eprintln!("loom: unknown command '{command_name}'");
                eprintln!("Run 'loom --help' for usage.");
                ExitCode::from(2)
            }
        },
    }
}

fn run_placeholder(command: &CommandSpec) {
    println!("loom {}: not implemented yet", command.name);
    println!("Purpose: {}", command.summary);
    println!("Planned behavior: {}", command.planned_behavior);
}

fn print_help() {
    println!("loom {VERSION}");
    println!();
    println!("Usage: loom <COMMAND>");
    println!();
    println!("Commands:");
    for command in COMMANDS {
        println!("  {:<10} {}", command.name, command.summary);
    }
    println!();
    println!("Options:");
    println!("  -h, --help     Print help");
    println!("  -V, --version  Print version");
}

fn print_command_help(command: &CommandSpec) {
    println!("loom {} - {}", command.name, command.summary);
    println!();
    println!("Usage: {}", command.usage);
    println!();
    println!("Status: not implemented yet");
    println!("Planned behavior: {}", command.planned_behavior);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_include_mvp_surface() {
        let names = COMMANDS
            .iter()
            .map(|command| command.name)
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec![
                "track",
                "status",
                "history",
                "checkpoint",
                "restore",
                "sync",
                "clone"
            ]
        );
    }
}

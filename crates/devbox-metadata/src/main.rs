use devbox_metadata::serve_sqlite;
use std::net::SocketAddr;
use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match run(&args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("devbox-metadata: {error}");
            eprintln!("Usage: devbox-metadata --db <SQLITE_PATH> [--listen <ADDR:PORT>]");
            ExitCode::from(1)
        }
    }
}

async fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let mut db_path = None;
    let mut listen = "127.0.0.1:8787".to_string();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--db" => {
                index += 1;
                db_path = args.get(index).cloned();
            }
            "--listen" => {
                index += 1;
                listen = args
                    .get(index)
                    .ok_or("--listen requires <ADDR:PORT>")?
                    .clone();
            }
            "--help" | "-h" => {
                println!("Usage: devbox-metadata --db <SQLITE_PATH> [--listen <ADDR:PORT>]");
                return Ok(());
            }
            value => return Err(format!("unknown option '{value}'").into()),
        }
        index += 1;
    }

    let db_path = db_path.ok_or("--db <SQLITE_PATH> is required")?;
    let addr: SocketAddr = listen.parse()?;
    serve_sqlite(&db_path, addr).await?;
    Ok(())
}

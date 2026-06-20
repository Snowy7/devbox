use devbox_api::LocalDevboxApi;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("devbox-api: {error}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut root = std::env::var("DEVBOX_API_ROOT").ok().map(PathBuf::from);
    let mut bind = std::env::var("PORT")
        .ok()
        .map(|port| format!("0.0.0.0:{port}"))
        .or_else(|| std::env::var("DEVBOX_API_BIND").ok())
        .unwrap_or_else(|| "127.0.0.1:0".to_string());

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => {
                root = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| "--root requires a path".to_string())?,
                ));
            }
            "--bind" => {
                bind = args
                    .next()
                    .ok_or_else(|| "--bind requires an address".to_string())?;
            }
            "--help" | "-h" => {
                println!("devbox-api --root <PATH> [--bind 127.0.0.1:0]");
                println!("Env: DEVBOX_API_ROOT, DEVBOX_API_BIND, PORT");
                return Ok(());
            }
            value => return Err(format!("unknown option '{value}'")),
        }
    }

    let root = root.unwrap_or_else(|| PathBuf::from(".devbox-api"));
    let api = LocalDevboxApi::open(&root).map_err(|error| error.to_string())?;
    let listener = TcpListener::bind(&bind).map_err(|error| error.to_string())?;
    let addr = listener.local_addr().map_err(|error| error.to_string())?;

    println!("devbox-api running locally at http://{addr}");
    println!("Storage: {}", api.root().display());
    api.serve(listener).map_err(|error| error.to_string())
}

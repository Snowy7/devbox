use bindhub_api::LocalBindhubApi;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bindhub-api: {error}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut root = std::env::var("BINDHUB_API_ROOT").ok().map(PathBuf::from);
    let mut bind = std::env::var("PORT")
        .ok()
        .map(|port| format!("0.0.0.0:{port}"))
        .or_else(|| std::env::var("BINDHUB_API_BIND").ok())
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
                println!("bindhub-api --root <PATH> [--bind 127.0.0.1:0]");
                println!("Env: DATABASE_URL or BINDHUB_API_DATABASE_URL, BINDHUB_API_METADATA_MODE=memory for local smoke/dev, BINDHUB_API_ROOT, BINDHUB_API_BIND, PORT");
                println!("Auth: CLI browser login uses /v1/auth/cli-device-flow plus service-token approval after web-side WorkOS verification");
                println!("Local dev: /v1/auth/dev-session requires BINDHUB_API_ENABLE_LOCAL_DEV_SESSION=1 and is not production auth");
                println!("R2 pack storage: BINDHUB_R2_ENDPOINT, BINDHUB_R2_BUCKET, BINDHUB_R2_ACCESS_KEY_ID, BINDHUB_R2_SECRET_ACCESS_KEY, optional BINDHUB_R2_REGION, BINDHUB_R2_PREFIX, BINDHUB_R2_SESSION_TOKEN");
                return Ok(());
            }
            value => return Err(format!("unknown option '{value}'")),
        }
    }

    let root = root.unwrap_or_else(|| PathBuf::from(".bindhub-api"));
    let api = LocalBindhubApi::open_from_env(&root).map_err(|error| error.to_string())?;
    let listener = TcpListener::bind(&bind).map_err(|error| error.to_string())?;
    let addr = listener.local_addr().map_err(|error| error.to_string())?;

    println!("bindhub-api running locally at http://{addr}");
    println!("Storage: {}", api.root().display());
    println!("Metadata storage: {}", api.metadata_storage_label());
    println!("Pack storage: {}", api.pack_storage_label());
    api.serve(listener).map_err(|error| error.to_string())
}

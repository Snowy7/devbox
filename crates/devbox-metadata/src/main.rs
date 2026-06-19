use devbox_metadata::{
    serve_sqlite_with_config, HostedApiConfig, HostedAuthPolicy, ManagedObjectAccessBrokerConfig,
};
use std::net::SocketAddr;
use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match run(&args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("devbox-metadata: {error}");
            eprintln!("Usage: devbox-metadata [--db <SQLITE_PATH>] [--listen <ADDR:PORT>] [--allow-mock-auth] [--session-ttl-seconds <SECONDS>] [--proof-ttl-seconds <SECONDS>]");
            ExitCode::from(1)
        }
    }
}

async fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let mut db_path = std::env::var("DEVBOX_METADATA_DB").ok();
    let mut listen = std::env::var("DEVBOX_METADATA_LISTEN")
        .ok()
        .or_else(|| {
            std::env::var("PORT")
                .ok()
                .map(|port| format!("0.0.0.0:{port}"))
        })
        .unwrap_or_else(|| "127.0.0.1:8787".to_string());
    let mut config = HostedApiConfig::hosted_alpha();
    if std::env::var("DEVBOX_ALLOW_MOCK_AUTH")
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        config.auth_policy = HostedAuthPolicy::MockDevAndAccountSession;
    }
    if let Ok(value) = std::env::var("DEVBOX_SESSION_TTL_SECONDS") {
        config.session_ttl_seconds = value.parse()?;
    }
    if let Ok(value) = std::env::var("DEVBOX_PROOF_TTL_SECONDS") {
        config.proof_ttl_seconds = value.parse()?;
    }
    config.object_access_broker = object_access_broker_config_from_env()?;
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
            "--allow-mock-auth" => {
                config.auth_policy = HostedAuthPolicy::MockDevAndAccountSession;
            }
            "--session-ttl-seconds" => {
                index += 1;
                config.session_ttl_seconds = args
                    .get(index)
                    .ok_or("--session-ttl-seconds requires <SECONDS>")?
                    .parse()?;
            }
            "--proof-ttl-seconds" => {
                index += 1;
                config.proof_ttl_seconds = args
                    .get(index)
                    .ok_or("--proof-ttl-seconds requires <SECONDS>")?
                    .parse()?;
            }
            "--help" | "-h" => {
                println!("Usage: devbox-metadata [--db <SQLITE_PATH>] [--listen <ADDR:PORT>] [--allow-mock-auth] [--session-ttl-seconds <SECONDS>] [--proof-ttl-seconds <SECONDS>]");
                println!("Env: DEVBOX_METADATA_DB, DEVBOX_METADATA_LISTEN, PORT, DEVBOX_ALLOW_MOCK_AUTH, DEVBOX_SESSION_TTL_SECONDS, DEVBOX_PROOF_TTL_SECONDS");
                println!("Object broker env: DEVBOX_OBJECT_LOCAL_ROOT for local/dev server-mediated object storage, or DEVBOX_OBJECT_ACCESS_KEY_ENV, DEVBOX_OBJECT_SECRET_KEY_ENV, DEVBOX_OBJECT_SESSION_TOKEN_ENV; credential env-name defaults point at DEVBOX_R2_ACCESS_KEY_ID, DEVBOX_R2_SECRET_ACCESS_KEY, DEVBOX_R2_SESSION_TOKEN");
                return Ok(());
            }
            value => return Err(format!("unknown option '{value}'").into()),
        }
        index += 1;
    }

    let db_path = db_path.ok_or("--db <SQLITE_PATH> is required")?;
    let addr: SocketAddr = listen.parse()?;
    if config.session_ttl_seconds <= 0 || config.proof_ttl_seconds <= 0 {
        return Err("session and proof TTLs must be positive".into());
    }
    serve_sqlite_with_config(&db_path, addr, config).await?;
    Ok(())
}

fn object_access_broker_config_from_env(
) -> Result<ManagedObjectAccessBrokerConfig, Box<dyn std::error::Error>> {
    if let Ok(root) = std::env::var("DEVBOX_OBJECT_LOCAL_ROOT") {
        if !root.trim().is_empty() {
            return Ok(ManagedObjectAccessBrokerConfig::server_managed_local(root)?);
        }
    }

    let access_key_env = std::env::var("DEVBOX_OBJECT_ACCESS_KEY_ENV")
        .unwrap_or_else(|_| "DEVBOX_R2_ACCESS_KEY_ID".to_string());
    let secret_key_env = std::env::var("DEVBOX_OBJECT_SECRET_KEY_ENV")
        .unwrap_or_else(|_| "DEVBOX_R2_SECRET_ACCESS_KEY".to_string());
    let session_token_env = std::env::var("DEVBOX_OBJECT_SESSION_TOKEN_ENV")
        .ok()
        .or_else(|| {
            std::env::var("DEVBOX_R2_SESSION_TOKEN")
                .ok()
                .map(|_| "DEVBOX_R2_SESSION_TOKEN".to_string())
        });

    if env_value_present(&access_key_env) && env_value_present(&secret_key_env) {
        Ok(ManagedObjectAccessBrokerConfig::server_managed_env(
            access_key_env,
            secret_key_env,
            session_token_env,
        )?)
    } else {
        Ok(ManagedObjectAccessBrokerConfig::disabled())
    }
}

fn env_value_present(name: &str) -> bool {
    std::env::var(name)
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

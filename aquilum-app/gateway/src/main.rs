use aquilum_gateway::{api, auth, config, upstream};

use anyhow::{Context, Result, bail};
use std::{path::PathBuf, sync::Arc};
use tracing_subscriber::{Layer, layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    // Do not enable dependency diagnostics that could include upstream payloads.
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_filter(tracing_subscriber::filter::filter_fn(|meta| {
                    meta.target().starts_with("aquilum_gateway")
                })),
        )
        .init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("serve") => {
            let path = args
                .get(1)
                .map(PathBuf::from)
                .unwrap_or_else(|| "config.json".into());
            let config = config::Config::load(&path)?;
            anyhow::ensure!(config.enabled, "Client API is disabled");
            auth::load_tokens(&config.tokens_file)
                .context("Create at least one client token before starting")?;
            let address = config.bind;
            let backend = Arc::new(upstream::Aquilum::new(
                config.aquilum_settings.clone(),
                config.workspace.clone(),
            ));
            let state = Arc::new(api::AppState::new(config, backend));
            let listener = tokio::net::TcpListener::bind(address)
                .await
                .context("Cannot bind gateway address")?;
            tracing::info!(%address, "Aquilum gateway listening (read-only)");
            axum::serve(listener, api::router(state))
                .with_graceful_shutdown(async {
                    let _ = tokio::signal::ctrl_c().await;
                })
                .await?;
        }
        Some("token-create") if args.len() == 4 => {
            auth::create_token(&PathBuf::from(&args[1]), &args[2], &PathBuf::from(&args[3]))?;
            println!("Client token created. Read it from the output file; keep it private.");
        }
        Some("token-revoke") if args.len() == 3 => {
            auth::revoke_token(&PathBuf::from(&args[1]), &args[2])?;
            println!("Client token revoked; new requests are rejected immediately.");
        }
        _ => bail!(
            "Usage:\n  aquilum-gateway serve [config.json]\n  aquilum-gateway token-create <tokens.json> <client-name> <new-private-output-file>\n  aquilum-gateway token-revoke <tokens.json> <client-name>"
        ),
    }
    Ok(())
}

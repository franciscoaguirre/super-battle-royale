use clap::Parser;
use matchbox_signaling::{
    SignalingServerBuilder,
    topologies::full_mesh::{FullMesh, FullMeshState},
};
use std::net::SocketAddr;
use tracing::info;

#[derive(Parser, Debug)]
#[clap(name = "sbr-signaling")]
struct Args {
    #[clap(default_value = "0.0.0.0:3536")]
    host: SocketAddr,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "sbr_signaling=info,tower_http=debug".into()),
        )
        .init();

    let args = Args::parse();
    info!("Starting signaling server on {}", args.host);

    SignalingServerBuilder::new(args.host, FullMesh, FullMeshState::default())
        .cors()
        .trace()
        .build()
        .serve()
        .await?;

    Ok(())
}

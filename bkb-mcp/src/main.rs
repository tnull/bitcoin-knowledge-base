mod remote_store;
mod tools;

use anyhow::Result;
use clap::Parser;
use tracing::info;

#[derive(Parser)]
#[command(name = "bkb-mcp", about = "BKB MCP server for AI agent access")]
struct Cli {
	/// URL of the BKB HTTP API server.
	#[arg(long, default_value = "http://127.0.0.1:3000", env = "BKB_API_URL")]
	api_url: String,
}

#[tokio::main]
async fn main() -> Result<()> {
	tracing_subscriber::fmt()
		.with_env_filter(
			tracing_subscriber::EnvFilter::try_from_default_env()
				.unwrap_or_else(|_| "info,bkb_mcp=debug".into()),
		)
		.with_writer(std::io::stderr)
		.init();

	let cli = Cli::parse();
	info!(api_url = %cli.api_url, "starting BKB MCP server");

	let store = remote_store::RemoteApiStore::new(&cli.api_url);
	tools::run_stdio_server(store).await
}

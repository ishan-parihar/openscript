//! OpenScript MCP Server — stdio transport for AI agent control.
//!
//! This binary starts the MCP server that listens on stdin/stdout using
//! JSON-RPC 2.0 with Content-Length framing (MCP spec). It exposes tools
//! for the entire video editing pipeline: transcription, timeline editing,
//! TTS, asset search, FFmpeg rendering, and more.
//!
//! Usage:
//!   cargo run -p openscript-mcp --bin mcp-server
//!   # or after build:
//!   ./target/release/mcp-server

use openscript_mcp::server;

#[tokio::main]
async fn main() -> Result<(), std::io::Error> {
    // Initialize tracing to stderr so it doesn't interfere with stdout JSON-RPC
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    tracing::info!("openscript-mcp server starting (stdio transport)");

    server::run().await?;

    tracing::info!("openscript-mcp server shutting down");
    Ok(())
}

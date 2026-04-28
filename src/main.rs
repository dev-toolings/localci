use anyhow::Result;
use localci::cli;

#[tokio::main]
async fn main() -> Result<()> {
    cli::run().await
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    tracing::info!("aristide-server {}", env!("CARGO_PKG_VERSION"));
    Ok(())
}

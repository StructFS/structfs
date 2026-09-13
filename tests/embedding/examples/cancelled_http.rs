//! Run with `cargo run --manifest-path tests/embedding/Cargo.toml --example cancelled_http`.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    featherweight_external_embedding_check::lifecycle::cancelled_http_demo().await?;
    println!("Disconnect, late reply, aliased handle, joined cleanup, capacity retention, and nonzero exit verified.");
    Ok(())
}

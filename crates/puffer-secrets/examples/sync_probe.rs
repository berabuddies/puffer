//! Dev probe for browser credential sync — NOT a production command.
//!
//! Lists which browser sources are available on this machine and (optionally)
//! runs a real import for one source into a throwaway temp vault, printing the
//! report plus the stored secret *metadata* (labels/usernames/origins only —
//! never the decrypted values).
//!
//! Usage:
//!   cargo run -p puffer-secrets --example sync_probe                # list sources
//!   cargo run -p puffer-secrets --example sync_probe -- firefox     # import probe
//!
//! Intended for testing the Firefox / Chromium decryptors on each OS (e.g. a
//! Windows VM) without building the full daemon.

use puffer_secrets::{available_browser_sources, BrowserSource, SecretVault};

fn main() -> anyhow::Result<()> {
    println!("Available browser sources:");
    for source in available_browser_sources() {
        println!(
            "  {:<8} {:<8} available={}",
            source.id, source.label, source.available
        );
    }

    let Some(id) = std::env::args().nth(1) else {
        println!("\nUsage: sync_probe <chrome|edge|brave|firefox>");
        return Ok(());
    };
    let source = BrowserSource::from_id(&id)
        .ok_or_else(|| anyhow::anyhow!("unknown source `{id}` (chrome|edge|brave|firefox)"))?;

    let dir = tempfile::tempdir()?;
    let vault = SecretVault::open_with_key(dir.path().join("secrets.json"), [0u8; 32]);
    let report = vault.sync_browser_source(source)?;

    println!(
        "\n{} import: imported={} skipped={} errors={}",
        source.label(),
        report.imported,
        report.skipped,
        report.errors.len()
    );
    for error in &report.errors {
        println!("  error: {error}");
    }

    println!("\nStored secret metadata (no values shown):");
    for secret in vault.list()? {
        println!(
            "  [{}] {} | user={:?} origin={:?}",
            secret.source, secret.label, secret.username, secret.origin
        );
    }
    Ok(())
}

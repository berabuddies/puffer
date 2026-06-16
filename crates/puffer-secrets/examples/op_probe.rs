//! Dev probe for the 1Password integration — exercises the real `op` CLI.
//!
//! Needs `op` on PATH and OP_SERVICE_ACCOUNT_TOKEN set in the environment.
//! Lists Login items as op:// references (the import path) and resolves each
//! (the resolve-on-demand path), printing only metadata + value LENGTHS — never
//! the secret values themselves.
//!
//! Usage: OP_SERVICE_ACCOUNT_TOKEN=ops_... op_probe

fn main() -> anyhow::Result<()> {
    println!("op_cli_available = {}", puffer_secrets::op_cli_available());
    let token_set = std::env::var("OP_SERVICE_ACCOUNT_TOKEN").is_ok();
    println!("OP_SERVICE_ACCOUNT_TOKEN set = {token_set}");
    if !token_set {
        println!("(set the token to run the live list/resolve test)");
        return Ok(());
    }

    let logins = puffer_secrets::onepassword::import_login_references()?;
    println!("imported {} login reference(s) via `op item list`", logins.len());
    for login in &logins {
        match puffer_secrets::resolve_op_reference(&login.reference) {
            Ok(value) => println!(
                "  RESOLVE_OK {} | label={:?} origin={:?} value_len={}",
                login.reference,
                login.label,
                login.origin,
                value.len()
            ),
            Err(error) => println!("  RESOLVE_FAIL {} | {error}", login.reference),
        }
    }
    Ok(())
}

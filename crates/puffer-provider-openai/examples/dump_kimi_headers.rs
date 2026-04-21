use puffer_provider_openai::kimi_oauth::{kimi_common_headers, KIMI_USER_AGENT, KIMI_CLI_VERSION};
fn main() {
    println!("KIMI_CLI_VERSION = {KIMI_CLI_VERSION}");
    println!("KIMI_USER_AGENT  = {KIMI_USER_AGENT}");
    println!("---");
    let h = kimi_common_headers().expect("headers");
    for (k, v) in &h {
        println!("{k}: {v}");
    }
}

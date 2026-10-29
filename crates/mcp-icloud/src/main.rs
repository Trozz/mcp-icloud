use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    mcp_icloud::server().main().await
}

use mcp_icloud_mail::VERSION;
use mcp_icloud_mail::app::{HttpSettings, mcp_router};
use mcp_icloud_mail::cache::{LruMessageCache, MessageCache};
use mcp_icloud_mail::config::MailConfig;
use mcp_icloud_mail::imap::{ImapClient, ImapSettings};
use mcp_icloud_mail::mail::build::Identity;
use mcp_icloud_mail::smtp::{Sender, SmtpSender};
use mcp_icloud_mail::tools::{MailServer, ToolSettings};
use mcp_server_core::config::EnvReader;
use mcp_server_core::oauth::AuthState;
use mcp_server_core::server::build_app;
use mcp_server_core::{install_crypto_provider, observability, shutdown_signal, tls};
use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::Arc;

const USAGE: &str = "usage: mcp-icloud-mail [--version | --healthcheck]\nAll configuration is read from environment variables; see README.md.";

#[tokio::main]
async fn main() -> ExitCode {
    let arg = std::env::args().nth(1);
    match arg.as_deref() {
        Some("--version" | "-V") => {
            println!("mcp-icloud-mail {VERSION}");
            return ExitCode::SUCCESS;
        }
        Some("--healthcheck") => return healthcheck().await,
        Some("--help" | "-h") => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Some(other) => {
            eprintln!("unknown argument: {other}\n{USAGE}");
            return ExitCode::from(2);
        }
        None => {}
    }
    install_crypto_provider();
    let mut env = EnvReader::from_process();
    let Some(cfg) = MailConfig::from_env(&mut env) else {
        for e in &env.errors {
            eprintln!("configuration error: {e}");
        }
        return ExitCode::FAILURE;
    };
    match run(cfg).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = %e, "fatal");
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// Container HEALTHCHECK: GET /healthz on 127.0.0.1 at the BIND_ADDR port.
async fn healthcheck() -> ExitCode {
    install_crypto_provider();
    let mut env = EnvReader::from_process();
    let addr = env.parse("BIND_ADDR", SocketAddr::from(([0, 0, 0, 0], 8443)));
    let insecure = env.bool("INSECURE_HTTP", false);
    if tls::healthcheck(addr.port(), !insecure).await {
        return ExitCode::SUCCESS;
    }
    eprintln!("healthcheck failed");
    ExitCode::FAILURE
}

/// Drops any user directive for async_imap and caps it at info: its trace output contains the
/// LOGIN command, password included.
fn log_filter(level: &str) -> String {
    let mut parts: Vec<&str> = level
        .split(',')
        .map(str::trim)
        .filter(|d| !d.is_empty() && !d.starts_with("async_imap"))
        .collect();
    parts.push("async_imap=info");
    parts.join(",")
}

async fn run(cfg: MailConfig) -> anyhow::Result<()> {
    let mut obs = cfg.core.observability.clone();
    obs.log_level = log_filter(&obs.log_level);
    let _guard = observability::init(&obs)?;
    tracing::info!(version = VERSION, "starting mcp-icloud-mail");

    let cache: Arc<dyn MessageCache> = Arc::new(LruMessageCache::new(cfg.cache_max_bytes));
    let imap = Arc::new(ImapClient::new(
        ImapSettings::icloud(
            cfg.username.clone(),
            cfg.app_password.clone(),
            cfg.imap_idle_timeout,
        ),
        cache,
    ));
    let sender: Option<Arc<dyn Sender>> = if cfg.allow_send {
        Some(Arc::new(SmtpSender::icloud(
            &cfg.username,
            &cfg.app_password,
        )?))
    } else {
        None
    };
    let settings = Arc::new(ToolSettings {
        identity: Identity {
            email: cfg.email.clone(),
            name: cfg.from_name.clone(),
        },
        max_attachment_bytes: cfg.max_attachment_bytes,
        send_lock: Default::default(),
    });
    let tools = MailServer::new(imap.clone(), sender.clone(), settings.clone()).tool_names();
    tracing::info!(send_enabled = cfg.allow_send, tools = ?tools, "tools registered");

    let shutdown = shutdown_signal();
    let core = cfg.core;
    let http = HttpSettings {
        public_url: core.public_url.clone(),
        public_host: core.public_host.clone(),
        allowed_origins: core.allowed_origins.clone(),
    };
    let mcp_url = core.mcp_url();
    let mcp = mcp_router(&http, imap, sender, settings, shutdown.clone());
    let app = build_app(AuthState::new(core.oauth), mcp);
    tracing::info!(addr = %core.bind_addr, mcp_url, "listening");
    tls::serve(app, core.bind_addr, core.tls, shutdown).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn async_imap_is_always_capped_at_info() {
        assert_eq!(log_filter("info"), "info,async_imap=info");
        assert_eq!(log_filter("trace"), "trace,async_imap=info");
        assert_eq!(log_filter(""), "async_imap=info");
        assert_eq!(
            log_filter("debug,async_imap=trace,async_imap::client=trace"),
            "debug,async_imap=info"
        );
    }
}

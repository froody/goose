use crate::config::Config;
use crate::subprocess::configure_subprocess;
use anyhow::{Context, Result};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::net::TcpStream;
use std::time::Duration;
use tokio::process::{Child, Command};
use tracing::{info, warn};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HeadroomConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_auto_start")]
    pub auto_start: bool,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_command")]
    pub command: String,
}

fn default_auto_start() -> bool {
    true
}

fn default_port() -> u16 {
    8787
}

fn default_command() -> String {
    "headroom".to_string()
}

impl Default for HeadroomConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            auto_start: default_auto_start(),
            port: default_port(),
            command: default_command(),
        }
    }
}

pub struct HeadroomSidecar {
    _child: Option<Child>,
    pub port: u16,
}

static SIDECAR_INSTANCE: Lazy<tokio::sync::Mutex<Option<HeadroomSidecar>>> =
    Lazy::new(|| tokio::sync::Mutex::new(None));

/// Get the headroom configuration.
pub fn get_config() -> HeadroomConfig {
    Config::global()
        .get_param::<HeadroomConfig>("headroom")
        .unwrap_or_default()
}

/// Check if the headroom proxy is active.
pub fn is_active() -> bool {
    let config = get_config();
    if !config.enabled {
        return false;
    }
    // If enabled, check if the proxy is running (responding on its port)
    TcpStream::connect(format!("127.0.0.1:{}", config.port)).is_ok()
}

/// Starts the headroom proxy sidecar process if configured and not already running.
pub async fn start_proxy_if_needed() -> Result<()> {
    let config = get_config();
    if !config.enabled {
        return Ok(());
    }

    let mut instance = SIDECAR_INSTANCE.lock().await;
    if instance.is_some() {
        return Ok(());
    }

    // Check if something is already listening on the port
    if TcpStream::connect(format!("127.0.0.1:{}", config.port)).is_ok() {
        info!("Headroom proxy is already running on port {}", config.port);
        // We still track it as active (without owning a child process handle)
        *instance = Some(HeadroomSidecar {
            _child: None,
            port: config.port,
        });
        return Ok(());
    }

    if !config.auto_start {
        info!(
            "Headroom is enabled but auto_start is disabled. Expecting external proxy on port {}",
            config.port
        );
        return Ok(());
    }

    info!(
        "Starting headroom proxy sidecar on port {} using command '{}'",
        config.port, config.command
    );
    let mut cmd = Command::new(&config.command);
    cmd.arg("proxy")
        .arg("--port")
        .arg(config.port.to_string())
        .kill_on_drop(true);

    configure_subprocess(&mut cmd);

    match cmd.spawn() {
        Ok(child) => {
            // Give it a brief moment to bind to the port
            tokio::time::sleep(Duration::from_millis(500)).await;
            if TcpStream::connect(format!("127.0.0.1:{}", config.port)).is_ok() {
                info!(
                    "Headroom proxy successfully started on port {}",
                    config.port
                );
                *instance = Some(HeadroomSidecar {
                    _child: Some(child),
                    port: config.port,
                });
            } else {
                warn!(
                    "Headroom proxy process spawned but is not responding on port {}",
                    config.port
                );
                *instance = Some(HeadroomSidecar {
                    _child: Some(child),
                    port: config.port,
                });
            }
        }
        Err(e) => {
            warn!("Failed to start headroom proxy sidecar: {}. Make sure 'headroom' is installed on your PATH or configure 'headroom.command' in config.yaml.", e);
            return Err(e).context("failed to start headroom proxy");
        }
    }

    Ok(())
}

/// Returns the rewritten URL if headroom is active.
pub fn rewrite_url_if_needed(original_url: &str) -> String {
    let config = get_config();
    if !config.enabled {
        return original_url.to_string();
    }

    // Only rewrite standard API endpoints that headroom proxy supports
    let is_supported = original_url.contains("api.openai.com")
        || original_url.contains("api.anthropic.com")
        || original_url.contains("api.google")
        || original_url.contains("localhost:8787") // already pointing to headroom
        || original_url.contains("127.0.0.1:8787");

    if !is_supported {
        return original_url.to_string();
    }

    // Check if proxy is running
    if !is_active() {
        return original_url.to_string();
    }

    // Extract path after host to append to our rewritten base
    if let Ok(parsed) = url::Url::parse(original_url) {
        let path_and_query = parsed.path();
        let query = parsed.query();

        let base = format!("http://127.0.0.1:{}", config.port);
        let mut rewritten = if path_and_query.starts_with("/v1") {
            format!("{}{}", base, path_and_query)
        } else if original_url.contains("api.anthropic.com") {
            // Anthropic doesn't include /v1 in some of its base configs, but headroom serves on /v1/messages
            format!("{}{}", base, path_and_query)
        } else {
            // For other endpoints, standard fallback
            format!("{}{}", base, path_and_query)
        };

        if let Some(q) = query {
            rewritten.push('?');
            rewritten.push_str(q);
        }
        rewritten
    } else {
        original_url.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = HeadroomConfig::default();
        assert!(!config.enabled);
        assert!(config.auto_start);
        assert_eq!(config.port, 8787);
        assert_eq!(config.command, "headroom");
    }

    #[test]
    fn test_rewrite_url_disabled() {
        // By default, headroom is disabled
        let original = "https://api.openai.com/v1/chat/completions";
        let rewritten = rewrite_url_if_needed(original);
        assert_eq!(rewritten, original);
    }

    #[test]
    fn test_rewrite_url_unsupported() {
        let original = "https://example.com/api";
        let rewritten = rewrite_url_if_needed(original);
        assert_eq!(rewritten, original);
    }
}


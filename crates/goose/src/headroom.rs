use crate::config::paths::Paths;
use crate::config::Config;
use crate::subprocess::configure_subprocess;
use anyhow::{Context, Result};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::net::TcpStream;
use std::process::Stdio;
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
    #[serde(default)]
    pub custom_hosts: Vec<String>,
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
            custom_hosts: Vec::new(),
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

/// Helper to construct the command and arguments for running headroom,
/// automatically falling back to uvx if the headroom binary is not available.
pub fn get_command_and_args(subcommand_args: &[String]) -> (String, Vec<String>) {
    let config = get_config();

    // If a custom command is configured (not the default "headroom"), or if headroom is available in PATH,
    // use it directly.
    if config.command != "headroom" || which::which("headroom").is_ok() {
        return (config.command.clone(), subcommand_args.to_vec());
    }

    // Check if uvx is available
    if which::which("uvx").is_ok() {
        let mut args = vec![
            "--python".to_string(),
            "python3.13".to_string(),
            "--from".to_string(),
            "headroom-ai[proxy,mcp]".to_string(),
            "headroom".to_string(),
        ];
        args.extend(subcommand_args.iter().cloned());
        return ("uvx".to_string(), args);
    }

    // Check if uv is available
    if which::which("uv").is_ok() {
        let mut args = vec![
            "tool".to_string(),
            "run".to_string(),
            "--python".to_string(),
            "python3.13".to_string(),
            "--from".to_string(),
            "headroom-ai[proxy,mcp]".to_string(),
            "headroom".to_string(),
        ];
        args.extend(subcommand_args.iter().cloned());
        return ("uv".to_string(), args);
    }

    // Fallback to config.command if neither is found
    (config.command.clone(), subcommand_args.to_vec())
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

    let subcommand_args = vec![
        "proxy".to_string(),
        "--port".to_string(),
        config.port.to_string(),
    ];
    let (cmd_name, cmd_args) = get_command_and_args(&subcommand_args);

    info!(
        "Starting headroom proxy sidecar on port {} using command '{}' and args {:?}",
        config.port, cmd_name, cmd_args
    );
    let mut cmd = Command::new(&cmd_name);
    cmd.args(&cmd_args).kill_on_drop(true);

    configure_subprocess(&mut cmd);

    // Redirect stdout and stderr to a logfile to prevent them from outputting directly to the terminal
    let log_dir = Paths::in_state_dir("logs");
    std::fs::create_dir_all(&log_dir).context("failed to create log directory for headroom")?;
    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir.join("headroom.log"))
        .context("failed to open headroom log file")?;

    cmd.stdout(Stdio::from(
        log_file
            .try_clone()
            .context("failed to clone headroom log file handle")?,
    ));
    cmd.stderr(Stdio::from(log_file));

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

    // Only rewrite standard API endpoints that headroom proxy supports, or custom configured hosts
    let is_supported = original_url.contains("api.openai.com")
        || original_url.contains("api.anthropic.com")
        || original_url.contains("api.google")
        || original_url.contains("googleapis.com")
        || original_url.contains("localhost:8787") // already pointing to headroom
        || original_url.contains("127.0.0.1:8787")
        || config.custom_hosts.iter().any(|h| original_url.contains(h));

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
        assert!(config.custom_hosts.is_empty());
    }

    #[test]
    fn test_parse_custom_hosts() {
        let yaml = "
enabled: true
auto_start: true
port: 8787
command: headroom
custom_hosts:
  - nadirclaw
  - custom-proxy.internal
";
        let config: HeadroomConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.enabled);
        assert_eq!(config.custom_hosts.len(), 2);
        assert_eq!(config.custom_hosts[0], "nadirclaw");
        assert_eq!(config.custom_hosts[1], "custom-proxy.internal");
    }

    #[test]
    fn test_rewrite_url_disabled() {
        let original = "https://api.openai.com/v1/chat/completions";
        let rewritten = rewrite_url_if_needed(original);
        if get_config().enabled && is_active() {
            let port = get_config().port;
            assert_eq!(
                rewritten,
                format!("http://127.0.0.1:{}/v1/chat/completions", port)
            );
        } else {
            assert_eq!(rewritten, original);
        }
    }

    #[test]
    fn test_rewrite_url_google() {
        let original = "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:generateContent";
        let rewritten = rewrite_url_if_needed(original);
        if get_config().enabled && is_active() {
            let port = get_config().port;
            assert_eq!(
                rewritten,
                format!(
                    "http://127.0.0.1:{}/v1beta/models/gemini-2.5-flash:generateContent",
                    port
                )
            );
        } else {
            assert_eq!(rewritten, original);
        }
    }

    #[test]
    fn test_rewrite_url_unsupported() {
        let original = "https://example.com/api";
        let rewritten = rewrite_url_if_needed(original);
        assert_eq!(rewritten, original);
    }
}

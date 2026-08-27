use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const CONFIG_NAME: &str = ".safeshell.toml";
pub const MAX_TIMEOUT_SECONDS: u64 = 600;
pub const MAX_OUTPUT_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_APPROVAL_TIMEOUT_SECONDS: u64 = 3600;
pub const MAX_APPROVAL_TTL_SECONDS: u64 = 3600;
pub const MAX_DEDUP_SECONDS: u64 = 3600;
pub const MAX_TRANSFER_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    pub version: u8,
    pub project_id: Uuid,
    #[serde(default)]
    pub limits: Limits,
    #[serde(default)]
    pub servers: BTreeMap<String, Server>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
    #[serde(default = "default_output")]
    pub max_output_bytes: usize,
    /// How long the broker waits for a human decision before denying the request.
    #[serde(default = "default_approval_timeout")]
    pub approval_timeout_seconds: u64,
    /// Upper bound on executed commands per rolling hour, across all servers.
    #[serde(default = "default_commands_per_hour")]
    pub max_commands_per_hour: u32,
    /// Window in which an identical command is rejected instead of run again.
    #[serde(default = "default_dedup")]
    pub dedup_seconds: u64,
    /// How long a manual approval keeps covering the exact same command.
    #[serde(default = "default_approval_ttl")]
    pub approval_ttl_seconds: u64,
    /// Largest file `get_file` and `put_file` will move in one request.
    #[serde(default = "default_transfer_bytes")]
    pub max_transfer_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            timeout_seconds: default_timeout(),
            max_output_bytes: default_output(),
            approval_timeout_seconds: default_approval_timeout(),
            max_commands_per_hour: default_commands_per_hour(),
            dedup_seconds: default_dedup(),
            approval_ttl_seconds: default_approval_ttl(),
            max_transfer_bytes: default_transfer_bytes(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Server {
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub username: String,
    pub auth: Auth,
    #[serde(default)]
    pub autoapprove: AutoApprove,
}

/// Command patterns that may skip the manual prompt. `deny` always wins.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutoApprove {
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Auth {
    Password { credential_id: Uuid },
    SshAgent,
}

impl Auth {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Password { .. } => "password",
            Self::SshAgent => "ssh-agent",
        }
    }
}

pub struct Project {
    pub root: PathBuf,
    pub path: PathBuf,
    pub config: ProjectConfig,
}

pub fn discover(start: &Path) -> Result<Project> {
    for directory in start.ancestors() {
        let path = directory.join(CONFIG_NAME);
        if path.is_file() {
            let config = load(&path)?;
            return Ok(Project {
                root: directory.to_path_buf(),
                path,
                config,
            });
        }
    }
    bail!("no {CONFIG_NAME} found in this directory or its parents; run `safeshell init`")
}

pub fn load(path: &Path) -> Result<ProjectConfig> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let config: ProjectConfig = toml::from_str(&raw).with_context(|| {
        format!(
            "invalid config {}; plaintext secret fields are forbidden",
            path.display()
        )
    })?;
    validate(&config)?;
    Ok(config)
}

pub fn save(path: &Path, config: &ProjectConfig) -> Result<()> {
    validate(config)?;
    let raw = toml::to_string_pretty(config)?;
    atomic_write(path, raw.as_bytes())
}

pub fn init_project(root: &Path) -> Result<()> {
    let path = root.join(CONFIG_NAME);
    if path.exists() {
        bail!("{} already exists", path.display());
    }
    let config = ProjectConfig {
        version: 1,
        project_id: Uuid::new_v4(),
        limits: Limits::default(),
        servers: BTreeMap::new(),
    };
    save(&path, &config)?;
    add_git_exclude(root)?;
    println!("Created {}", path.display());
    Ok(())
}

pub fn validate_alias(alias: &str) -> Result<()> {
    if alias.is_empty()
        || alias.len() > 64
        || !alias
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        bail!("alias must be 1-64 ASCII letters, digits, '.', '_' or '-'");
    }
    Ok(())
}

fn validate(config: &ProjectConfig) -> Result<()> {
    if config.version != 1 {
        bail!("unsupported config version {}", config.version);
    }
    if !(1..=MAX_TIMEOUT_SECONDS).contains(&config.limits.timeout_seconds) {
        bail!("timeout_seconds must be between 1 and {MAX_TIMEOUT_SECONDS}");
    }
    if !(1024..=MAX_OUTPUT_BYTES).contains(&config.limits.max_output_bytes) {
        bail!("max_output_bytes must be between 1024 and {MAX_OUTPUT_BYTES}");
    }
    if !(1..=MAX_APPROVAL_TIMEOUT_SECONDS).contains(&config.limits.approval_timeout_seconds) {
        bail!("approval_timeout_seconds must be between 1 and {MAX_APPROVAL_TIMEOUT_SECONDS}");
    }
    if config.limits.max_commands_per_hour == 0 {
        bail!("max_commands_per_hour must be at least 1");
    }
    if config.limits.approval_ttl_seconds > MAX_APPROVAL_TTL_SECONDS {
        bail!("approval_ttl_seconds must be at most {MAX_APPROVAL_TTL_SECONDS}");
    }
    if config.limits.dedup_seconds > MAX_DEDUP_SECONDS {
        bail!("dedup_seconds must be at most {MAX_DEDUP_SECONDS}");
    }
    if !(1..=MAX_TRANSFER_BYTES).contains(&config.limits.max_transfer_bytes) {
        bail!("max_transfer_bytes must be between 1 and {MAX_TRANSFER_BYTES}");
    }
    for (alias, server) in &config.servers {
        validate_alias(alias)?;
        if server.host.trim().is_empty() || server.username.trim().is_empty() {
            bail!("server {alias} has an empty host or username");
        }
        if server.port == 0 {
            bail!("server {alias} has invalid port 0");
        }
        if server
            .autoapprove
            .allow
            .iter()
            .chain(&server.autoapprove.deny)
            .any(|pattern| pattern.trim().is_empty())
        {
            bail!("server {alias} has an empty autoapprove pattern");
        }
        if server
            .autoapprove
            .allow
            .iter()
            .any(|pattern| pattern == "*")
        {
            bail!("server {alias} cannot allow the '*' pattern; list explicit commands");
        }
    }
    Ok(())
}

fn add_git_exclude(root: &Path) -> Result<()> {
    let exclude = root.join(".git/info/exclude");
    if !exclude.exists() {
        return Ok(());
    }
    let current = fs::read_to_string(&exclude).unwrap_or_default();
    if current.lines().any(|line| line.trim() == CONFIG_NAME) {
        return Ok(());
    }
    let mut next = current;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(CONFIG_NAME);
    next.push('\n');
    atomic_write(&exclude, next.as_bytes())
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("path has no parent")?;
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp).inspect_err(|_| {
        let _ = fs::remove_file(&temp);
    })?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temp, path).inspect_err(|_| {
        let _ = fs::remove_file(&temp);
    })?;
    Ok(())
}

const fn default_timeout() -> u64 {
    60
}
const fn default_output() -> usize {
    1024 * 1024
}
const fn default_port() -> u16 {
    22
}
const fn default_approval_timeout() -> u64 {
    120
}
const fn default_commands_per_hour() -> u32 {
    60
}
const fn default_dedup() -> u64 {
    10
}
const fn default_approval_ttl() -> u64 {
    300
}
const fn default_transfer_bytes() -> usize {
    1024 * 1024
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_plaintext_secret_fields() {
        let raw = r#"
version = 1
project_id = "00000000-0000-0000-0000-000000000001"
[servers.prod]
host = "example.com"
username = "root"
password = "never"
[servers.prod.auth]
type = "ssh-agent"
"#;
        assert!(toml::from_str::<ProjectConfig>(raw).is_err());
    }

    #[test]
    fn validates_bounds_and_aliases() {
        assert!(validate_alias("prod.eu-1").is_ok());
        assert!(validate_alias("bad alias").is_err());
        let mut cfg = ProjectConfig {
            version: 1,
            project_id: Uuid::nil(),
            limits: Limits::default(),
            servers: BTreeMap::new(),
        };
        cfg.limits.timeout_seconds = 0;
        assert!(validate(&cfg).is_err());
    }

    #[test]
    fn rejects_blanket_autoapprove_and_out_of_range_windows() {
        let mut cfg = ProjectConfig {
            version: 1,
            project_id: Uuid::nil(),
            limits: Limits::default(),
            servers: BTreeMap::new(),
        };
        cfg.servers.insert(
            "prod".into(),
            Server {
                host: "example.com".into(),
                port: 22,
                username: "deploy".into(),
                auth: Auth::SshAgent,
                autoapprove: AutoApprove {
                    allow: vec!["*".into()],
                    deny: Vec::new(),
                },
            },
        );
        assert!(validate(&cfg).is_err());
        cfg.servers.get_mut("prod").unwrap().autoapprove.allow = vec!["docker logs *".into()];
        assert!(validate(&cfg).is_ok());
        cfg.limits.dedup_seconds = MAX_DEDUP_SECONDS + 1;
        assert!(validate(&cfg).is_err());
    }
}

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
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            timeout_seconds: default_timeout(),
            max_output_bytes: default_output(),
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
    for (alias, server) in &config.servers {
        validate_alias(alias)?;
        if server.host.trim().is_empty() || server.username.trim().is_empty() {
            bail!("server {alias} has an empty host or username");
        }
        if server.port == 0 {
            bail!("server {alias} has invalid port 0");
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
}

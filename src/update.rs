use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

const INSTALLER_URL: &str = "https://raw.githubusercontent.com/suiflex/SafeHell/develop/install.sh";

pub fn run(version: Option<&str>) -> Result<()> {
    let executable = std::env::current_exe().context("cannot determine installed safehell path")?;
    let install_dir = executable
        .parent()
        .context("installed safehell has no parent directory")?;

    let installer = Command::new("curl")
        .args(["-fsSL", INSTALLER_URL])
        .output()
        .context("cannot download the SafeHell installer; is curl installed?")?;
    if !installer.status.success() {
        bail!("could not download the SafeHell installer");
    }

    let mut command = Command::new("sh");
    command
        .stdin(Stdio::piped())
        .env("SAFEHELL_INSTALL_DIR", install_dir);
    if let Some(version) = version {
        if version.is_empty() || version.contains('/') || version.contains('\\') {
            bail!("version must be a non-empty release tag");
        }
        command.env("SAFEHELL_VERSION", version);
    }

    let mut child = command
        .spawn()
        .context("cannot start the SafeHell installer")?;
    child
        .stdin
        .take()
        .context("cannot open installer input")?
        .write_all(&installer.stdout)
        .context("cannot pass installer to shell")?;
    let status = child
        .wait()
        .context("SafeHell installer failed to finish")?;
    if !status.success() {
        bail!("SafeHell update failed");
    }
    Ok(())
}

use std::io::Write;

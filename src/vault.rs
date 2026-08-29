use std::collections::BTreeMap;
use std::fs;

use age::secrecy::ExposeSecret;
use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::config;

const SERVICE: &str = "dev.safehell.master";
const ACCOUNT: &str = "default";

#[derive(Serialize, Deserialize, Default)]
struct VaultData {
    version: u8,
    credentials: BTreeMap<Uuid, Credential>,
}

#[derive(Serialize, Deserialize, Clone, Zeroize, ZeroizeOnDrop)]
pub struct Credential {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
}

pub fn setup() -> Result<()> {
    let path = vault_path()?;
    let entry = entry()?;
    let key_exists = match entry.get_password() {
        Ok(secret) => {
            drop(Zeroizing::new(secret));
            true
        }
        Err(keyring::Error::NoEntry) => false,
        Err(error) => return Err(error).context("cannot read the OS credential store"),
    };
    let vault_exists = path.exists();
    match (key_exists, vault_exists) {
        (true, true) => bail!("SafeHell is already set up"),
        (true, false) | (false, true) => {
            bail!("vault/key mismatch; restore the missing component before continuing")
        }
        (false, false) => {}
    }
    let identity = age::x25519::Identity::generate();
    entry
        .set_password(identity.to_string().expose_secret())
        .context("cannot save master identity in the OS credential store")?;
    if let Err(error) = save_with_identity(
        &identity,
        &VaultData {
            version: 1,
            ..Default::default()
        },
    ) {
        let _ = entry.delete_credential();
        return Err(error);
    }
    println!("Created encrypted vault at {}", path.display());
    Ok(())
}

pub fn add_credential(host: &str, port: u16, username: &str, password: &str) -> Result<Uuid> {
    let identity = identity()?;
    let mut data = load_with_identity(&identity)?;
    let id = Uuid::new_v4();
    data.credentials.insert(
        id,
        Credential {
            host: host.to_owned(),
            port,
            username: username.to_owned(),
            password: password.to_owned(),
        },
    );
    save_with_identity(&identity, &data)?;
    Ok(id)
}

pub fn remove_credential(id: Uuid) -> Result<()> {
    let identity = identity()?;
    let mut data = load_with_identity(&identity)?;
    data.credentials
        .remove(&id)
        .context("credential not found in vault")?;
    save_with_identity(&identity, &data)
}

pub fn credential(id: Uuid, host: &str, port: u16, username: &str) -> Result<Credential> {
    let identity = identity()?;
    let data = load_with_identity(&identity)?;
    let credential = data
        .credentials
        .get(&id)
        .context("credential not found in vault")?;
    if credential.host != host || credential.port != port || credential.username != username {
        bail!("credential binding mismatch; refusing to use it for a different endpoint")
    }
    Ok(credential.clone())
}

fn entry() -> Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, ACCOUNT).context("cannot open OS credential store")
}

fn identity() -> Result<age::x25519::Identity> {
    let encoded = Zeroizing::new(
        entry()?
            .get_password()
            .context("SafeHell master identity is unavailable; run `safehell setup`")?,
    );
    encoded
        .parse()
        .map_err(|_| anyhow::anyhow!("stored SafeHell master identity is invalid"))
}

#[cfg(windows)]
pub fn socket_token() -> Result<String> {
    use sha2::{Digest, Sha256};

    let encoded = Zeroizing::new(
        entry()?
            .get_password()
            .context("SafeHell master identity is unavailable; run `safehell setup`")?,
    );
    let digest = Sha256::digest(encoded.as_bytes());
    Ok(digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn data_dir() -> Result<std::path::PathBuf> {
    let dirs = ProjectDirs::from("dev", "SafeHell", "SafeHell")
        .context("cannot determine user data directory")?;
    let path = dirs.data_local_dir().to_path_buf();
    fs::create_dir_all(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(path)
}

pub fn vault_path() -> Result<std::path::PathBuf> {
    Ok(data_dir()?.join("vault.age"))
}

pub fn known_hosts_path() -> Result<std::path::PathBuf> {
    Ok(data_dir()?.join("known_hosts"))
}

pub fn audit_path() -> Result<std::path::PathBuf> {
    Ok(data_dir()?.join("audit.jsonl"))
}

#[cfg(unix)]
pub fn socket_path() -> Result<std::path::PathBuf> {
    Ok(data_dir()?.join("broker.sock"))
}

fn load_with_identity(identity: &age::x25519::Identity) -> Result<VaultData> {
    let ciphertext =
        fs::read(vault_path()?).context("cannot read encrypted vault; run `safehell setup`")?;
    let plaintext =
        Zeroizing::new(age::decrypt(identity, &ciphertext).context("cannot decrypt vault")?);
    let data: VaultData = serde_json::from_slice(&plaintext).context("invalid vault contents")?;
    if data.version != 1 {
        bail!("unsupported vault version {}", data.version);
    }
    Ok(data)
}

fn save_with_identity(identity: &age::x25519::Identity, data: &VaultData) -> Result<()> {
    let plaintext = Zeroizing::new(serde_json::to_vec(data)?);
    let ciphertext =
        age::encrypt(&identity.to_public(), &plaintext).context("cannot encrypt vault")?;
    config::atomic_write(&vault_path()?, &ciphertext)
}

#[cfg(test)]
mod tests {
    #[test]
    fn age_round_trip_and_tamper_detection() {
        let identity = age::x25519::Identity::generate();
        let ciphertext = age::encrypt(&identity.to_public(), b"sentinel").unwrap();
        assert_eq!(age::decrypt(&identity, &ciphertext).unwrap(), b"sentinel");
        let mut tampered = ciphertext;
        let last = tampered.len() - 1;
        tampered[last] ^= 1;
        assert!(age::decrypt(&identity, &tampered).is_err());
    }
}

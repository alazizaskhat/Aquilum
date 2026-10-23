use anyhow::{Context, Result, bail, ensure};
use rand::{RngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{File, OpenOptions},
    io::Write,
    path::Path,
};
use subtle::ConstantTimeEq;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TokenFile {
    pub clients: Vec<Client>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Client {
    pub name: String,
    pub sha256: String,
    pub enabled: bool,
}

pub fn load_tokens(path: &Path) -> Result<TokenFile> {
    let bytes = std::fs::read(path).context("Cannot read client token file")?;
    ensure!(bytes.len() <= 32_768, "Client token file is too large");
    let file: TokenFile = serde_json::from_slice(&bytes).context("Invalid client token file")?;
    ensure!(
        !file.clients.is_empty() && file.clients.len() <= 64,
        "Expected 1–64 clients"
    );
    let mut names = HashSet::new();
    let mut hashes = HashSet::new();
    for client in &file.clients {
        validate_name(&client.name)?;
        ensure!(
            names.insert(&client.name) && hashes.insert(&client.sha256),
            "Duplicate client"
        );
        ensure!(
            hex::decode(&client.sha256).is_ok_and(|value| value.len() == 32),
            "Invalid client token hash"
        );
    }
    Ok(file)
}

pub fn authenticate(file: &TokenFile, token: &str) -> Option<String> {
    if token.len() != 68
        || !token.starts_with("aqg_")
        || !token[4..].bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    let candidate = Sha256::digest(token.as_bytes());
    let mut name = None;
    for client in &file.clients {
        if let Ok(expected) = hex::decode(&client.sha256) {
            let equal = candidate.as_slice().ct_eq(&expected).unwrap_u8() == 1;
            if equal && client.enabled {
                name = Some(client.name.clone());
            }
        }
    }
    name
}

pub fn create_token(path: &Path, name: &str, output: &Path) -> Result<()> {
    validate_name(name)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _lock = token_lock(path)?;
    let mut file = if path.exists() {
        load_tokens(path)?
    } else {
        TokenFile { clients: vec![] }
    };
    ensure!(file.clients.len() < 64, "Maximum number of clients reached");
    ensure!(
        !file.clients.iter().any(|client| client.name == name),
        "Client name already exists; use a new name for rotation"
    );
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let token = format!("aqg_{}", hex::encode(bytes));
    // Create an empty private file before writing the credential. Never print it.
    let mut credential = private_new_file(output)
        .context("Cannot create private output file (it must not already exist)")?;
    credential.write_all(token.as_bytes())?;
    credential.sync_all()?;
    file.clients.push(Client {
        name: name.into(),
        sha256: hex::encode(Sha256::digest(token.as_bytes())),
        enabled: true,
    });
    if let Err(error) = save_tokens(path, &file) {
        drop(credential);
        let _ = std::fs::remove_file(output);
        return Err(error);
    }
    Ok(())
}

pub fn revoke_token(path: &Path, name: &str) -> Result<()> {
    let _lock = token_lock(path)?;
    let mut file = load_tokens(path)?;
    let Some(client) = file.clients.iter_mut().find(|client| client.name == name) else {
        bail!("Client not found")
    };
    client.enabled = false;
    save_tokens(path, &file)
}

fn validate_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name.len() <= 64
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte)),
        "Client names must be 1–64 ASCII letters, numbers, dots, underscores or hyphens"
    );
    Ok(())
}

fn token_lock(path: &Path) -> Result<File> {
    let lock = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path.with_extension("lock"))?;
    lock.try_lock()
        .context("Another token management command is running")?;
    Ok(lock)
}

fn save_tokens(path: &Path, file: &TokenFile) -> Result<()> {
    let temp = path.with_extension(format!("{}.tmp", rand::random::<u64>()));
    let result = (|| {
        let mut out = private_new_file(&temp)?;
        serde_json::to_writer_pretty(&mut out, file)?;
        out.sync_all()?;
        drop(out);
        std::fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

fn private_new_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    #[cfg(windows)]
    {
        use std::{
            os::windows::process::CommandExt,
            process::{Command, Stdio},
        };
        const HIDDEN: u32 = 0x08000000;
        let identity = Command::new("whoami.exe").creation_flags(HIDDEN).output()?;
        ensure!(
            identity.status.success(),
            "Cannot determine Windows account"
        );
        let account = String::from_utf8(identity.stdout)?.trim().to_owned();
        let status = Command::new("icacls.exe")
            .arg(path)
            .args([
                "/inheritance:r",
                "/grant:r",
                &format!("{account}:(F)"),
                "*S-1-5-18:(F)",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(HIDDEN)
            .status()?;
        if !status.success() {
            drop(file);
            let _ = std::fs::remove_file(path);
            bail!("Cannot restrict credential file permissions");
        }
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_wrong_and_revoked_tokens() {
        let token = format!("aqg_{}", "ab".repeat(32));
        let mut file = TokenFile {
            clients: vec![Client {
                name: "phone".into(),
                sha256: hex::encode(Sha256::digest(token.as_bytes())),
                enabled: true,
            }],
        };
        assert_eq!(authenticate(&file, &token).as_deref(), Some("phone"));
        assert!(authenticate(&file, &format!("aqg_{}", "cd".repeat(32))).is_none());
        assert!(authenticate(&file, "short").is_none());
        file.clients[0].enabled = false;
        assert!(authenticate(&file, &token).is_none());
    }

    #[test]
    fn token_creation_and_revocation_never_store_raw_token_in_registry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tokens.json");
        let output = dir.path().join("phone.token");
        create_token(&path, "phone", &output).unwrap();
        let token = std::fs::read_to_string(&output).unwrap();
        assert!(!std::fs::read_to_string(&path).unwrap().contains(&token));
        assert_eq!(
            authenticate(&load_tokens(&path).unwrap(), &token).as_deref(),
            Some("phone")
        );
        assert!(create_token(&path, "phone", &dir.path().join("duplicate.token")).is_err());
        revoke_token(&path, "phone").unwrap();
        assert!(authenticate(&load_tokens(&path).unwrap(), &token).is_none());
    }
}

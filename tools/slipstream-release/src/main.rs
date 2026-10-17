//! slipstream-release: signs a station's update manifest (used by
//! `scripts/build-station.sh`).
//!
//! ```sh
//! slipstream-release pubkey      # release public key; creates the key on first use
//! slipstream-release manifest DIST/clients --version 0.5.1 --build 15 --commit 1a2b3c4
//! ```
//!
//! The private key lives in `$SLIPSTREAM_SIGNING_KEY`, or by default in
//! `~/.config/slipstream-release/update-signing.key` (outside the project,
//! so it's never uploaded or synced with the code). Apps built with its
//! public key install only updates signed by it; see
//! `slipstream_core::release`.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use iroh::SecretKey;
use sha2::{Digest, Sha256};
use slipstream_core::{
    directory::{hex, unhex},
    release::SignedUpdate,
};

/// (manifest platform, download page `os`, file in clients/)
const PLATFORMS: &[(&str, &str, &str)] = &[
    ("linux-x86_64", "linux", "slipstream-linux-x86_64"),
    ("windows-x86_64", "windows", "slipstream-windows-x86_64.exe"),
    // One universal app for both kinds of Mac.
    ("macos-aarch64", "macos", "slipstream-macos.zip"),
    ("macos-x86_64", "macos", "slipstream-macos.zip"),
];

#[derive(Parser)]
#[command(about = "Signs Slipstream station builds")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Print the release public key (hex), creating the key pair on first use.
    Pubkey,
    /// Write a signed manifest.json for the apps in a station's clients/ folder.
    Manifest {
        clients: PathBuf,
        #[arg(long)]
        version: String,
        #[arg(long)]
        build: u64,
        #[arg(long)]
        commit: String,
        /// Platforms whose apps are code-signed (e.g. windows-x86_64).
        /// Stations serve those untouched, so the signature stays valid.
        #[arg(long, value_delimiter = ',')]
        signed: Vec<String>,
    },
}

fn key_path() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("SLIPSTREAM_SIGNING_KEY") {
        return Ok(p.into());
    }
    let dirs = directories::BaseDirs::new().context("no home directory")?;
    Ok(dirs.config_dir().join("slipstream-release").join("update-signing.key"))
}

fn load_key(path: &Path) -> Result<SecretKey> {
    let text = fs::read_to_string(path).with_context(|| format!("reading the signing key {}", path.display()))?;
    let bytes: [u8; 32] = unhex(text.trim())
        .and_then(|b| b.try_into().ok())
        .with_context(|| format!("{} is not a signing key (expected 64 hex digits)", path.display()))?;
    Ok(SecretKey::from_bytes(&bytes))
}

fn create_key(path: &Path) -> Result<SecretKey> {
    let key = SecretKey::generate();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, format!("{}\n", hex(&key.to_bytes())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
    }
    fs::rename(&tmp, path)?;
    eprintln!();
    eprintln!("*** Created a new update signing key: {}", path.display());
    eprintln!("*** Back it up somewhere safe and private. Apps built from now on only");
    eprintln!("*** install updates signed with it: lose it, and every installed app has");
    eprintln!("*** to be downloaded again by hand.");
    eprintln!();
    Ok(key)
}

fn sha256_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(hex(&Sha256::digest(&bytes)))
}

fn manifest(key: &SecretKey, clients: &Path, version: &str, build: u64, commit: &str, signed: &[String]) -> Result<()> {
    let mut platforms = serde_json::Map::new();
    for &(platform, os, file) in PLATFORMS {
        let path = clients.join(file);
        if !path.is_file() {
            continue;
        }
        let size = fs::metadata(&path)?.len();
        let sha256 = sha256_file(&path)?;
        let sig = SignedUpdate { platform, version, build, size, sha256: &sha256 }.sign(key);
        platforms.insert(
            platform.into(),
            serde_json::json!({
                "os": os,
                "file": file,
                "size": size,
                "sha256": sha256,
                "sig": sig,
                "signed": signed.iter().any(|p| p == platform),
            }),
        );
    }
    if platforms.is_empty() {
        bail!("no apps in {}", clients.display());
    }
    let built = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs();
    let m = serde_json::json!({
        "version": version,
        "build": build,
        "commit": commit,
        "built": built,
        "signer": key.public().to_string(),
        "platforms": platforms,
    });
    let out = clients.join("manifest.json");
    let tmp = clients.join("manifest.json.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(&m)? + "\n")?;
    fs::rename(&tmp, &out)?;
    Ok(())
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Pubkey => {
            let path = key_path()?;
            let key = if path.exists() { load_key(&path)? } else { create_key(&path)? };
            println!("{}", key.public());
        }
        Cmd::Manifest { clients, version, build, commit, signed } => {
            manifest(&load_key(&key_path()?)?, &clients, &version, build, &commit, &signed)?
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the tool writes is what the app accepts (the station passes the
    /// entry through as `update_check`).
    #[test]
    fn manifest_entries_verify() {
        let dir = std::env::temp_dir().join(format!("slipstream-release-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("slipstream-linux-x86_64"), b"linux app").unwrap();
        fs::write(dir.join("slipstream-windows-x86_64.exe"), b"windows app").unwrap();
        let key = SecretKey::generate();
        manifest(&key, &dir, "0.5.1", 15, "abc1234", &["windows-x86_64".into()]).unwrap();

        let m: serde_json::Value = serde_json::from_slice(&fs::read(dir.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(m["platforms"].as_object().unwrap().len(), 2);
        assert_eq!(m["platforms"]["windows-x86_64"]["signed"], true);
        assert_eq!(m["platforms"]["linux-x86_64"]["signed"], false);
        for (platform, e) in m["platforms"].as_object().unwrap() {
            let u = SignedUpdate {
                platform,
                version: m["version"].as_str().unwrap(),
                build: m["build"].as_u64().unwrap(),
                size: e["size"].as_u64().unwrap(),
                sha256: e["sha256"].as_str().unwrap(),
            };
            u.verify(&[key.public()], e["sig"].as_str().unwrap()).unwrap();
            assert!(u.verify(&[SecretKey::generate().public()], e["sig"].as_str().unwrap()).is_err());
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn key_file_roundtrip() {
        let path = std::env::temp_dir().join(format!("slipstream-release-key-{}", std::process::id())).join("k.key");
        let key = create_key(&path).unwrap();
        assert_eq!(load_key(&path).unwrap().public(), key.public());
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}

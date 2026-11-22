//! `cargo xtask release-assets <version> <installers> <out> <owner/repo>`: the
//! publish job's last step before `gh release create`. From the build jobs'
//! artifacts (`<installers>/ircord-gpui-<platform>/`) it fills `<out>` with:
//!
//! - the installers under their release names,
//! - `build-info.txt`, after checking every build job built the same commit,
//! - a `.sig` beside the Windows setup and the macOS `.app.tar.gz`, signed
//!   with the key in `CARGO_PACKAGER_SIGN_PRIVATE_KEY` (password in
//!   `CARGO_PACKAGER_SIGN_PRIVATE_KEY_PASSWORD`) and checked against
//!   `packaging/updater.pub`, the key the app trusts,
//! - `latest.json`, the manifest the updater reads, and `SHA256SUMS`.
//!
//! The signatures are the ones cargo-packager makes (a base64 minisign
//! signature whose trusted comment names the file), so the app checks them
//! with cargo-packager-updater.

use crate::package::{LINUX_DEB, MACOS_DMG, MACOS_UPDATE, WINDOWS_SETUP};
use anyhow::{Context, Result, bail, ensure};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::io::Cursor;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Each build job's artifact and the release assets it holds.
const ARTIFACTS: [(&str, &[&str]); 3] = [
    ("ircord-gpui-linux-x64", &[LINUX_DEB]),
    ("ircord-gpui-windows-x64", &[WINDOWS_SETUP]),
    ("ircord-gpui-macos-arm64", &[MACOS_DMG, MACOS_UPDATE]),
];

/// The updater's entries: manifest key, asset, cargo-packager-updater format.
const UPDATES: [(&str, &str, &str); 2] = [
    ("macos-aarch64", MACOS_UPDATE, "app"),
    ("windows-x86_64", WINDOWS_SETUP, "nsis"),
];

pub fn run(root: &Path, version: &str, installers: &Path, out: &Path, repo: &str) -> Result<()> {
    ensure!(
        is_stable_version(version),
        "{version} is not a release version such as 0.1.0"
    );
    ensure!(
        version == env!("CARGO_PKG_VERSION"),
        "release {version} but Cargo.toml says {}: set the workspace version first",
        env!("CARGO_PKG_VERSION")
    );
    std::fs::create_dir_all(out)?;

    let mut build_info: Option<String> = None;
    for (artifact, assets) in ARTIFACTS {
        let dir = installers.join(artifact);
        let info = std::fs::read_to_string(dir.join("build-info.txt"))
            .with_context(|| format!("read {artifact}/build-info.txt"))?;
        match &build_info {
            None => build_info = Some(info),
            Some(first) if *first == info => {}
            Some(first) => {
                bail!("{artifact} was built from\n{info}\nbut another job from\n{first}")
            }
        }
        for asset in assets {
            std::fs::copy(dir.join(asset), out.join(asset))
                .with_context(|| format!("copy {artifact}/{asset}"))?;
        }
    }
    std::fs::write(
        out.join("build-info.txt"),
        build_info.expect("three artifacts"),
    )?;

    let key = std::env::var("CARGO_PACKAGER_SIGN_PRIVATE_KEY")
        .context("CARGO_PACKAGER_SIGN_PRIVATE_KEY is not set")?;
    let password = std::env::var("CARGO_PACKAGER_SIGN_PRIVATE_KEY_PASSWORD").unwrap_or_default();
    let secret = secret_key(&key, &password)?;
    let public = std::fs::read_to_string(root.join("packaging/updater.pub"))
        .context("read packaging/updater.pub")?;

    let mut signatures = Vec::new();
    for (_, asset, _) in UPDATES {
        let data = std::fs::read(out.join(asset))?;
        let signature = sign(&secret, &data, asset, unix_time())?;
        verify(&public, &data, &signature, asset)
            .with_context(|| format!("{asset} does not verify with packaging/updater.pub"))?;
        std::fs::write(out.join(format!("{asset}.sig")), &signature)?;
        println!("signed {asset}");
        signatures.push(signature);
    }

    let url = format!("https://github.com/{repo}/releases/download/v{version}");
    let manifest = manifest(version, &url, &signatures);
    std::fs::write(
        out.join("latest.json"),
        serde_json::to_string_pretty(&manifest)? + "\n",
    )?;
    std::fs::write(out.join("SHA256SUMS"), checksums(out)?)?;
    println!("release assets for v{version} are in {}", out.display());
    Ok(())
}

fn is_stable_version(version: &str) -> bool {
    let parts: Vec<&str> = version.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.bytes().all(|b| b.is_ascii_digit())
                && (p.len() == 1 || !p.starts_with('0'))
        })
}

/// The key as cargo-packager writes it: base64 of a minisign secret key file.
fn secret_key(key: &str, password: &str) -> Result<minisign::SecretKey> {
    let text = String::from_utf8(
        STANDARD
            .decode(key.trim())
            .context("the key is not base64")?,
    )?;
    minisign::SecretKeyBox::from_string(&text)?
        .into_secret_key(Some(password.to_string()))
        .context("decrypt the signing key (is the password right?)")
}

/// A signature as cargo-packager makes it: base64 of the minisign signature
/// file, with the file's name in the trusted comment.
fn sign(key: &minisign::SecretKey, data: &[u8], name: &str, time: u64) -> Result<String> {
    let trusted = format!("timestamp:{time}\tfile:{name}");
    let signature = minisign::sign(
        None,
        key,
        Cursor::new(data),
        Some(&trusted),
        Some("signature from cargo-packager secret key"),
    )?;
    Ok(STANDARD.encode(signature.to_string()))
}

/// Checks `signature` as the updater does, and that it was made for `name`.
fn verify(public: &str, data: &[u8], signature: &str, name: &str) -> Result<()> {
    let public = String::from_utf8(STANDARD.decode(public.trim())?)?;
    let public = minisign::PublicKeyBox::from_string(&public)?.into_public_key()?;
    let signature = String::from_utf8(STANDARD.decode(signature)?)?;
    let signature = minisign::SignatureBox::from_string(&signature)?;
    minisign::verify(&public, &signature, Cursor::new(data), true, false, false)?;
    let trusted = signature.trusted_comment()?;
    ensure!(
        trusted
            .split('\t')
            .any(|field| field == format!("file:{name}")),
        "the signature was made for another file ({trusted})"
    );
    Ok(())
}

fn manifest(version: &str, url: &str, signatures: &[String]) -> serde_json::Value {
    let mut platforms = serde_json::Map::new();
    for ((key, asset, format), signature) in UPDATES.iter().zip(signatures) {
        platforms.insert(
            key.to_string(),
            json!({ "url": format!("{url}/{asset}"), "signature": signature, "format": format }),
        );
    }
    json!({ "version": version, "platforms": platforms })
}

/// `sha256sum`'s format, for every release asset in `dir`.
fn checksums(dir: &Path) -> Result<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)?
        .map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect::<Result<_, _>>()?;
    names.retain(|n| n.starts_with("IRCord-GPUI-"));
    names.sort();
    let mut sums = String::new();
    for name in names {
        let digest = Sha256::digest(std::fs::read(dir.join(&name))?);
        sums.push_str(&format!("{digest:x}  {name}\n"));
    }
    Ok(sums)
}

fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway key pair in cargo-packager's encoding (no password, so the
    /// test skips the slow key derivation).
    fn key_pair() -> (String, String) {
        let pair = minisign::KeyPair::generate_encrypted_keypair(Some(String::new())).unwrap();
        (
            STANDARD.encode(pair.sk.to_box(None).unwrap().to_string()),
            STANDARD.encode(pair.pk.to_box().unwrap().to_string()),
        )
    }

    #[test]
    fn a_signature_verifies_for_its_own_file_only() {
        let (secret, public) = key_pair();
        let key = secret_key(&secret, "").unwrap();
        let signature = sign(&key, b"setup", WINDOWS_SETUP, 1).unwrap();
        verify(&public, b"setup", &signature, WINDOWS_SETUP).unwrap();
        assert!(verify(&public, b"other bytes", &signature, WINDOWS_SETUP).is_err());
        assert!(verify(&public, b"setup", &signature, MACOS_UPDATE).is_err());
        let (_, stranger) = key_pair();
        assert!(verify(&stranger, b"setup", &signature, WINDOWS_SETUP).is_err());
    }

    #[test]
    fn the_signature_is_what_cargo_packager_writes() {
        let (secret, _) = key_pair();
        let key = secret_key(&secret, "").unwrap();
        let signature = sign(&key, b"setup", WINDOWS_SETUP, 42).unwrap();
        let text = String::from_utf8(STANDARD.decode(signature).unwrap()).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines[0],
            "untrusted comment: signature from cargo-packager secret key"
        );
        assert_eq!(
            lines[2],
            format!("trusted comment: timestamp:42\tfile:{WINDOWS_SETUP}")
        );
    }

    /// The app hands this file to cargo-packager-updater as it is, and the
    /// updater base64-decodes it strictly: plain minisign text, or a trailing
    /// newline, would make every update fail to verify.
    #[test]
    fn the_committed_public_key_is_in_the_updaters_encoding() {
        let file = include_str!("../../packaging/updater.pub");
        let text = String::from_utf8(STANDARD.decode(file).expect("strict base64")).unwrap();
        minisign::PublicKeyBox::from_string(&text)
            .unwrap()
            .into_public_key()
            .unwrap();
    }

    #[test]
    fn a_key_that_is_not_base64_is_refused() {
        assert!(secret_key("not base64!", "").is_err());
    }

    #[test]
    fn the_manifest_has_the_updater_keys_and_formats() {
        let url = "https://github.com/ircord/frontend-gpui/releases/download/v1.2.3";
        let m = manifest("1.2.3", url, &["mac".into(), "win".into()]);
        assert_eq!(m["version"], "1.2.3");
        assert_eq!(m["platforms"]["macos-aarch64"]["format"], "app");
        assert_eq!(m["platforms"]["macos-aarch64"]["signature"], "mac");
        assert_eq!(
            m["platforms"]["macos-aarch64"]["url"],
            format!("{url}/{MACOS_UPDATE}")
        );
        assert_eq!(m["platforms"]["windows-x86_64"]["format"], "nsis");
        assert_eq!(
            m["platforms"]["windows-x86_64"]["url"],
            format!("{url}/{WINDOWS_SETUP}")
        );
        assert_eq!(m["platforms"].as_object().unwrap().len(), 2);
    }

    #[test]
    fn checksums_cover_the_assets_in_sha256sum_format() {
        let dir = std::env::temp_dir().join(format!("xtask-sums-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("IRCord-GPUI-b.deb"), b"").unwrap();
        std::fs::write(dir.join("IRCord-GPUI-a.exe"), b"abc").unwrap();
        std::fs::write(dir.join("latest.json"), b"{}").unwrap();
        let sums = checksums(&dir).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            sums,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  IRCord-GPUI-a.exe\n\
             e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  IRCord-GPUI-b.deb\n"
        );
    }

    #[test]
    fn only_plain_release_versions_pass() {
        for ok in ["0.1.0", "1.20.3", "10.0.0"] {
            assert!(is_stable_version(ok), "{ok}");
        }
        for bad in ["0.1", "v0.1.0", "0.1.0-rc.1", "01.0.0", "0..1", "a.b.c"] {
            assert!(!is_stable_version(bad), "{bad}");
        }
    }
}

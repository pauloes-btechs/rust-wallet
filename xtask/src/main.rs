//! Repo automation: `cargo xtask <task>`.
//!
//! * `fetch-sidecars [--triple <tauri-triple> | --all]`: download the pinned
//!   litd release asset, verify its SHA-256 against `sidecars.lock.json`
//!   (hashes copied from the signed upstream release manifest), and only then
//!   extract `litd` to `apps/desktop/src-tauri/binaries/litd-<triple>[.exe]`.
//!   A mismatch is fatal: nothing is extracted or bundled.
//! * `print-triple`: the Tauri target triple of this machine.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pkg {
    repo: String,
    tag: String,
    asset_prefix: String,
    sha256: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct Lock {
    litd: Pkg,
}

struct Platform {
    key: &'static str,
    triple: &'static str,
    asset_os: &'static str,
    exe: bool,
}

const PLATFORMS: &[Platform] = &[
    Platform { key: "linux-x64", triple: "x86_64-unknown-linux-gnu", asset_os: "linux-amd64", exe: false },
    Platform { key: "linux-arm64", triple: "aarch64-unknown-linux-gnu", asset_os: "linux-arm64", exe: false },
    Platform { key: "darwin-x64", triple: "x86_64-apple-darwin", asset_os: "darwin-amd64", exe: false },
    Platform { key: "darwin-arm64", triple: "aarch64-apple-darwin", asset_os: "darwin-arm64", exe: false },
    Platform { key: "win32-x64", triple: "x86_64-pc-windows-msvc", asset_os: "windows-amd64", exe: true },
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("xtask lives in the repo").to_path_buf()
}

fn host_triple() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        (os, arch) => panic!("no pinned sidecar for {os}-{arch}"),
    }
}

fn fetch(lock: &Lock, p: &Platform) -> Result<()> {
    let pkg = &lock.litd;
    let ext = if p.exe { ".zip" } else { ".tar.gz" };
    let asset = format!("{}-{}-{}{ext}", pkg.asset_prefix, p.asset_os, pkg.tag);
    let expected = pkg.sha256.get(p.key).with_context(|| format!("no pinned sha256 for {}", p.key))?;
    let url = format!("https://github.com/{}/releases/download/{}/{asset}", pkg.repo, pkg.tag);
    println!("[{}] downloading {url}", p.key);
    let mut body = Vec::new();
    // OS trust store (keychain on macOS), so corporate/proxy CAs work too.
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .tls_config(ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build())
        .build()
        .into();
    agent.get(&url).call()?.into_body().into_reader().read_to_end(&mut body)?;
    let actual = hex::encode(Sha256::digest(&body));
    if &actual != expected {
        bail!("checksum mismatch for {asset}: expected {expected}, got {actual}. Refusing to bundle.");
    }
    println!("[{}] sha256 verified: {actual}", p.key);

    let member = format!("{}-{}-{}/litd{}", pkg.asset_prefix, p.asset_os, pkg.tag, if p.exe { ".exe" } else { "" });
    let bin = if p.exe { from_zip(&body, &member)? } else { from_targz(&body, &member)? };
    let out_dir = root().join("apps/desktop/src-tauri/binaries");
    std::fs::create_dir_all(&out_dir)?;
    let target = out_dir.join(format!("litd-{}{}", p.triple, if p.exe { ".exe" } else { "" }));
    std::fs::write(&target, bin)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755))?;
    }
    println!("[{}] wrote {}", p.key, target.display());
    Ok(())
}

fn from_targz(body: &[u8], member: &str) -> Result<Vec<u8>> {
    let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(body));
    for e in ar.entries()? {
        let mut e = e?;
        if e.path()?.to_string_lossy().trim_start_matches("./") == member {
            let mut v = Vec::new();
            e.read_to_end(&mut v)?;
            return Ok(v);
        }
    }
    bail!("archive does not contain {member}")
}

fn from_zip(body: &[u8], member: &str) -> Result<Vec<u8>> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(body))?;
    let mut f = z.by_name(member).with_context(|| format!("zip does not contain {member}"))?;
    let mut v = Vec::new();
    f.read_to_end(&mut v)?;
    Ok(v)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("fetch-sidecars") => {
            let lock: Lock = serde_json::from_slice(&std::fs::read(root().join("sidecars.lock.json"))?)?;
            let targets: Vec<&Platform> = if args.iter().any(|a| a == "--all") {
                PLATFORMS.iter().collect()
            } else {
                let triple = args
                    .iter()
                    .position(|a| a == "--triple")
                    .and_then(|i| args.get(i + 1).cloned())
                    .unwrap_or_else(|| host_triple().to_string());
                vec![PLATFORMS
                    .iter()
                    .find(|p| p.triple == triple)
                    .with_context(|| format!("unknown triple {triple}"))?]
            };
            for p in targets {
                fetch(&lock, p)?;
            }
            Ok(())
        }
        Some("print-triple") => {
            println!("{}", host_triple());
            Ok(())
        }
        _ => {
            eprintln!("usage: cargo xtask fetch-sidecars [--triple <triple> | --all]\n       cargo xtask print-triple");
            std::process::exit(2)
        }
    }
}

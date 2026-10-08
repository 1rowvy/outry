//! `routy update`: скачивает свежий CLI из GitHub Releases, проверяет sha256
//! и подменяет текущий бинарь. Тот же архив, что ставит install.sh.

use std::io::Read;
use std::path::Path;

use anyhow::{Context, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};

const LATEST_URL: &str = "https://api.github.com/repos/1rowvy/routy/releases/latest";
const TARGET: &str = env!("ROUTY_TARGET");
pub(crate) const CURRENT: &str = env!("CARGO_PKG_VERSION");

#[derive(Deserialize)]
pub(crate) struct Release {
    pub tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    /// `sha256:<hex>` — GitHub считает сам для каждого загруженного файла.
    digest: Option<String>,
}

pub(crate) fn client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(concat!("routy/", env!("CARGO_PKG_VERSION")))
        .build()
}

pub(crate) async fn fetch_latest(client: &reqwest::Client) -> anyhow::Result<Release> {
    // Переопределяется только для тестов.
    let url = std::env::var("ROUTY_UPDATE_URL").unwrap_or_else(|_| LATEST_URL.into());
    let body = client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?
        .error_for_status()
        .context("не удалось получить последний релиз")?
        .bytes()
        .await?;
    Ok(serde_json::from_slice(&body)?)
}

pub async fn run(check_only: bool) -> anyhow::Result<()> {
    let client = client()?;
    let release = fetch_latest(&client).await?;
    crate::notifier::remember(&release.tag_name);

    let latest = release.tag_name.trim_start_matches('v');
    if !is_newer(latest, CURRENT) {
        println!("routy {CURRENT} — последняя версия");
        return Ok(());
    }
    if check_only {
        println!("доступна версия {latest} (сейчас {CURRENT}): routy update");
        return Ok(());
    }

    if cfg!(windows) {
        bail!(
            "самообновление пока только для Linux и macOS; скачайте архив из релиза {}",
            release.tag_name
        );
    }
    let asset = pick_asset(&release.assets, &release.tag_name, TARGET)
        .with_context(|| format!("в релизе {} нет сборки для {TARGET}", release.tag_name))?;

    let exe = std::env::current_exe()?.canonicalize()?;
    if let Some(manager) = package_manager(&exe) {
        bail!(
            "routy установлен через {manager} ({}) — обновляйте им же",
            exe.display()
        );
    }

    println!("скачиваю {}", asset.name);
    let archive = client
        .get(&asset.browser_download_url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    let expected = expected_sha256(&client, &release.assets, asset).await?;
    let actual = hex(&Sha256::digest(&archive));
    if actual != expected {
        bail!("sha256 не совпадает: ожидали {expected}, получили {actual}");
    }

    let binary = extract_binary(&archive)?;
    let tmp = std::env::temp_dir().join(format!("routy-update-{}", std::process::id()));
    std::fs::write(&tmp, binary)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }
    let replaced = self_replace::self_replace(&tmp);
    let _ = std::fs::remove_file(&tmp);
    replaced.map_err(|e| match e.kind() {
        std::io::ErrorKind::PermissionDenied => anyhow::anyhow!(
            "нет прав на запись в {} — запустите `sudo routy update`",
            exe.parent().unwrap_or(&exe).display()
        ),
        _ => e.into(),
    })?;

    println!("routy обновлён: {CURRENT} → {latest}");
    Ok(())
}

/// Linux-сборки в релизе статические (musl): подходят и для gnu-таргета.
fn pick_asset<'a>(assets: &'a [Asset], tag: &str, target: &str) -> Option<&'a Asset> {
    let musl = target.replace("-linux-gnu", "-linux-musl");
    [target, musl.as_str()]
        .iter()
        .map(|t| format!("routy-cli-{tag}-{t}.tar.gz"))
        .find_map(|name| assets.iter().find(|a| a.name == name))
}

async fn expected_sha256(
    client: &reqwest::Client,
    assets: &[Asset],
    asset: &Asset,
) -> anyhow::Result<String> {
    if let Some(hex) = asset
        .digest
        .as_deref()
        .and_then(|d| d.strip_prefix("sha256:"))
    {
        return Ok(hex.to_ascii_lowercase());
    }
    let sums_name = format!("{}.sha256", asset.name);
    let sums = assets
        .iter()
        .find(|a| a.name == sums_name)
        .context("в релизе нет контрольной суммы архива — обновление отменено")?;
    let text = client
        .get(&sums.browser_download_url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    text.split_whitespace()
        .next()
        .map(str::to_ascii_lowercase)
        .context("пустой файл контрольной суммы")
}

fn extract_binary(archive: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    for entry in tar.entries()? {
        let mut entry = entry?;
        if entry.header().entry_type().is_file()
            && entry.path()?.file_name().is_some_and(|n| n == "routy")
        {
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            return Ok(buf);
        }
    }
    bail!("в архиве нет файла routy")
}

/// Бинарь из системного пакета или чужого менеджера не трогаем — они разъедутся.
pub(crate) fn package_manager(exe: &Path) -> Option<&'static str> {
    let s = exe.to_string_lossy();
    if s.starts_with("/nix/store/") {
        Some("nix")
    } else if s.contains("/Cellar/") || s.contains("/homebrew/") || s.contains("/linuxbrew/") {
        Some("Homebrew")
    } else if s.starts_with("/snap/") {
        Some("snap")
    } else if s.starts_with("/usr/bin/") || s.starts_with("/bin/") {
        Some("системный пакетный менеджер")
    } else {
        None
    }
}

fn parse_version(v: &str) -> Option<(u64, u64, u64, bool)> {
    let (core, pre) = match v.split_once('-') {
        Some((c, _)) => (c, true),
        None => (v, false),
    };
    let mut it = core.split('.').map(|p| p.parse::<u64>().ok());
    let v = (it.next()??, it.next()??, it.next()??);
    // Последний элемент — «это релиз»: 0.2.0 (true) старше 0.2.0-beta (false).
    Some((v.0, v.1, v.2, !pre))
}

pub(crate) fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str) -> Asset {
        Asset {
            name: name.into(),
            browser_download_url: String::new(),
            digest: None,
        }
    }

    #[test]
    fn versions() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(is_newer("0.1.10", "0.1.9"));
        assert!(is_newer("0.2.0", "0.2.0-beta.1"));
        assert!(!is_newer("0.2.0-beta.1", "0.2.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("garbage", "0.1.0"));
    }

    #[test]
    fn picks_musl_for_gnu() {
        let assets = [
            asset("routy-cli-v0.2.0-x86_64-unknown-linux-musl.tar.gz"),
            asset("routy-cli-v0.2.0-aarch64-apple-darwin.tar.gz"),
            asset("Routy_0.2.0_amd64.AppImage"),
        ];
        let pick = |t| pick_asset(&assets, "v0.2.0", t).map(|a| a.name.as_str());
        assert_eq!(
            pick("x86_64-unknown-linux-gnu"),
            Some("routy-cli-v0.2.0-x86_64-unknown-linux-musl.tar.gz")
        );
        assert_eq!(
            pick("aarch64-apple-darwin"),
            Some("routy-cli-v0.2.0-aarch64-apple-darwin.tar.gz")
        );
        assert_eq!(pick("aarch64-unknown-linux-gnu"), None);
    }

    #[test]
    fn package_managers() {
        assert_eq!(
            package_manager(Path::new("/usr/bin/routy")),
            Some("системный пакетный менеджер")
        );
        assert_eq!(
            package_manager(Path::new("/nix/store/abc-routy/bin/routy")),
            Some("nix")
        );
        assert_eq!(package_manager(Path::new("/home/u/.local/bin/routy")), None);
        assert_eq!(package_manager(Path::new("/usr/local/bin/routy")), None);
    }

    #[test]
    fn extracts_from_release_layout() {
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        for (path, data) in [
            ("routy-cli-v0.2.0-x/README.md", &b"readme"[..]),
            ("routy-cli-v0.2.0-x/routy", b"BIN"),
        ] {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o755);
            h.set_cksum();
            tar.append_data(&mut h, path, data).unwrap();
        }
        let gz = tar.into_inner().unwrap().finish().unwrap();
        assert_eq!(extract_binary(&gz).unwrap(), b"BIN");
    }
}

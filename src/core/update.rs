//! Self-update from GitHub Releases (UI-free).
//!
//! Flow: [`check_latest`] asks the GitHub API for the latest release and
//! compares its tag with [`current_version`]; [`download`] fetches this
//! platform's package and `SHA256SUMS`, verifies the checksum, extracts the
//! package into `<data dir>/Rivet/updates/<version>/staging` and returns the
//! staged app; [`install_and_restart`] swaps the installed files for the staged
//! ones and starts the new version (the caller then quits). Old files are kept
//! next to the new ones (`Rivet.app.old-<ver>`, `Rivet.exe.old`, `rivet.old`)
//! because a running executable can be renamed but not always deleted;
//! [`cleanup_leftovers`] removes them on the next start.
//!
//! Dev builds (`-dev` version or running from cargo's `target/`) never update.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::http::{self, CancelToken, MAX_BODY_BYTES, Progress};
use super::types::{Method, ResolvedRequest, ResponseData, Settings};

/// GitHub repository releases are published to.
pub const REPO: &str = "rivet-soft/rivet";
/// GitHub REST API root (tests point [`check_latest_at`] at a local server).
pub const API_BASE: &str = "https://api.github.com";
/// Release asset with `sha256sum` lines for every package.
pub const CHECKSUMS_ASSET: &str = "SHA256SUMS";
/// Backups made by the last install, deleted by [`cleanup_leftovers`].
const LEFTOVERS_FILE: &str = "leftovers.txt";
const CHECK_TIMEOUT_MS: u64 = 30_000;
const DOWNLOAD_TIMEOUT_MS: u64 = 15 * 60_000;

// ---- versions ---------------------------------------------------------------

/// `major.minor.patch[-pre]` (build metadata after `+` is ignored). A
/// pre-release is lower than the same version without one.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Option<String>,
}

impl Version {
    pub fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self { major, minor, patch, pre: None }
    }

    /// Accepts `1.2.3`, `v1.2.3`, `1.2.3-rc.1`, `1.2` (patch 0); `None` otherwise.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let text = text.strip_prefix(['v', 'V']).unwrap_or(text);
        let text = text.split('+').next()?;
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (text, None),
        };
        if pre.is_some_and(|p| p.is_empty() || p.split('.').any(str::is_empty)) {
            return None;
        }
        let mut parts = core.split('.');
        let number = |part: Option<&str>, required: bool| -> Option<u64> {
            match part {
                Some(p) if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) => p.parse().ok(),
                None if !required => Some(0),
                _ => None,
            }
        };
        let major = number(parts.next(), true)?;
        let minor = number(parts.next(), true)?;
        let patch = number(parts.next(), false)?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self { major, minor, patch, pre: pre.map(str::to_string) })
    }

    pub fn is_prerelease(&self) -> bool {
        self.pre.is_some()
    }
}

fn cmp_pre(a: &str, b: &str) -> Ordering {
    let mut left = a.split('.');
    let mut right = b.split('.');
    loop {
        match (left.next(), right.next()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let order = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(x), Ok(y)) => x.cmp(&y),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => x.cmp(y),
                };
                if order != Ordering::Equal {
                    return order;
                }
            }
        }
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (&self.pre, &other.pre) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(a), Some(b)) => cmp_pre(a, b),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(pre) = &self.pre {
            write!(f, "-{pre}")?;
        }
        Ok(())
    }
}

/// The running app's version (`RIVET_VERSION` from build.rs).
pub fn current_version() -> Version {
    Version::parse(env!("RIVET_VERSION")).unwrap_or_else(|| Version::new(0, 0, 0))
}

/// `true` when `exe` is a cargo build output (`…/target/…/debug|release/…`).
pub fn runs_from_cargo_target(exe: &Path) -> bool {
    let parts: Vec<String> = exe
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts
        .iter()
        .position(|p| p == "target")
        .is_some_and(|ix| parts[ix + 1..].iter().any(|p| p == "debug" || p == "release"))
}

/// Updates are disabled for local builds: a `-dev` version or an executable
/// started from cargo's `target/` directory.
pub fn is_dev_build() -> bool {
    env!("RIVET_VERSION").ends_with("-dev")
        || std::env::current_exe().is_ok_and(|exe| runs_from_cargo_target(&exe))
}

// ---- platforms and releases ---------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Windows,
    Linux,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Platform::MacOs
        } else if cfg!(windows) {
            Platform::Windows
        } else {
            Platform::Linux
        }
    }

    /// End of this platform's package name (`Rivet-<ver><suffix>`).
    pub fn asset_suffix(self) -> &'static str {
        match self {
            Platform::MacOs => "-macos-universal.zip",
            Platform::Windows => "-windows-x64.zip",
            Platform::Linux => "-linux-x64.tar.gz",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Platform::MacOs => "macOS",
            Platform::Windows => "Windows",
            Platform::Linux => "Linux",
        }
    }

    /// The executable inside the package (Windows/Linux) or the bundle (macOS).
    fn app_name(self) -> &'static str {
        match self {
            Platform::MacOs => "Rivet.app",
            Platform::Windows => "Rivet.exe",
            Platform::Linux => "rivet",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
    #[serde(default)]
    pub size: u64,
}

/// A published release, with the package for the current platform.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseInfo {
    pub version: Version,
    pub tag: String,
    pub name: String,
    /// Release notes (Markdown).
    pub notes: String,
    pub html_url: String,
    /// ISO 8601, e.g. `2026-10-12T09:30:00Z` (may be empty).
    pub published_at: String,
    pub asset: Asset,
    pub checksums: Option<Asset>,
}

#[derive(Deserialize)]
struct RawRelease {
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    assets: Vec<Asset>,
}

/// The package built for `platform`, by name suffix.
pub fn select_asset(assets: &[Asset], platform: Platform) -> Option<&Asset> {
    assets.iter().find(|a| a.name.ends_with(platform.asset_suffix()))
}

/// Parses a `GET /repos/{repo}/releases/latest` response.
pub fn parse_release(json: &[u8], platform: Platform) -> Result<ReleaseInfo, String> {
    let raw: RawRelease = serde_json::from_slice(json)
        .map_err(|e| format!("Не удалось разобрать ответ GitHub: {e}"))?;
    let version = Version::parse(&raw.tag_name)
        .ok_or_else(|| format!("Непонятная версия релиза: {}", raw.tag_name))?;
    let asset = select_asset(&raw.assets, platform)
        .cloned()
        .ok_or_else(|| format!("В релизе {} нет сборки для {}", raw.tag_name, platform.label()))?;
    let checksums = raw.assets.iter().find(|a| a.name == CHECKSUMS_ASSET).cloned();
    let html_url = if raw.html_url.is_empty() {
        format!("https://github.com/{REPO}/releases/tag/{}", raw.tag_name)
    } else {
        raw.html_url
    };
    Ok(ReleaseInfo {
        name: raw.name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| format!("Rivet {version}")),
        version,
        tag: raw.tag_name,
        notes: raw.body.unwrap_or_default(),
        html_url,
        published_at: raw.published_at.unwrap_or_default(),
        asset,
        checksums,
    })
}

/// `2026-10-12T09:30:00Z` → `12.10.2026` (the input as-is when it isn't ISO).
pub fn format_date(iso: &str) -> String {
    let date = iso.split('T').next().unwrap_or("");
    let parts: Vec<&str> = date.split('-').collect();
    match parts.as_slice() {
        [y, m, d] if y.len() == 4 && m.len() == 2 && d.len() == 2 => format!("{d}.{m}.{y}"),
        _ => iso.to_string(),
    }
}

/// The repository's latest-release page (manual download fallback).
pub fn releases_page() -> String {
    format!("https://github.com/{REPO}/releases/latest")
}

fn user_agent() -> String {
    format!("rivet/{}", env!("RIVET_VERSION"))
}

/// Network settings for updater requests: the user's proxy and HTTP version,
/// but always TLS verification and redirects (downloads redirect to a CDN).
fn net_settings(user: &Settings, timeout_ms: u64) -> Settings {
    Settings {
        proxy: user.proxy.clone(),
        http_version: user.http_version,
        ssl_verify: true,
        follow_redirects: true,
        max_redirects: 10,
        timeout_ms,
        ..Settings::default()
    }
}

fn get(
    url: &str,
    settings: &Settings,
    headers: &[(&str, &str)],
    timeout_ms: u64,
    cancel: &CancelToken,
    progress: Option<Progress>,
) -> Result<ResponseData, String> {
    let mut all = vec![("user-agent".to_string(), user_agent())];
    all.extend(headers.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    let req = ResolvedRequest {
        method: Method::Get,
        url: url.to_string(),
        headers: all,
        body: None,
        missing_vars: Vec::new(),
        omit_default_headers: Vec::new(),
    };
    let settings = net_settings(settings, timeout_ms);
    let result = match progress {
        Some(progress) => http::execute_with_progress(&req, &settings, cancel, progress),
        None => http::execute(&req, &settings, cancel),
    };
    result.map_err(|e| match e {
        super::SendError::Cancelled => "Загрузка отменена".to_string(),
        super::SendError::Timeout { .. } => "Превышено время ожидания ответа".to_string(),
        other => format!("Сетевая ошибка: {other}"),
    })
}

fn header<'a>(response: &'a ResponseData, name: &str) -> Option<&'a str> {
    response
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// Checks GitHub for a release newer than the running version.
pub fn check_latest(settings: &Settings) -> Result<Option<ReleaseInfo>, String> {
    check_latest_at(API_BASE, settings, &current_version(), Platform::current())
}

/// [`check_latest`] against any API root, version and platform (tests).
pub fn check_latest_at(
    api_base: &str,
    settings: &Settings,
    current: &Version,
    platform: Platform,
) -> Result<Option<ReleaseInfo>, String> {
    let url = format!("{}/repos/{REPO}/releases/latest", api_base.trim_end_matches('/'));
    let response = get(
        &url,
        settings,
        &[("accept", "application/vnd.github+json"), ("x-github-api-version", "2022-11-28")],
        CHECK_TIMEOUT_MS,
        &CancelToken::new(),
        None,
    )?;
    match response.status {
        200 => {}
        // No published release yet.
        404 => return Ok(None),
        403 | 429 if header(&response, "x-ratelimit-remaining") == Some("0") || response.status == 429 => {
            return Err("Превышен лимит запросов к GitHub API — попробуйте позже".into());
        }
        status => return Err(format!("GitHub ответил {status}")),
    }
    let release = parse_release(&response.body, platform)?;
    Ok((release.version > *current).then_some(release))
}

// ---- download and verification ------------------------------------------------

/// `<data dir>/Rivet/updates`.
pub fn updates_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("Rivet")
        .join("updates")
}

/// `sha256sum` output (`<hex>  <name>` or `<hex> *<name>`) → name → lowercase hex.
pub fn parse_checksums(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let (hash, name) = line.split_once(char::is_whitespace)?;
            let name = name.trim_start().trim_start_matches('*').trim();
            let name = name.rsplit(['/', '\\']).next().unwrap_or(name);
            (hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()) && !name.is_empty())
                .then(|| (name.to_string(), hash.to_ascii_lowercase()))
        })
        .collect()
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Streams `path` through SHA-256 and compares with `expected` (hex).
pub fn verify_file(path: &Path, expected: &str) -> Result<(), String> {
    let mut file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|e| format!("{}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let actual: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if actual.eq_ignore_ascii_case(expected.trim()) {
        Ok(())
    } else {
        Err(format!(
            "Контрольная сумма не совпадает (ожидалась {}, получена {actual}) — файл удалён",
            expected.trim()
        ))
    }
}

/// Downloads, verifies and extracts `release` into [`updates_dir`]; returns
/// the staged app (see [`locate_app`]).
pub fn download(
    release: &ReleaseInfo,
    settings: &Settings,
    progress: Progress,
    cancel: &CancelToken,
) -> Result<PathBuf, String> {
    download_to(&updates_dir(), release, settings, progress, cancel, Platform::current())
}

/// [`download`] into `root/<version>/` for `platform` (tests).
pub fn download_to(
    root: &Path,
    release: &ReleaseInfo,
    settings: &Settings,
    progress: Progress,
    cancel: &CancelToken,
    platform: Platform,
) -> Result<PathBuf, String> {
    let checksums = release
        .checksums
        .as_ref()
        .ok_or("В релизе нет файла SHA256SUMS — скачайте обновление вручную")?;
    let asset = &release.asset;
    if asset.size > MAX_BODY_BYTES as u64 {
        return Err(format!("Пакет {} слишком большой для автообновления", asset.name));
    }
    let dir = root.join(release.version.to_string());
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;

    let sums = get(
        &checksums.browser_download_url,
        settings,
        &[("accept", "application/octet-stream")],
        CHECK_TIMEOUT_MS,
        cancel,
        None,
    )?;
    if sums.status != 200 {
        return Err(format!("SHA256SUMS: сервер ответил {}", sums.status));
    }
    let expected = parse_checksums(&String::from_utf8_lossy(&sums.body))
        .remove(&asset.name)
        .ok_or_else(|| format!("В SHA256SUMS нет строки для {}", asset.name))?;

    let size = asset.size;
    let report: Progress = std::sync::Arc::new(move |received, total| {
        progress(received, if total > 0 { total } else { size })
    });
    let response = get(
        &asset.browser_download_url,
        settings,
        &[("accept", "application/octet-stream"), ("accept-encoding", "identity")],
        DOWNLOAD_TIMEOUT_MS,
        cancel,
        Some(report),
    )?;
    if cancel.is_cancelled() {
        return Err("Загрузка отменена".into());
    }
    if response.status != 200 {
        return Err(format!("{}: сервер ответил {}", asset.name, response.status));
    }
    let len = response.body.len();
    if len >= MAX_BODY_BYTES || (size > 0 && len as u64 != size) {
        return Err(format!("{} скачан не полностью ({len} из {size} байт)", asset.name));
    }
    let archive = dir.join(&asset.name);
    fs::write(&archive, &response.body).map_err(|e| format!("{}: {e}", archive.display()))?;
    drop(response);
    if let Err(e) = verify_file(&archive, &expected) {
        let _ = fs::remove_file(&archive);
        return Err(e);
    }
    let staging = dir.join("staging");
    let extracted = extract_archive(&archive, &staging);
    let _ = fs::remove_file(&archive);
    extracted?;
    locate_app(&staging, platform)
        .ok_or_else(|| format!("В архиве {} не найдено приложение", asset.name))
}

/// Unpacks a `.zip` or `.tar.gz` into `dest` (created). Entries escaping
/// `dest` are rejected by the archive crates; macOS resource-fork debris
/// (`__MACOSX`, `._*`) is removed.
pub fn extract_archive(archive: &Path, dest: &Path) -> Result<(), String> {
    let name = archive.file_name().and_then(|n| n.to_str()).unwrap_or("").to_ascii_lowercase();
    let _ = fs::remove_dir_all(dest);
    fs::create_dir_all(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    let file = fs::File::open(archive).map_err(|e| format!("{}: {e}", archive.display()))?;
    if name.ends_with(".zip") {
        let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("Повреждённый архив: {e}"))?;
        zip.extract(dest).map_err(|e| format!("Не удалось распаковать архив: {e}"))?;
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
        tar.set_preserve_permissions(true);
        tar.unpack(dest).map_err(|e| format!("Не удалось распаковать архив: {e}"))?;
    } else {
        return Err(format!("Неизвестный формат архива: {name}"));
    }
    remove_resource_forks(dest);
    Ok(())
}

fn remove_resource_forks(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
        if name == "__MACOSX" && is_dir {
            let _ = fs::remove_dir_all(&path);
        } else if name.starts_with("._") && !is_dir {
            let _ = fs::remove_file(&path);
        } else if is_dir {
            remove_resource_forks(&path);
        }
    }
}

/// Finds the app inside an extracted package (up to 3 levels deep): the
/// `Rivet.app` bundle on macOS, the folder holding `Rivet.exe` / `rivet` on
/// Windows / Linux.
pub fn locate_app(staging: &Path, platform: Platform) -> Option<PathBuf> {
    let target = platform.app_name();
    let mut level = vec![staging.to_path_buf()];
    for _ in 0..4 {
        let mut next = Vec::new();
        for dir in &level {
            let Ok(entries) = fs::read_dir(dir) else { continue };
            let mut entries: Vec<_> = entries.flatten().collect();
            entries.sort_by_key(|e| e.file_name());
            for entry in entries {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().into_owned();
                let Ok(kind) = entry.file_type() else { continue };
                match platform {
                    Platform::MacOs if kind.is_dir() && name == target => {
                        if path.join("Contents").join("MacOS").is_dir() {
                            return Some(path);
                        }
                    }
                    Platform::Windows if kind.is_file() && name.eq_ignore_ascii_case(target) => {
                        return Some(dir.clone());
                    }
                    Platform::Linux if kind.is_file() && name == target => {
                        return Some(dir.clone());
                    }
                    _ => {}
                }
                if kind.is_dir() {
                    next.push(path);
                }
            }
        }
        level = next;
    }
    None
}

// ---- installation -------------------------------------------------------------

/// `…/Rivet.app` for `…/Rivet.app/Contents/MacOS/<exe>`.
pub fn bundle_of(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    (macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle.extension()? == "app")
        .then(|| bundle.to_path_buf())
}

/// Run from a mounted disk image or a Gatekeeper-translocated copy: the
/// bundle can't be replaced in place.
pub fn is_translocated(path: &Path) -> bool {
    let text = path.to_string_lossy();
    text.contains("/AppTranslocation/") || text.starts_with("/Volumes/")
}

/// What gets replaced: the `.app` bundle on macOS, the executable's folder elsewhere.
pub fn install_target(platform: Platform, exe: &Path) -> Result<PathBuf, String> {
    match platform {
        Platform::MacOs => {
            let bundle = bundle_of(exe)
                .ok_or("Автообновление работает только для установленного Rivet.app")?;
            if is_translocated(&bundle) {
                return Err("Rivet запущен из образа диска или временной копии macOS. \
                            Перетащите Rivet.app в «Программы», запустите оттуда и повторите"
                    .into());
            }
            Ok(bundle)
        }
        Platform::Windows | Platform::Linux => exe
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| "Не удалось определить папку установки".to_string()),
    }
}

fn ensure_writable(dir: &Path) -> Result<(), String> {
    let probe = dir.join(".rivet-update-probe");
    match fs::write(&probe, b"") {
        Ok(()) => {
            let _ = fs::remove_file(&probe);
            Ok(())
        }
        Err(_) => Err(format!(
            "Нет прав на запись в {} — скачайте новую версию вручную со страницы релиза",
            dir.display()
        )),
    }
}

/// Why the running copy can't update itself (shown before downloading), or `None`.
pub fn install_blocker() -> Option<String> {
    if is_dev_build() {
        return Some("Обновления отключены в dev-сборке".into());
    }
    let exe = std::env::current_exe().ok()?;
    let target = match install_target(Platform::current(), &exe) {
        Ok(target) => target,
        Err(e) => return Some(e),
    };
    let dir = match Platform::current() {
        Platform::MacOs => target.parent().map(Path::to_path_buf).unwrap_or(target),
        _ => target,
    };
    ensure_writable(&dir).err()
}

fn copy_dir_all(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let dst = to.join(entry.file_name());
        if kind.is_dir() {
            copy_dir_all(&entry.path(), &dst)?;
        } else if kind.is_symlink() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(fs::read_link(entry.path())?, &dst)?;
            #[cfg(not(unix))]
            fs::copy(entry.path(), &dst).map(|_| ())?;
        } else {
            fs::copy(entry.path(), &dst)?;
        }
    }
    Ok(())
}

/// `rename`, or copy + delete when `from` and `to` are on different volumes.
fn move_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    if fs::rename(from, to).is_ok() {
        return Ok(());
    }
    copy_dir_all(from, to)?;
    let _ = fs::remove_dir_all(from);
    Ok(())
}

fn leftovers_manifest(updates_root: &Path) -> PathBuf {
    updates_root.parent().unwrap_or(updates_root).join(format!("update-{LEFTOVERS_FILE}"))
}

fn record_leftovers(updates_root: &Path, paths: &[PathBuf]) {
    let manifest = leftovers_manifest(updates_root);
    let mut text = fs::read_to_string(&manifest).unwrap_or_default();
    for path in paths {
        text.push_str(&path.to_string_lossy());
        text.push('\n');
    }
    if let Some(dir) = manifest.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::write(&manifest, text);
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = fs::metadata(path) {
        let mut perms = meta.permissions();
        perms.set_mode(perms.mode() | 0o755);
        let _ = fs::set_permissions(path, perms);
    }
}

#[cfg(not(unix))]
fn make_executable(_: &Path) {}

/// Replaces `bundle` with `new_bundle`, keeping the old one as
/// `<bundle>.old-<current>` next to it. Returns the backup path.
pub fn install_bundle(new_bundle: &Path, bundle: &Path, current: &Version) -> Result<PathBuf, String> {
    let parent = bundle.parent().ok_or("Не удалось определить папку Rivet.app")?;
    ensure_writable(parent)?;
    let name = bundle.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let backup = parent.join(format!("{name}.old-{current}"));
    if backup.exists() {
        fs::remove_dir_all(&backup).map_err(|e| format!("{}: {e}", backup.display()))?;
    }
    fs::rename(bundle, &backup).map_err(|e| format!("Не удалось переместить {}: {e}", bundle.display()))?;
    if let Err(e) = move_dir(new_bundle, bundle) {
        let _ = fs::remove_dir_all(bundle);
        let _ = fs::rename(&backup, bundle);
        return Err(format!("Не удалось установить новую версию: {e}"));
    }
    let executables = bundle.join("Contents").join("MacOS");
    if let Ok(entries) = fs::read_dir(&executables) {
        for entry in entries.flatten() {
            make_executable(&entry.path());
        }
    }
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("xattr")
        .args(["-dr", "com.apple.quarantine"])
        .arg(bundle)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    Ok(backup)
}

/// A free `<name>.old` (or `<name>.old-N`) path next to `path`.
fn backup_path(path: &Path) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let first = path.with_file_name(format!("{name}.old"));
    if !first.exists() || fs::remove_file(&first).is_ok() {
        return first;
    }
    (1..)
        .map(|n| path.with_file_name(format!("{name}.old-{n}")))
        .find(|p| !p.exists() || fs::remove_file(p).is_ok())
        .expect("unbounded")
}

/// Copies the staged folder over `install_dir`. Existing files are renamed to
/// `<name>.old` first (a running `.exe` / loaded `.dll` can be renamed but not
/// overwritten); on failure everything is put back. Returns the backups.
pub fn install_files(staged: &Path, install_dir: &Path) -> Result<Vec<PathBuf>, String> {
    ensure_writable(install_dir)?;
    let mut backups: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut created: Vec<PathBuf> = Vec::new();
    let result = copy_over(staged, install_dir, &mut backups, &mut created);
    if let Err(e) = result {
        for path in created.iter().rev() {
            let _ = fs::remove_file(path);
        }
        for (original, backup) in backups.iter().rev() {
            let _ = fs::rename(backup, original);
        }
        return Err(format!("Не удалось установить новую версию: {e}"));
    }
    Ok(backups.into_iter().map(|(_, b)| b).collect())
}

fn copy_over(
    from: &Path,
    to: &Path,
    backups: &mut Vec<(PathBuf, PathBuf)>,
    created: &mut Vec<PathBuf>,
) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let src = entry.path();
        let dst = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_over(&src, &dst, backups, created)?;
            continue;
        }
        if dst.exists() {
            let backup = backup_path(&dst);
            fs::rename(&dst, &backup)?;
            backups.push((dst.clone(), backup));
        }
        fs::copy(&src, &dst)?;
        created.push(dst);
    }
    Ok(())
}

/// Swaps the installed app for `staged` (from [`download`]); returns what to
/// launch: the bundle on macOS (`open -n`), the executable elsewhere.
pub fn apply_update(
    platform: Platform,
    staged: &Path,
    exe: &Path,
    current: &Version,
    updates_root: &Path,
) -> Result<PathBuf, String> {
    let target = install_target(platform, exe)?;
    match platform {
        Platform::MacOs => {
            let backup = install_bundle(staged, &target, current)?;
            record_leftovers(updates_root, &[backup]);
            Ok(target)
        }
        Platform::Windows | Platform::Linux => {
            let backups = install_files(staged, &target)?;
            record_leftovers(updates_root, &backups);
            let launch = target.join(platform.app_name());
            make_executable(&launch);
            Ok(launch)
        }
    }
}

/// Starts the installed version as a separate process.
pub fn relaunch(platform: Platform, launch: &Path) -> Result<(), String> {
    use std::process::{Command, Stdio};
    let mut command = match platform {
        Platform::MacOs => {
            let mut c = Command::new("open");
            c.arg("-n").arg(launch);
            c
        }
        Platform::Windows | Platform::Linux => {
            let mut c = Command::new(launch);
            if let Some(dir) = launch.parent() {
                c.current_dir(dir);
            }
            c
        }
    };
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    command
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Новая версия установлена, но не запустилась: {e}. Запустите Rivet вручную"))
}

/// Installs the staged app over the running one and starts it. The caller
/// must quit right after `Ok` (the old process keeps running until then).
pub fn install_and_restart(staged: &Path) -> Result<(), String> {
    let platform = Platform::current();
    let exe = std::env::current_exe().map_err(|e| format!("Не удалось определить путь к Rivet: {e}"))?;
    let launch = apply_update(platform, staged, &exe, &current_version(), &updates_dir())?;
    relaunch(platform, &launch)
}

/// Deletes backups of the previous install and downloaded packages. Called on
/// start of a non-dev build.
pub fn cleanup_leftovers() -> usize {
    match std::env::current_exe() {
        Ok(exe) => cleanup_leftovers_at(Platform::current(), &exe, &updates_dir()),
        Err(_) => 0,
    }
}

/// [`cleanup_leftovers`] for an explicit executable and updates folder (tests).
/// Removes the paths recorded by the last install plus the well-known backup
/// names next to the app; returns how many were deleted.
pub fn cleanup_leftovers_at(platform: Platform, exe: &Path, updates_root: &Path) -> usize {
    let mut removed = 0;
    let remove = |path: &Path| -> bool {
        let result = if path.is_dir() { fs::remove_dir_all(path) } else { fs::remove_file(path) };
        result.is_ok()
    };
    let manifest = leftovers_manifest(updates_root);
    let mut kept = String::new();
    for line in fs::read_to_string(&manifest).unwrap_or_default().lines() {
        let path = Path::new(line.trim());
        if line.trim().is_empty() || !path.exists() {
            continue;
        }
        if remove(path) {
            removed += 1;
        } else {
            // Still in use (e.g. the old process is exiting): try next time.
            kept.push_str(line.trim());
            kept.push('\n');
        }
    }
    if kept.is_empty() {
        let _ = fs::remove_file(&manifest);
    } else {
        let _ = fs::write(&manifest, kept);
    }

    let (dir, prefix) = match platform {
        Platform::MacOs => match bundle_of(exe) {
            Some(bundle) => (
                bundle.parent().map(Path::to_path_buf),
                format!("{}.old-", bundle.file_name().unwrap_or_default().to_string_lossy()),
            ),
            None => (None, String::new()),
        },
        Platform::Windows | Platform::Linux => (
            exe.parent().map(Path::to_path_buf),
            format!("{}.old", exe.file_name().unwrap_or_default().to_string_lossy()),
        ),
    };
    if let Some(dir) = dir
        && let Ok(entries) = fs::read_dir(&dir)
    {
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with(&prefix) && remove(&entry.path()) {
                removed += 1;
            }
        }
    }
    if updates_root.exists() && fs::remove_dir_all(updates_root).is_ok() {
        removed += 1;
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn parses_versions() {
        assert_eq!(v("1.2.3"), Version::new(1, 2, 3));
        assert_eq!(v("v0.10.0"), Version::new(0, 10, 0));
        assert_eq!(v("1.2"), Version::new(1, 2, 0));
        assert_eq!(v("1.2.3-rc.1").pre.as_deref(), Some("rc.1"));
        assert_eq!(v("1.2.3+build.5"), Version::new(1, 2, 3));
        assert_eq!(v("0.1.0-dev").to_string(), "0.1.0-dev");
        for bad in ["", "1", "a.b.c", "1.2.3.4", "1.2.3-", "1..3", "1.2.x", "-1.2.3"] {
            assert!(Version::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn compares_versions() {
        assert!(v("1.2.4") > v("1.2.3"));
        assert!(v("1.10.0") > v("1.9.9"));
        assert!(v("2.0.0") > v("1.99.99"));
        assert!(v("1.0.0") > v("1.0.0-rc.2"));
        assert!(v("1.0.0-rc.2") > v("1.0.0-rc.1"));
        assert!(v("1.0.0-rc.10") > v("1.0.0-rc.9"));
        assert!(v("1.0.0-beta") > v("1.0.0-alpha.5"));
        assert!(v("1.0.0-alpha.1") > v("1.0.0-alpha"));
        assert!(v("1.0.0-alpha.beta") > v("1.0.0-alpha.1"));
        assert_eq!(v("v1.2.3").cmp(&v("1.2.3")), Ordering::Equal);
        assert!(v("0.1.0") > v("0.1.0-dev"));
    }

    #[test]
    fn detects_cargo_target() {
        assert!(runs_from_cargo_target(Path::new("/src/rivet/target/debug/rivet")));
        assert!(runs_from_cargo_target(Path::new("/src/rivet/target/aarch64-apple-darwin/release/rivet")));
        assert!(!runs_from_cargo_target(Path::new("/Applications/Rivet.app/Contents/MacOS/Rivet")));
        assert!(!runs_from_cargo_target(Path::new("/src/rivet/target/macos-release/Rivet.app/Contents/MacOS/Rivet")));
    }

    fn asset(name: &str) -> Asset {
        Asset { name: name.into(), browser_download_url: format!("https://x/{name}"), size: 1 }
    }

    #[test]
    fn selects_asset_per_platform() {
        let assets = vec![
            asset("Rivet-1.3.0-linux-x64.tar.gz"),
            asset("Rivet-1.3.0-macos-universal.dmg"),
            asset("Rivet-1.3.0-macos-universal.zip"),
            asset("Rivet-1.3.0-windows-x64.zip"),
            asset("SHA256SUMS"),
        ];
        let pick = |p| select_asset(&assets, p).map(|a| a.name.as_str());
        assert_eq!(pick(Platform::MacOs), Some("Rivet-1.3.0-macos-universal.zip"));
        assert_eq!(pick(Platform::Windows), Some("Rivet-1.3.0-windows-x64.zip"));
        assert_eq!(pick(Platform::Linux), Some("Rivet-1.3.0-linux-x64.tar.gz"));
        assert_eq!(select_asset(&assets[1..2], Platform::MacOs), None);
    }

    #[test]
    fn parses_release_json() {
        let json = br#"{
            "tag_name": "v1.3.0", "name": "", "body": "* Fixes\n* More",
            "html_url": "https://github.com/rivet-soft/rivet/releases/tag/v1.3.0",
            "published_at": "2026-10-12T09:30:00Z",
            "assets": [
                {"name": "Rivet-1.3.0-linux-x64.tar.gz", "browser_download_url": "https://d/l", "size": 20},
                {"name": "SHA256SUMS", "browser_download_url": "https://d/s", "size": 300}
            ]
        }"#;
        let release = parse_release(json, Platform::Linux).unwrap();
        assert_eq!(release.version, Version::new(1, 3, 0));
        assert_eq!(release.name, "Rivet 1.3.0");
        assert_eq!(release.notes, "* Fixes\n* More");
        assert_eq!(release.asset.size, 20);
        assert_eq!(release.checksums.unwrap().browser_download_url, "https://d/s");
        let err = parse_release(json, Platform::Windows).unwrap_err();
        assert!(err.contains("Windows"), "{err}");
        assert_eq!(format_date("2026-10-12T09:30:00Z"), "12.10.2026");
        assert_eq!(format_date(""), "");
    }

    #[test]
    fn parses_and_verifies_checksums() {
        let hash = sha256_hex(b"hello");
        assert_eq!(hash, "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
        let text = format!(
            "{hash}  Rivet-1.3.0-linux-x64.tar.gz\n{}  *dist/Rivet-1.3.0-windows-x64.zip\nbad line\n\n",
            "A".repeat(64)
        );
        let sums = parse_checksums(&text);
        assert_eq!(sums.len(), 2);
        assert_eq!(sums["Rivet-1.3.0-linux-x64.tar.gz"], hash);
        assert_eq!(sums["Rivet-1.3.0-windows-x64.zip"], "a".repeat(64));

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("pkg");
        fs::write(&file, b"hello").unwrap();
        assert!(verify_file(&file, &hash.to_uppercase()).is_ok());
        let err = verify_file(&file, &"0".repeat(64)).unwrap_err();
        assert!(err.contains("Контрольная сумма"), "{err}");
    }

    #[test]
    fn locates_bundle_and_detects_translocation() {
        assert_eq!(
            bundle_of(Path::new("/Applications/Rivet.app/Contents/MacOS/Rivet")),
            Some(PathBuf::from("/Applications/Rivet.app"))
        );
        assert_eq!(bundle_of(Path::new("/usr/local/bin/rivet")), None);
        assert!(is_translocated(Path::new("/Volumes/Rivet/Rivet.app")));
        assert!(is_translocated(Path::new(
            "/private/var/folders/x/AppTranslocation/ABC/d/Rivet.app"
        )));
        assert!(!is_translocated(Path::new("/Applications/Rivet.app")));
        assert!(install_target(Platform::MacOs, Path::new("/usr/local/bin/rivet")).is_err());
        let err = install_target(Platform::MacOs, Path::new("/Volumes/Rivet/Rivet.app/Contents/MacOS/Rivet"))
            .unwrap_err();
        assert!(err.contains("Программы"), "{err}");
        assert_eq!(
            install_target(Platform::Linux, Path::new("/opt/rivet/rivet")).unwrap(),
            PathBuf::from("/opt/rivet")
        );
    }
}

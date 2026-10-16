//! core::update against a local HTTP server: release check, download with
//! checksum verification and extraction, file swap and leftover cleanup.
//! Nothing here touches the running app or its install location.

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rivet::core::Settings;
use rivet::core::http::CancelToken;
use rivet::core::update::{
    self, Asset, Platform, ReleaseInfo, Version, check_latest_at, cleanup_leftovers_at,
    download_to, extract_archive, install_bundle, install_files, locate_app, sha256_hex,
};

type Routes = Arc<Mutex<HashMap<String, (u16, Vec<u8>)>>>;
type Seen = Arc<Mutex<Vec<(String, Vec<(String, String)>)>>>;

/// Serves `routes` (path → status, body); `/redirect/<path>` answers 302 to
/// `/<path>` like GitHub's download URLs. Records each request's headers.
fn server(routes: Routes) -> (String, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let seen: Seen = Arc::default();
    let log = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let (routes, log) = (routes.clone(), log.clone());
            std::thread::spawn(move || handle(stream, &routes, &log));
        }
    });
    (format!("http://{addr}"), seen)
}

fn handle(stream: TcpStream, routes: &Routes, seen: &Seen) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).unwrap_or(0) == 0 {
            break;
        }
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.push((k.to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    seen.lock().unwrap().push((path.clone(), headers));
    let mut stream = stream;
    if let Some(rest) = path.strip_prefix("/redirect") {
        let head = format!(
            "HTTP/1.1 302 Found\r\nLocation: {rest}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        let _ = stream.write_all(head.as_bytes());
        return;
    }
    let (status, body) = routes
        .lock()
        .unwrap()
        .get(&path)
        .cloned()
        .unwrap_or((404, b"{\"message\":\"Not Found\"}".to_vec()));
    let head = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&body);
    let _ = stream.read(&mut [0u8; 1]);
}

const LATEST: &str = "/repos/rivet-soft/rivet/releases/latest";

fn release_json(base: &str, tag: &str) -> Vec<u8> {
    serde_json::json!({
        "tag_name": tag,
        "name": format!("Rivet {}", tag.trim_start_matches('v')),
        "body": "## Что нового\n- Автообновление",
        "html_url": format!("https://github.com/rivet-soft/rivet/releases/tag/{tag}"),
        "published_at": "2026-10-12T09:30:00Z",
        "assets": [
            {"name": "Rivet-1.3.0-macos-universal.zip", "browser_download_url": format!("{base}/redirect/mac.zip"), "size": 10},
            {"name": "Rivet-1.3.0-windows-x64.zip", "browser_download_url": format!("{base}/win.zip"), "size": 11},
            {"name": "Rivet-1.3.0-linux-x64.tar.gz", "browser_download_url": format!("{base}/linux.tar.gz"), "size": 12},
            {"name": "SHA256SUMS", "browser_download_url": format!("{base}/redirect/SHA256SUMS"), "size": 300}
        ]
    })
    .to_string()
    .into_bytes()
}

#[test]
fn check_latest_reports_only_newer_releases() {
    let routes: Routes = Arc::default();
    let (base, seen) = server(routes.clone());
    routes.lock().unwrap().insert(LATEST.into(), (200, release_json(&base, "v1.3.0")));
    let settings = Settings::default();

    let found = check_latest_at(&base, &settings, &Version::new(1, 2, 9), Platform::Linux)
        .unwrap()
        .expect("1.3.0 is newer");
    assert_eq!(found.version, Version::new(1, 3, 0));
    assert_eq!(found.tag, "v1.3.0");
    assert_eq!(found.name, "Rivet 1.3.0");
    assert!(found.notes.contains("Автообновление"));
    assert_eq!(found.published_at, "2026-10-12T09:30:00Z");
    assert_eq!(found.asset.name, "Rivet-1.3.0-linux-x64.tar.gz");
    assert_eq!(found.checksums.as_ref().unwrap().name, "SHA256SUMS");

    let mac = check_latest_at(&base, &settings, &Version::new(1, 0, 0), Platform::MacOs).unwrap().unwrap();
    assert_eq!(mac.asset.name, "Rivet-1.3.0-macos-universal.zip");

    for current in ["1.3.0", "1.4.0"] {
        let none = check_latest_at(&base, &settings, &Version::parse(current).unwrap(), Platform::Linux).unwrap();
        assert!(none.is_none(), "{current}");
    }
    // A pre-release of the same version is older.
    let rc = Version::parse("1.3.0-rc.1").unwrap();
    assert!(check_latest_at(&base, &settings, &rc, Platform::Windows).unwrap().is_some());

    let (_, headers) = seen.lock().unwrap()[0].clone();
    let get = |k: &str| headers.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    assert_eq!(get("accept").as_deref(), Some("application/vnd.github+json"));
    assert!(get("user-agent").unwrap().starts_with("rivet/"));

    // No releases yet → nothing to update; server errors are reported.
    routes.lock().unwrap().remove(LATEST);
    assert!(check_latest_at(&base, &settings, &Version::new(1, 0, 0), Platform::Linux).unwrap().is_none());
    routes.lock().unwrap().insert(LATEST.into(), (500, b"oops".to_vec()));
    let err = check_latest_at(&base, &settings, &Version::new(1, 0, 0), Platform::Linux).unwrap_err();
    assert!(err.contains("500"), "{err}");
}

fn zip_bytes(files: &[(&str, &[u8], u32)]) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut out);
        for (name, data, mode) in files {
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .unix_permissions(*mode);
            zip.start_file(*name, options).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
    }
    out.into_inner()
}

fn tar_gz_bytes(files: &[(&str, &[u8], u32)]) -> Vec<u8> {
    let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut tar = tar::Builder::new(gz);
    for (name, data, mode) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(*mode);
        header.set_cksum();
        tar.append_data(&mut header, name, *data).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap()
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).unwrap().permissions().mode() & 0o111 != 0
}

#[test]
fn extracts_zip_and_tar_into_staging() {
    let dir = tempfile::tempdir().unwrap();

    let zip = dir.path().join("Rivet-1.3.0-macos-universal.zip");
    fs::write(
        &zip,
        zip_bytes(&[
            ("Rivet.app/Contents/Info.plist", b"<plist/>", 0o644),
            ("Rivet.app/Contents/MacOS/Rivet", b"#!/bin/sh\n", 0o755),
            ("Rivet.app/Contents/._Info.plist", b"fork", 0o644),
            ("__MACOSX/Rivet.app/._x", b"fork", 0o644),
        ]),
    )
    .unwrap();
    let staging = dir.path().join("mac");
    extract_archive(&zip, &staging).unwrap();
    let app = locate_app(&staging, Platform::MacOs).unwrap();
    assert_eq!(app, staging.join("Rivet.app"));
    assert!(app.join("Contents/MacOS/Rivet").is_file());
    assert!(!app.join("Contents/._Info.plist").exists());
    assert!(!staging.join("__MACOSX").exists());
    #[cfg(unix)]
    assert!(is_executable(&app.join("Contents/MacOS/Rivet")));

    let win = dir.path().join("Rivet-1.3.0-windows-x64.zip");
    fs::write(
        &win,
        zip_bytes(&[
            ("Rivet-1.3.0-windows-x64/Rivet.exe", b"MZ", 0o644),
            ("Rivet-1.3.0-windows-x64/WebView2Loader.dll", b"dll", 0o644),
            ("Rivet-1.3.0-windows-x64/examples/shop-api.yaml", b"openapi: 3.0.0", 0o644),
        ]),
    )
    .unwrap();
    let staging = dir.path().join("win");
    extract_archive(&win, &staging).unwrap();
    assert_eq!(
        locate_app(&staging, Platform::Windows).unwrap(),
        staging.join("Rivet-1.3.0-windows-x64")
    );

    let tgz = dir.path().join("Rivet-1.3.0-linux-x64.tar.gz");
    fs::write(
        &tgz,
        tar_gz_bytes(&[
            ("Rivet-1.3.0-linux-x64/rivet", b"\x7fELF", 0o755),
            ("Rivet-1.3.0-linux-x64/examples/shop-api.yaml", b"openapi: 3.0.0", 0o644),
        ]),
    )
    .unwrap();
    let staging = dir.path().join("linux");
    extract_archive(&tgz, &staging).unwrap();
    let folder = locate_app(&staging, Platform::Linux).unwrap();
    assert_eq!(folder, staging.join("Rivet-1.3.0-linux-x64"));
    #[cfg(unix)]
    assert!(is_executable(&folder.join("rivet")));
    assert!(locate_app(&staging, Platform::MacOs).is_none());

    let bad = dir.path().join("pkg.rar");
    fs::write(&bad, b"x").unwrap();
    assert!(extract_archive(&bad, &dir.path().join("bad")).is_err());
}

fn release(base: &str, asset_path: &str, size: u64, platform: Platform) -> ReleaseInfo {
    ReleaseInfo {
        version: Version::new(1, 3, 0),
        tag: "v1.3.0".into(),
        name: "Rivet 1.3.0".into(),
        notes: String::new(),
        html_url: "https://example.invalid/release".into(),
        published_at: String::new(),
        asset: Asset {
            name: format!("Rivet-1.3.0{}", platform.asset_suffix()),
            browser_download_url: format!("{base}{asset_path}"),
            size,
        },
        checksums: Some(Asset {
            name: "SHA256SUMS".into(),
            browser_download_url: format!("{base}/redirect/SHA256SUMS"),
            size: 0,
        }),
    }
}

#[test]
fn download_verifies_checksum_and_stages_the_app() {
    let routes: Routes = Arc::default();
    let (base, seen) = server(routes.clone());
    let package = tar_gz_bytes(&[
        ("Rivet-1.3.0-linux-x64/rivet", b"\x7fELF new", 0o755),
        ("Rivet-1.3.0-linux-x64/README.md", b"readme", 0o644),
    ]);
    let name = "Rivet-1.3.0-linux-x64.tar.gz";
    let sums = format!("{}  Rivet-1.3.0-windows-x64.zip\n{}  {name}\n", "0".repeat(64), sha256_hex(&package));
    routes.lock().unwrap().insert("/SHA256SUMS".into(), (200, sums.into_bytes()));
    routes.lock().unwrap().insert("/linux.tar.gz".into(), (200, package.clone()));

    let root = tempfile::tempdir().unwrap();
    let received = Arc::new(AtomicU64::new(0));
    let total = Arc::new(AtomicU64::new(0));
    let progress = {
        let (received, total) = (received.clone(), total.clone());
        Arc::new(move |r: u64, t: u64| {
            received.store(r, Ordering::SeqCst);
            total.store(t, Ordering::SeqCst);
        })
    };
    let info = release(&base, "/redirect/linux.tar.gz", package.len() as u64, Platform::Linux);
    let staged = download_to(root.path(), &info, &Settings::default(), progress, &CancelToken::new(), Platform::Linux)
        .unwrap();
    assert_eq!(staged, root.path().join("1.3.0/staging/Rivet-1.3.0-linux-x64"));
    assert_eq!(fs::read(staged.join("rivet")).unwrap(), b"\x7fELF new");
    assert!(!root.path().join("1.3.0").join(name).exists(), "archive removed after extraction");
    assert_eq!(received.load(Ordering::SeqCst), package.len() as u64);
    assert_eq!(total.load(Ordering::SeqCst), package.len() as u64);
    let asset_request = seen
        .lock()
        .unwrap()
        .iter()
        .find(|(p, _)| p == "/linux.tar.gz")
        .cloned()
        .unwrap();
    assert!(asset_request.1.iter().any(|(k, v)| k == "accept-encoding" && v == "identity"));

    // Tampered package: checksum mismatch → error, nothing left behind.
    let mut tampered = package.clone();
    tampered[20] ^= 0xff;
    routes.lock().unwrap().insert("/linux.tar.gz".into(), (200, tampered));
    let err = download_to(
        root.path(),
        &info,
        &Settings::default(),
        Arc::new(|_, _| {}),
        &CancelToken::new(),
        Platform::Linux,
    )
    .unwrap_err();
    assert!(err.contains("Контрольная сумма"), "{err}");
    assert!(!root.path().join("1.3.0").join(name).exists());
    assert!(!root.path().join("1.3.0/staging").exists());

    // Missing line in SHA256SUMS, missing SHA256SUMS asset, truncated body.
    let windows = release(&base, "/linux.tar.gz", package.len() as u64, Platform::Windows);
    let mut no_line = windows.clone();
    no_line.asset.name = "Rivet-1.3.0-other.zip".into();
    let err = download_to(root.path(), &no_line, &Settings::default(), Arc::new(|_, _| {}), &CancelToken::new(), Platform::Windows)
        .unwrap_err();
    assert!(err.contains("нет строки"), "{err}");
    let mut no_sums = info.clone();
    no_sums.checksums = None;
    assert!(
        download_to(root.path(), &no_sums, &Settings::default(), Arc::new(|_, _| {}), &CancelToken::new(), Platform::Linux)
            .unwrap_err()
            .contains("SHA256SUMS")
    );
    routes.lock().unwrap().insert("/linux.tar.gz".into(), (200, package.clone()));
    let mut wrong_size = info.clone();
    wrong_size.asset.size += 5;
    assert!(
        download_to(root.path(), &wrong_size, &Settings::default(), Arc::new(|_, _| {}), &CancelToken::new(), Platform::Linux)
            .unwrap_err()
            .contains("не полностью")
    );
}

fn write(path: &Path, data: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, data).unwrap();
}

#[test]
fn install_files_keeps_backups_and_cleanup_removes_them() {
    let dir = tempfile::tempdir().unwrap();
    let install = dir.path().join("Rivet");
    write(&install.join("Rivet.exe"), b"old exe");
    write(&install.join("WebView2Loader.dll"), b"old dll");
    write(&install.join("examples/shop-api.yaml"), b"old yaml");
    write(&install.join("notes.txt"), b"user file");
    let staged = dir.path().join("staged");
    write(&staged.join("Rivet.exe"), b"new exe");
    write(&staged.join("WebView2Loader.dll"), b"new dll");
    write(&staged.join("examples/shop-api.yaml"), b"new yaml");

    let backups = install_files(&staged, &install).unwrap();
    assert_eq!(fs::read(install.join("Rivet.exe")).unwrap(), b"new exe");
    assert_eq!(fs::read(install.join("WebView2Loader.dll")).unwrap(), b"new dll");
    assert_eq!(fs::read(install.join("examples/shop-api.yaml")).unwrap(), b"new yaml");
    assert_eq!(fs::read(install.join("Rivet.exe.old")).unwrap(), b"old exe");
    assert_eq!(fs::read(install.join("notes.txt")).unwrap(), b"user file");
    assert_eq!(backups.len(), 3);

    // A second update while the first backup is still there.
    write(&staged.join("Rivet.exe"), b"newer exe");
    install_files(&staged, &install).unwrap();
    assert_eq!(fs::read(install.join("Rivet.exe")).unwrap(), b"newer exe");

    let updates = dir.path().join("data/updates");
    write(&updates.join("1.3.0/staging/x"), b"x");
    let removed = cleanup_leftovers_at(Platform::Windows, &install.join("Rivet.exe"), &updates);
    assert!(removed >= 2, "{removed}");
    assert!(!install.join("Rivet.exe.old").exists());
    assert!(!updates.exists());
    assert!(install.join("Rivet.exe").exists());
    assert!(install.join("examples/shop-api.yaml").exists());
    // Unrelated *.old files of the user are not touched without a manifest entry.
    assert!(install.join("examples/shop-api.yaml.old").exists());
    assert_eq!(fs::read(install.join("notes.txt")).unwrap(), b"user file");
}

#[test]
fn apply_update_records_backups_for_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let install = dir.path().join("opt/rivet");
    write(&install.join("rivet"), b"old");
    let staged = dir.path().join("staged/Rivet-1.3.0-linux-x64");
    write(&staged.join("rivet"), b"new");
    write(&staged.join("examples/shop-api.yaml"), b"yaml");
    write(&install.join("examples/shop-api.yaml"), b"old yaml");
    let updates = dir.path().join("data/Rivet/updates");
    let exe = install.join("rivet");

    let launch = update::apply_update(Platform::Linux, &staged, &exe, &Version::new(1, 2, 0), &updates).unwrap();
    assert_eq!(launch, exe);
    assert_eq!(fs::read(&exe).unwrap(), b"new");
    #[cfg(unix)]
    assert!(is_executable(&exe));
    assert!(install.join("rivet.old").exists());
    assert!(install.join("examples/shop-api.yaml.old").exists());

    cleanup_leftovers_at(Platform::Linux, &exe, &updates);
    assert!(!install.join("rivet.old").exists());
    assert!(!install.join("examples/shop-api.yaml.old").exists());
    assert!(!dir.path().join("data/Rivet/update-leftovers.txt").exists());
}

#[test]
fn install_bundle_swaps_app_and_cleanup_removes_old_bundle() {
    let dir = tempfile::tempdir().unwrap();
    let apps = dir.path().join("Applications");
    let bundle = apps.join("Rivet.app");
    write(&bundle.join("Contents/MacOS/Rivet"), b"old");
    write(&bundle.join("Contents/Info.plist"), b"old plist");
    let new_bundle = dir.path().join("staging/Rivet.app");
    write(&new_bundle.join("Contents/MacOS/Rivet"), b"new");
    write(&new_bundle.join("Contents/Info.plist"), b"new plist");

    let backup = install_bundle(&new_bundle, &bundle, &Version::new(1, 2, 0)).unwrap();
    assert_eq!(backup, apps.join("Rivet.app.old-1.2.0"));
    assert_eq!(fs::read(bundle.join("Contents/MacOS/Rivet")).unwrap(), b"new");
    assert_eq!(fs::read(backup.join("Contents/MacOS/Rivet")).unwrap(), b"old");
    assert!(!new_bundle.exists());
    #[cfg(unix)]
    assert!(is_executable(&bundle.join("Contents/MacOS/Rivet")));

    // A missing staged bundle leaves the installed one in place.
    let err = install_bundle(&dir.path().join("nope/Rivet.app"), &bundle, &Version::new(1, 3, 0)).unwrap_err();
    assert!(!err.is_empty());
    assert_eq!(fs::read(bundle.join("Contents/MacOS/Rivet")).unwrap(), b"new");

    let exe = bundle.join("Contents/MacOS/Rivet");
    let removed = cleanup_leftovers_at(Platform::MacOs, &exe, &dir.path().join("updates"));
    assert_eq!(removed, 1);
    assert!(!backup.exists());
    assert!(bundle.exists());
}

#[test]
fn apply_update_on_macos_requires_an_app_bundle() {
    let dir = tempfile::tempdir().unwrap();
    let staged: PathBuf = dir.path().join("Rivet.app");
    let err = update::apply_update(
        Platform::MacOs,
        &staged,
        &dir.path().join("target/debug/rivet"),
        &Version::new(1, 0, 0),
        &dir.path().join("updates"),
    )
    .unwrap_err();
    assert!(err.contains("Rivet.app"), "{err}");
}

/// The macOS package is made by `ditto -c -k --keepParent` (package-macos.sh):
/// permissions and symlinks must survive extraction.
#[cfg(target_os = "macos")]
#[test]
fn extracts_ditto_zip_with_permissions_and_symlinks() {
    use std::process::Command;
    let dir = tempfile::tempdir().unwrap();
    let app = dir.path().join("src/Rivet.app");
    write(&app.join("Contents/MacOS/Rivet"), b"#!/bin/sh\necho hi\n");
    write(&app.join("Contents/Resources/a.txt"), b"a");
    std::os::unix::fs::symlink("a.txt", app.join("Contents/Resources/link.txt")).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(app.join("Contents/MacOS/Rivet"), fs::Permissions::from_mode(0o755)).unwrap();
    }
    let zip = dir.path().join("Rivet-1.3.0-macos-universal.zip");
    let ok = Command::new("ditto").args(["-c", "-k", "--keepParent"]).arg(&app).arg(&zip).status().unwrap();
    assert!(ok.success());

    let staging = dir.path().join("staging");
    extract_archive(&zip, &staging).unwrap();
    let found = locate_app(&staging, Platform::MacOs).unwrap();
    assert!(is_executable(&found.join("Contents/MacOS/Rivet")));
    let link = found.join("Contents/Resources/link.txt");
    assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
    assert_eq!(fs::read(&link).unwrap(), b"a");
}

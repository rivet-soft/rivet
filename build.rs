fn main() {
    // Version shown in Settings → «О приложении». Release CI passes the git
    // tag (e.g. `v1.2.0`) in RIVET_VERSION; local builds are marked `-dev`.
    println!("cargo:rerun-if-env-changed=RIVET_VERSION");
    let version = std::env::var("RIVET_VERSION")
        .ok()
        .map(|v| v.trim().trim_start_matches('v').to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| format!("{}-dev", std::env::var("CARGO_PKG_VERSION").unwrap_or_default()));
    println!("cargo:rustc-env=RIVET_VERSION={version}");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winres::WindowsResource::new();
        if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu") && cfg!(unix) {
            resource
                .set_windres_path("x86_64-w64-mingw32-windres")
                .set_ar_path("x86_64-w64-mingw32-ar");
        }
        resource.set_icon("assets/app-icon.ico");
        resource.compile().expect("failed to embed Rivet icon");
    }
}

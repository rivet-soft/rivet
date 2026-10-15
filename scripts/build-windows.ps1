$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path

Push-Location $root
try {
    if (-not $env:GPUI_FXC_PATH) {
        $sdkBin = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
        if (Test-Path $sdkBin) {
            $fxc = Get-ChildItem $sdkBin -Filter fxc.exe -Recurse -ErrorAction SilentlyContinue |
                Where-Object { $_.FullName -match '\\x64\\fxc\.exe$' } |
                Sort-Object FullName -Descending |
                Select-Object -First 1
            if ($fxc) { $env:GPUI_FXC_PATH = $fxc.FullName }
        }
    }

    # Release CI passes the git tag (v1.2.0) so Settings shows the same version.
    if ($env:RIVET_VERSION) {
        $version = $env:RIVET_VERSION.Trim().TrimStart('v')
    } else {
        $versionLine = Select-String -Path Cargo.toml -Pattern '^version = "([^"]+)"' |
            Select-Object -First 1
        $version = $versionLine.Matches[0].Groups[1].Value + '-dev'
    }
    $env:RIVET_VERSION = $version

    cargo build --locked --release --bin rivet
    if ($LASTEXITCODE -ne 0) { throw 'Windows release build failed' }
    $package = "Rivet-$version-windows-x64"
    $output = Join-Path $root 'target\windows-release'
    $staging = Join-Path $output $package
    $archive = Join-Path $output "$package.zip"

    New-Item -ItemType Directory -Force (Join-Path $staging 'examples') | Out-Null
    Copy-Item 'target\release\rivet.exe' (Join-Path $staging 'Rivet.exe') -Force
    $loader = Get-ChildItem 'target\release\build' -Filter WebView2Loader.dll -Recurse |
        Where-Object { $_.FullName -match '\\x64\\WebView2Loader\.dll$' } |
        Select-Object -First 1
    if (-not $loader) { throw 'WebView2Loader.dll was not produced by the build' }
    Copy-Item $loader.FullName (Join-Path $staging 'WebView2Loader.dll') -Force
    Copy-Item 'examples\shop-api.yaml' (Join-Path $staging 'examples\shop-api.yaml') -Force
    if (Test-Path $archive) { Remove-Item $archive -Force }
    Compress-Archive -Path $staging -DestinationPath $archive
    Write-Host "Windows package: $archive"
}
finally {
    Pop-Location
}

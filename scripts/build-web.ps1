# Builds the web version into dist/, which can be uploaded to any static host.
#
# One-time setup (the wasm-bindgen version must match the one in Cargo.lock):
#   rustup target add wasm32-unknown-unknown
#   cargo install wasm-bindgen-cli --version 0.2.130 --locked
#
# Try it locally: python -m http.server 8080 -d dist, then open http://localhost:8080
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot
$dist = Join-Path $root 'dist'
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root 'target' }

Push-Location $root
try {
    # --locked: the CLI has to match the wasm-bindgen version in Cargo.lock.
    cargo build --locked --profile web --target wasm32-unknown-unknown
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

    if (Test-Path $dist) { Remove-Item $dist -Recurse -Force -Confirm:$false }
    wasm-bindgen --target web --no-typescript --remove-name-section --remove-producers-section --out-dir $dist "$target/wasm32-unknown-unknown/web/bevy-particle.wasm"
    if ($LASTEXITCODE -ne 0) { throw 'wasm-bindgen failed' }

    Copy-Item web/index.html $dist
    Copy-Item assets $dist -Recurse

    if (Get-Command wasm-opt -ErrorAction SilentlyContinue) {
        $wasm = Join-Path $dist 'bevy-particle_bg.wasm'
        wasm-opt -Os --all-features -o $wasm $wasm
        if ($LASTEXITCODE -ne 0) { throw 'wasm-opt failed' }
    }
    $size = (Get-Item (Join-Path $dist 'bevy-particle_bg.wasm')).Length / 1MB
    Write-Host ('Built dist/ (wasm {0:N1} MB before compression)' -f $size)
} finally {
    Pop-Location
}

# Builds target\release\omega.exe then Omega-<version>-x64.msi at the project root.
# Requires WiX Toolset 3.x (candle/light) in $env:WIX_BIN, or default %USERPROFILE%\tools\wix314.
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$wix = if ($env:WIX_BIN) { $env:WIX_BIN } else { Join-Path $env:USERPROFILE 'tools\wix314' }
$version = (Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
$obj = Join-Path $root 'target\wix'
New-Item -ItemType Directory -Force -Path $obj | Out-Null

Push-Location $root
try {
    # cargo writes its progress to stderr; with ErrorActionPreference=Stop, PowerShell 5.1 turns that into a
    # terminating NativeCommandError even when the build succeeds. Relax it for the call, then check the exit code.
    $ErrorActionPreference = 'Continue'
    cargo build --release
    $ErrorActionPreference = 'Stop'
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
    & (Join-Path $wix 'candle.exe') -nologo -arch x64 "-dVersion=$version" "-dRoot=$root" -ext WixUIExtension -ext WixUtilExtension `
        -out (Join-Path $obj 'omega.wixobj') (Join-Path $PSScriptRoot 'omega.wxs')
    if ($LASTEXITCODE -ne 0) { throw 'candle failed' }
    $msi = Join-Path $root "Omega-$version-x64.msi"
    & (Join-Path $wix 'light.exe') -nologo -ext WixUIExtension -ext WixUtilExtension -cultures:fr-FR -spdb `
        -out $msi (Join-Path $obj 'omega.wixobj')
    if ($LASTEXITCODE -ne 0) { throw 'light failed' }
    Write-Host "MSI: $msi"
} finally {
    Pop-Location
}

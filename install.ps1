param([string] $InstallDir)

$ErrorActionPreference = 'Stop'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

if ([Runtime.InteropServices.RuntimeInformation]::OSArchitecture -ne [Runtime.InteropServices.Architecture]::X64) {
    throw 'Editur does not publish a Windows build for this architecture.'
}
if ([string]::IsNullOrWhiteSpace($InstallDir)) {
    $InstallDir = if ($env:EDITUR_INSTALL_DIR) {
        $env:EDITUR_INSTALL_DIR
    } else {
        Join-Path $env:LOCALAPPDATA 'Programs\Editur'
    }
}

$asset = 'editur-windows-x86_64.exe'
$releaseBase = 'https://github.com/snowdamiz/editur/releases/download/release'
$temporary = Join-Path ([IO.Path]::GetTempPath()) ("editur-install-$([Guid]::NewGuid())")
[IO.Directory]::CreateDirectory($temporary) | Out-Null

try {
    $binary = Join-Path $temporary $asset
    $checksum = "$binary.sha256"
    Invoke-WebRequest -UseBasicParsing -Uri "$releaseBase/$asset" -OutFile $binary
    Invoke-WebRequest -UseBasicParsing -Uri "$releaseBase/$asset.sha256" -OutFile $checksum

    $expectedHash = (Get-Content -LiteralPath $checksum -Raw).Trim().ToLowerInvariant()
    if ($expectedHash -notmatch '^[0-9a-f]{64}$') {
        throw 'The release checksum is invalid.'
    }
    $actualHash = (Get-FileHash -LiteralPath $binary -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actualHash -ne $expectedHash) {
        throw 'The downloaded binary failed SHA-256 verification.'
    }

    Write-Host 'Editur includes Cursor Agent as a proprietary third-party dependency.'
    Write-Host 'It is downloaded directly from Cursor and is subject to https://cursor.com/terms-of-service.'
    & $binary --provision-agent
    if ($LASTEXITCODE -ne 0) {
        throw "Cursor Agent provisioning failed with exit code $LASTEXITCODE."
    }

    [IO.Directory]::CreateDirectory($InstallDir) | Out-Null
    $destination = Join-Path $InstallDir 'editur.exe'
    Copy-Item -LiteralPath $binary -Destination "$destination.new" -Force
    Move-Item -LiteralPath "$destination.new" -Destination $destination -Force

    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $pathEntries = @($userPath -split ';' | Where-Object { $_ })
    if ($pathEntries -notcontains $InstallDir) {
        $newPath = if ([string]::IsNullOrWhiteSpace($userPath)) {
            $InstallDir
        } else {
            "$userPath;$InstallDir"
        }
        [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
    }
    Write-Host "Installed Editur to $destination"
    Write-Host 'Open a new terminal, then run: editur .'
} finally {
    if ([IO.Directory]::Exists($temporary)) {
        Remove-Item -LiteralPath $temporary -Recurse -Force
    }
}

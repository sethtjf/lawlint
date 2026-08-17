$ErrorActionPreference = "Stop"

$DownloadBaseUrl = if ($env:DOWNLOAD_BASE_URL) { $env:DOWNLOAD_BASE_URL.TrimEnd("/") } else { "https://assets.lawlint.com/downloads" }
$arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
if ($arch -ne "X64") {
  throw "lawlint currently publishes a Windows CLI for x64 only (detected $arch)."
}

$archive = "lawlint-x86_64-pc-windows-msvc.zip"
$url = "$DownloadBaseUrl/latest/$archive"
$checksumsUrl = "$DownloadBaseUrl/latest/SHA256SUMS"
$tempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("lawlint-" + [guid]::NewGuid())
$zipPath = Join-Path $tempDir $archive
$checksumsPath = Join-Path $tempDir "SHA256SUMS"
$installDir = if ($env:LAWLINT_INSTALL_DIR) { $env:LAWLINT_INSTALL_DIR } else { Join-Path $HOME "bin" }

New-Item -ItemType Directory -Force -Path $tempDir | Out-Null
try {
  Write-Host "Downloading lawlint for Windows x64..."
  Invoke-WebRequest -Uri $url -OutFile $zipPath
  Invoke-WebRequest -Uri $checksumsUrl -OutFile $checksumsPath
  $expected = (Get-Content $checksumsPath |
    Where-Object { $_ -match "^([0-9a-fA-F]{64})\s+\*?$([regex]::Escape($archive))$" } |
    Select-Object -First 1) -replace "\s+.*$", ""
  if (-not $expected) {
    throw "No checksum was published for $archive."
  }
  $actual = (Get-FileHash -Algorithm SHA256 -Path $zipPath).Hash
  if ($actual -ne $expected.ToUpperInvariant()) {
    throw "Checksum mismatch for $archive."
  }
  Expand-Archive -Path $zipPath -DestinationPath $tempDir -Force
  New-Item -ItemType Directory -Force -Path $installDir | Out-Null
  Copy-Item (Join-Path $tempDir "lawlint.exe") (Join-Path $installDir "lawlint.exe") -Force

  $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
  $parts = if ($userPath) { $userPath -split ";" | Where-Object { $_ } } else { @() }
  if ($parts -notcontains $installDir) {
    [Environment]::SetEnvironmentVariable("Path", (($parts + $installDir) -join ";"), "User")
  }
  $env:Path = "$installDir;$env:Path"
  Write-Host "Installed lawlint to $(Join-Path $installDir "lawlint.exe")"
  Write-Host "Restart PowerShell to refresh PATH, then run: lawlint --help"
}
finally {
  Remove-Item $tempDir -Recurse -Force -ErrorAction SilentlyContinue
}

$ErrorActionPreference = "Stop"

$DownloadBaseUrl = if ($env:DOWNLOAD_BASE_URL) { $env:DOWNLOAD_BASE_URL.TrimEnd("/") } else { "https://downloads.lawlint.dev" }
$arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
if ($arch -ne "X64") {
  throw "lawlint currently publishes a Windows CLI for x64 only (detected $arch)."
}

$archive = "lawlint-x86_64-pc-windows-msvc.zip"
$url = "$DownloadBaseUrl/latest/$archive"
$tempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("lawlint-" + [guid]::NewGuid())
$zipPath = Join-Path $tempDir $archive
$installDir = if ($env:LAWLINT_INSTALL_DIR) { $env:LAWLINT_INSTALL_DIR } else { Join-Path $HOME "bin" }

New-Item -ItemType Directory -Force -Path $tempDir | Out-Null
try {
  Write-Host "Downloading lawlint for Windows x64..."
  Invoke-WebRequest -Uri $url -OutFile $zipPath
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

$ErrorActionPreference = "Stop"

$Repo = if ($env:AGENTLINK_REPO) { $env:AGENTLINK_REPO } else { "loyality7/AgentLink" }
$InstallDir = Join-Path $env:LOCALAPPDATA "AgentLink\bin"
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null

$ZipUrl = "https://github.com/$Repo/releases/latest/download/agentlink-x86_64-pc-windows-msvc.zip"
$TmpZip = Join-Path $env:TEMP "agentlink-win.zip"

Write-Host "Downloading AgentLink from $ZipUrl..."
Invoke-WebRequest -Uri $ZipUrl -OutFile $TmpZip
Expand-Archive -Path $TmpZip -DestinationPath $InstallDir -Force
Remove-Item $TmpZip -Force

$UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($UserPath -notlike "*$InstallDir*") {
    [Environment]::SetEnvironmentVariable("Path", "$UserPath;$InstallDir", "User")
    $env:Path = "$env:Path;$InstallDir"
}

Write-Host "Installed agentlink to $InstallDir\agentlink.exe"
Write-Host "Next: run 'agentlink init --name <your-name>' and 'agentlink setup'"


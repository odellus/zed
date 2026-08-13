# Build & install crow from source into %LOCALAPPDATA%\Crow, mirroring zed's
# Windows layout so the `crow` command launches the GUI detached (the prompt
# returns at once and is NOT filled with the GUI's logs).
#
# Layout (same idea as script/bundle-windows.ps1 + script/install-crow):
#   %LOCALAPPDATA%\Crow\crow.exe       <- GUI binary (the `zed` crate, bin `crow`)
#   %LOCALAPPDATA%\Crow\bin\crow.exe   <- CLI binary (the `cli` crate)
#   %APPDATA%\Microsoft\Windows\Start Menu\Programs\Crow.lnk  <- Start menu
#   %LOCALAPPDATA%\Crow\bin on the user PATH
#
# The CLI binary finds the GUI at ..\crow.exe relative to itself
# (crates/cli/src/main.rs, windows `Detect`), then spawns it with stdio
# nulled -- so `crow` returns your prompt immediately instead of holding the
# terminal and dumping logs into it.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File script\install-crow.ps1
#   powershell -ExecutionPolicy Bypass -File script\install-crow.ps1 -Jobs 4

param(
    [int]$Jobs = 6
)

$ErrorActionPreference = "Stop"
Set-Location (Split-Path -Parent $PSScriptRoot)

$AppDir = Join-Path $env:LOCALAPPDATA "Crow"
$BinDir = Join-Path $AppDir "bin"
$GuiExe = Join-Path $AppDir "crow.exe"
$CliExe = Join-Path $BinDir "crow.exe"

Write-Output "==> Building crow + cli (release, -j $Jobs)..."
cargo build --release -j $Jobs -p zed -p cli
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

Write-Output "==> Installing to $AppDir..."
Remove-Item -Recurse -Force $AppDir -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $BinDir | Out-Null

# GUI binary -> app root (the path the CLI looks for: ..\crow.exe)
Copy-Item "target\release\crow.exe" $GuiExe
# CLI binary -> bin (this is what goes on PATH; it spawns the GUI detached)
Copy-Item "target\release\cli.exe" $CliExe

# Start menu entry: points straight at the GUI, like zed's installer.
$StartMenu = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs"
$WshShell = New-Object -ComObject WScript.Shell
$Shortcut = $WshShell.CreateShortcut((Join-Path $StartMenu "Crow.lnk"))
$Shortcut.TargetPath = $GuiExe
$Shortcut.WorkingDirectory = $AppDir
$Shortcut.IconLocation = "$GuiExe,0"
$Shortcut.Description = "Crow AI Dev Environment"
$Shortcut.Save()

# Remove stale Zed shortcuts from earlier installs (they point at binaries
# that no longer exist).
foreach ($stale in "Zed.lnk", "Zed Dev.lnk", "Zed Nightly.lnk", "Zed Preview.lnk") {
    Remove-Item (Join-Path $StartMenu $stale) -ErrorAction SilentlyContinue
}

# Put the CLI on the user PATH (persisted; takes effect in new terminals).
$UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
if (($UserPath -split ";") -notcontains $BinDir) {
    $NewPath = $UserPath.TrimEnd(";") + ";" + $BinDir
    [Environment]::SetEnvironmentVariable("Path", $NewPath, "User")
    Write-Output "==> Added $BinDir to your user PATH (open a new terminal)"
} else {
    Write-Output "==> $BinDir already on your user PATH"
}

Write-Output ""
Write-Output "==> Crow installed."
Write-Output "    Run with: crow   (launches detached; your prompt returns at once)"
Write-Output "    CLI:     $CliExe"
Write-Output "    GUI:     $GuiExe"
Write-Output "    Start:   $StartMenu\Crow.lnk"

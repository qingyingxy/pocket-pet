[CmdletBinding(SupportsShouldProcess=$true)]
param(
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA "Programs\PocketPet"),
    [switch]$NoLaunch
)
$ErrorActionPreference="Stop"
$repoRoot=Split-Path -Parent $PSScriptRoot
$source=Join-Path $repoRoot "slint-preview\target\release\pocket-pet-slint.exe"
$icon=Join-Path $repoRoot "slint-preview\ui\cat.ico"
if(!(Test-Path -LiteralPath $source)){throw "Build first: cargo build --release --manifest-path slint-preview/Cargo.toml"}
if(!(Test-Path -LiteralPath $icon)){throw "Missing cat.ico. Run cargo run --manifest-path slint-preview/Cargo.toml --example make_icon"}
$InstallDir=[IO.Path]::GetFullPath($InstallDir)
$destination=Join-Path $InstallDir "pocket-pet-slint.exe"
if(!$PSCmdlet.ShouldProcess($InstallDir,"Install Pocket Pet and create Desktop / Start Menu shortcuts")){return}

# Request normal save-and-exit. If saving/import blocks exit, do not overwrite.
$running=@(Get-Process pocket-pet-slint -ErrorAction SilentlyContinue)
if($running.Count){
    $quit=Start-Process -FilePath $source -ArgumentList "--quit" -WindowStyle Hidden -PassThru
    if(!$quit.WaitForExit(5000)){throw "Exit request timed out. Close Pocket Pet from the tray and retry."}
    foreach($process in $running){
        if(!$process.WaitForExit(10000)){throw "Pocket Pet is still running. Finish imports/save, then exit from the tray and retry. Nothing was overwritten."}
    }
}
[void][IO.Directory]::CreateDirectory($InstallDir)
if([IO.Path]::GetFullPath($source) -ne $destination){Copy-Item -LiteralPath $source -Destination $destination -Force}
Copy-Item -LiteralPath $icon -Destination (Join-Path $InstallDir "cat.ico") -Force
Copy-Item -LiteralPath (Join-Path $repoRoot "LICENSE") -Destination (Join-Path $InstallDir "LICENSE.txt") -Force
Copy-Item -LiteralPath (Join-Path $repoRoot "docs\licenses\Slint-Royalty-free-2.0.md") -Destination (Join-Path $InstallDir "Slint-LICENSE.md") -Force

$shell=New-Object -ComObject WScript.Shell
# Unicode assembled explicitly so this script also works under Windows PowerShell 5.1.
$name=([string][char]0x53e3)+[char]0x888b+[char]0x5ba0+[char]0x7269
$desktop=[Environment]::GetFolderPath("Desktop")
$programs=[Environment]::GetFolderPath("Programs")
foreach($folder in @($desktop,$programs)){
    if([string]::IsNullOrWhiteSpace($folder)){throw "Desktop or Start Menu location is unavailable."}
    [void][IO.Directory]::CreateDirectory($folder)
    $shortcut=$shell.CreateShortcut((Join-Path $folder ($name+".lnk")))
    $shortcut.TargetPath=$destination
    $shortcut.WorkingDirectory=$InstallDir
    $shortcut.IconLocation=(Join-Path $InstallDir "cat.ico")+",0"
    $shortcut.Description="Pocket Pet - quick notes and gentle reminders"
    $shortcut.Save()
}
# Preserve opt-in startup, but update its old executable path after installation.
$runKey="HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
$existing=Get-ItemProperty -LiteralPath $runKey -Name "PocketPetSlint" -ErrorAction SilentlyContinue
if($null -ne $existing){
    Set-ItemProperty -LiteralPath $runKey -Name "PocketPetSlint" -Value ('"'+$destination+'"')
}
Write-Output "Installed: $destination"
Write-Output "Desktop and Start Menu shortcuts created. Notes remain in LOCALAPPDATA\PocketPetSlintPreview."
if(!$NoLaunch){Start-Process -FilePath $destination -WindowStyle Hidden}
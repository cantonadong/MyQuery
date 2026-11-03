param([string]$LlvmBin = $env:LLVM_MINGW_BIN)
$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot
if (-not $LlvmBin -or -not (Test-Path (Join-Path $LlvmBin 'ld.lld.exe'))) {
    throw 'Set LLVM_MINGW_BIN or pass -LlvmBin with the LLVM-MinGW bin directory.'
}
$env:PATH = $LlvmBin + ';' + $env:PATH
$env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = Join-Path $LlvmBin 'ld.lld.exe'
$env:RUSTFLAGS = '-C linker-flavor=ld.lld -C link-self-contained=yes'
$env:SLINT_STYLE = 'fluent-light'
if (-not (Test-Path 'ui/icons/app.ico') -or (Get-Item 'ui/icons/app.ico').LastWriteTimeUtc -lt (Get-Item 'ui/icons/app.png').LastWriteTimeUtc) {
Add-Type -AssemblyName System.Drawing
$sourceIcon = [System.Drawing.Image]::FromFile((Join-Path $PSScriptRoot 'ui/icons/app.png'))
$iconSizes = @(16, 24, 32, 48, 64, 128, 256)
$iconParts = @()
try {
    foreach ($iconSize in $iconSizes) {
        $bitmap = [System.Drawing.Bitmap]::new($iconSize, $iconSize)
        $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
        $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
        $graphics.DrawImage($sourceIcon, 0, 0, $iconSize, $iconSize)
        $buffer = [System.IO.MemoryStream]::new()
        $bitmap.Save($buffer, [System.Drawing.Imaging.ImageFormat]::Png)
        $iconParts += ,$buffer.ToArray()
        $buffer.Dispose()
        $graphics.Dispose()
        $bitmap.Dispose()
    }
} finally { $sourceIcon.Dispose() }
$iconWriter = [System.IO.BinaryWriter]::new([System.IO.File]::Create((Join-Path $PSScriptRoot 'ui/icons/app.ico')))
try {
    $iconWriter.Write([uint16]0); $iconWriter.Write([uint16]1); $iconWriter.Write([uint16]$iconSizes.Count)
    $iconOffset = 6 + 16 * $iconSizes.Count
    for ($iconIndex = 0; $iconIndex -lt $iconSizes.Count; $iconIndex++) {
        $dimension = $iconSizes[$iconIndex] % 256
        $iconWriter.Write([byte]$dimension); $iconWriter.Write([byte]$dimension)
        $iconWriter.Write([uint16]0); $iconWriter.Write([uint16]1); $iconWriter.Write([uint16]32)
        $iconWriter.Write([uint32]$iconParts[$iconIndex].Length); $iconWriter.Write([uint32]$iconOffset)
        $iconOffset += $iconParts[$iconIndex].Length
    }
    foreach ($iconPart in $iconParts) { $iconWriter.Write([byte[]]$iconPart) }
} finally { $iconWriter.Dispose() }
}
& cargo +stable-x86_64-pc-windows-gnu build --release --locked
if ($LASTEXITCODE -ne 0) { throw 'Release build failed.' }
$appProcesses = Get-Process -Name SqlDesk,sql-desk -ErrorAction SilentlyContinue
if ($appProcesses) {
    $appProcesses | Stop-Process -Force
    $appProcesses | Wait-Process -Timeout 10
}
Copy-Item -LiteralPath 'target/release/sql-desk.exe' -Destination 'SqlDesk.exe' -Force
Get-Item -LiteralPath 'SqlDesk.exe' | Select-Object FullName,Length,LastWriteTime

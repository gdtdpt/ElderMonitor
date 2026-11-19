#requires -version 5.1
<#
  fetch-mpv.ps1 — 下载 Windows 版 libmpv 到 vendor\mpv

  用法：
    tools\fetch-mpv.ps1              # 下载最新 mpv-dev（x86_64）
    tools\fetch-mpv.ps1 -DryRun      # 只打印将下载的版本，不下载
    tools\fetch-mpv.ps1 -Force       # 已存在也重新下载
    tools\fetch-mpv.ps1 -Dest D:\mpv # 指定落盘目录

  产物：
    vendor\mpv\libmpv-2.dll
    vendor\mpv\include\mpv\*.h
    vendor\mpv\VERSION.txt
#>
[CmdletBinding()]
param(
    [string]$Dest,
    [string]$AssetPattern = '^mpv-dev-x86_64-.*\.7z$',
    [string]$Archive,
    [string]$Proxy,
    [switch]$Force,
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

if (-not $Dest) {
    $root = Split-Path -Parent $PSScriptRoot
    $Dest = Join-Path $root 'vendor\mpv'
}

$api = 'https://api.github.com/repos/shinchiro/mpv-winbuild-cmake/releases/latest'
$headers = @{ 'User-Agent' = 'ElderMonitor-fetch-mpv' }
if ($Proxy) { $proxyArg = @{ Proxy = $Proxy } } else { $proxyArg = @{} }

function Write-Step($msg) { Write-Host "[mpv] $msg" }

$release = $null
$asset = $null

if ($Archive) {
    if (-not (Test-Path $Archive)) { throw "指定的归档不存在：$Archive" }
    Write-Step "使用本地归档：$Archive"
}
else {
    Write-Step "查询最新发布 ..."
    $lastErr = $null
    for ($i = 1; $i -le 3 -and -not $release; $i++) {
        try {
            $release = Invoke-RestMethod -Uri $api -Headers $headers -UseBasicParsing -TimeoutSec 60 @proxyArg
        }
        catch {
            $lastErr = $_.Exception.Message
            if ($i -lt 3) {
                Write-Step "第 $i 次查询失败，2 秒后重试 ..."
                Start-Sleep -Seconds 2
            }
        }
    }

    if (-not $release) {
        Write-Host ""
        Write-Host "[mpv] 无法访问 GitHub API：$lastErr"
        Write-Host ""
        Write-Host "两条路，任选其一："
        Write-Host "  1) 设代理后重试：  tools\fetch-mpv.ps1 -Proxy http://127.0.0.1:7890"
        Write-Host "  2) 手动下载后喂给脚本："
        Write-Host "     在 https://github.com/shinchiro/mpv-winbuild-cmake/releases 下载 mpv-dev-x86_64-*.7z"
        Write-Host "     再执行：  tools\fetch-mpv.ps1 -Archive D:\downloads\mpv-dev-x86_64-xxx.7z"
        exit 1
    }

    $asset = $release.assets | Where-Object { $_.name -match $AssetPattern } | Select-Object -First 1
    if (-not $asset) {
        $names = ($release.assets | ForEach-Object { $_.name }) -join ', '
        throw "发布 $($release.tag_name) 里没有匹配 '$AssetPattern' 的资产。现有：$names"
    }
    Write-Step "版本：$($release.tag_name)"
    Write-Step "资产：$($asset.name)  ($([math]::Round($asset.size / 1MB, 1)) MB)"
}

Write-Step "落盘：$Dest"

if ($DryRun) {
    Write-Step "DryRun：不下载、不解压。"
    exit 0
}

$dll = Join-Path $Dest 'libmpv-2.dll'
if ((Test-Path $dll) -and (-not $Force)) {
    Write-Step "已存在 $dll，跳过（-Force 可重下）"
    exit 0
}

$downloaded = $false
if ($Archive) {
    $tmpArchive = (Resolve-Path $Archive).Path
}
else {
    $tmpArchive = Join-Path $env:TEMP ("mpv-dev-" + [guid]::NewGuid().ToString('N') + '.7z')
    Write-Step "下载中（$([math]::Round($asset.size / 1MB, 1)) MB）..."
    $ok = $false
    for ($i = 1; $i -le 3 -and -not $ok; $i++) {
        try {
            Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $tmpArchive -UseBasicParsing -TimeoutSec 300 @proxyArg
            $ok = $true
        }
        catch {
            if ($i -lt 3) {
                Write-Step "第 $i 次下载失败，3 秒后重试 ..."
                Start-Sleep -Seconds 3
            }
            else {
                throw "下载失败：$($_.Exception.Message)。可手动下载后使用 -Archive 参数。"
            }
        }
    }
    $downloaded = $true
}

# 找解压工具：优先 7z，其次系统自带 bsdtar（支持读取 7z）
$sevenZip = $null
$candidates = @('7z.exe', (Join-Path $env:ProgramFiles '7-Zip\7z.exe'))
if (${env:ProgramFiles(x86)}) { $candidates += (Join-Path ${env:ProgramFiles(x86)} '7-Zip\7z.exe') }
foreach ($c in $candidates) {
    if (Test-Path $c) { $sevenZip = $c; break }
    $cmd = Get-Command $c -ErrorAction SilentlyContinue
    if ($cmd) { $sevenZip = $cmd.Source; break }
}

$tmpDir = Join-Path $env:TEMP ("mpv-dev-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmpDir -Force | Out-Null

Write-Step "解压 ..."
if ($sevenZip) {
    & $sevenZip x $tmpArchive "-o$tmpDir" -y | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "7-Zip 解压失败（退出码 $LASTEXITCODE）" }
}
elseif (Get-Command tar.exe -ErrorAction SilentlyContinue) {
    Write-Step "未找到 7-Zip，改用系统 tar（bsdtar）解 7z"
    & tar.exe -xf $tmpArchive -C $tmpDir
    if ($LASTEXITCODE -ne 0) { throw "tar 解压失败，请安装 7-Zip：winget install 7-Zip" }
}
else {
    throw "没有可用的解压工具。请安装 7-Zip（winget install 7-Zip）后重试。"
}

$srcDll = Get-ChildItem -Path $tmpDir -Filter 'libmpv-2.dll' -Recurse | Select-Object -First 1
if (-not $srcDll) { throw "解压后没找到 libmpv-2.dll，归档结构可能变了，请检查 $tmpDir" }

New-Item -ItemType Directory -Path $Dest -Force | Out-Null
Copy-Item $srcDll.FullName (Join-Path $Dest 'libmpv-2.dll') -Force

$srcInc = Join-Path $srcDll.DirectoryName 'include'
if (Test-Path $srcInc) {
    $dstInc = Join-Path $Dest 'include'
    if (Test-Path $dstInc) { Remove-Item $dstInc -Recurse -Force }
    Copy-Item $srcInc $dstInc -Recurse -Force
}

$verText = if ($release) { $release.tag_name } else { 'local-archive: ' + (Split-Path -Leaf $tmpArchive) }
Set-Content -Path (Join-Path $Dest 'VERSION.txt') -Value $verText -Encoding ASCII

$hash = (Get-FileHash (Join-Path $Dest 'libmpv-2.dll') -Algorithm SHA256).Hash
Write-Step "完成：$Dest\libmpv-2.dll"
Write-Step "SHA256：$hash"
Write-Step "头文件：$Dest\include\mpv\client.h"

if ($downloaded) { Remove-Item $tmpArchive -Force -ErrorAction SilentlyContinue }
Remove-Item $tmpDir -Recurse -Force -ErrorAction SilentlyContinue

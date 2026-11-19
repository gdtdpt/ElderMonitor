#!/usr/bin/env pwsh
<#
.SYNOPSIS
    ElderMonitor 构建脚本：编译 → 测试 → 整理 dist 目录。

.DESCRIPTION
    Windows 侧的统一入口。零参数即可完成「构建 + 单测 + 产物整理」。
    libmpv 是运行时动态加载的，不参与链接；但产物目录里必须带上 dll 才能跑。

    注意：本文件必须保存为 UTF-8 with BOM，否则 Windows PowerShell 5.1
    会按 ANSI 解码中文并报语法错误。

.EXAMPLE
    tools\build.ps1
    构建 release、跑测试、把 em-smoke.exe + libmpv-2.dll 整理到 dist\

.EXAMPLE
    tools\build.ps1 -Profile debug -SkipTest
    只做快速 debug 构建

.EXAMPLE
    tools\build.ps1 -Smoke 7734200
    构建完成后直接跑冒烟：解析该房间并用 libmpv 出声 12 秒
#>
[CmdletBinding()]
param(
    [ValidateSet('debug', 'release')]
    [string]$Profile = 'release',

    # 跳过 cargo test
    [switch]$SkipTest,

    # 只编译，不整理 dist
    [switch]$NoDist,

    # 构建后跑 em-smoke（值为房间号或直播间链接）
    [string]$Smoke = ''
)

[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$ErrorActionPreference = 'Stop'

# 不要在 param 默认值里用 $PSScriptRoot：`powershell -File x.ps1` 时它还是空的
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

function Write-Step([string]$msg) { Write-Host "[build] $msg" -ForegroundColor Cyan }
function Fail([string]$msg) { Write-Host "[build] $msg" -ForegroundColor Red; exit 1 }

<#
    调用原生命令的安全包装。

    PowerShell 5.1 会把原生命令写到 stderr 的内容（cargo 的 "Compiling ..." 就在 stderr）
    包成 ErrorRecord；脚本里一旦 $ErrorActionPreference = 'Stop'，这些进度输出就会
    变成终止错误，构建看起来「失败」其实编译正常。这里统一做三件事：
      1. 临时把 EAP 放成 Continue
      2. 把 stderr 混进 stdout 并当普通文本打印（不再污染错误流、不再染红）
      3. 以退出码为准
#>
function Invoke-Native {
    param(
        [string]$Exe,
        [string[]]$Arguments
    )
    $prev = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        & $Exe @Arguments 2>&1 | ForEach-Object {
            if ($_ -is [System.Management.Automation.ErrorRecord]) {
                Write-Host $_.Exception.Message
            } else {
                Write-Host $_
            }
        }
    } finally {
        $ErrorActionPreference = $prev
    }
    return $LASTEXITCODE
}

# ---------- 0. 前置检查 ----------
$dll = Join-Path $root 'vendor\mpv\libmpv-2.dll'
if (-not (Test-Path $dll)) {
    Write-Host "[build] 缺少 vendor\mpv\libmpv-2.dll" -ForegroundColor Red
    Write-Host "        先跑一次：tools\fetch-mpv.ps1" -ForegroundColor Yellow
    exit 1
}

# ---------- 1. 编译 ----------
$cargoArgs = @('build')
if ($Profile -eq 'release') { $cargoArgs += '--release' }

Write-Step "cargo $($cargoArgs -join ' ')"
$code = Invoke-Native -Exe 'cargo' -Arguments $cargoArgs
if ($code -ne 0) { Fail "编译失败（exit $code）" }

# ---------- 2. 测试 ----------
if (-not $SkipTest) {
    Write-Step 'cargo test --workspace'
    $code = Invoke-Native -Exe 'cargo' -Arguments @('test', '--workspace', '--quiet')
    if ($code -ne 0) { Fail "测试失败（exit $code）" }
}

# ---------- 3. 整理产物 ----------
$targetDir = Join-Path $root 'target'
$outDir = if ($Profile -eq 'release') { Join-Path $targetDir 'release' } else { Join-Path $targetDir 'debug' }
$dist = Join-Path $root 'dist'

if (-not $NoDist) {
    Write-Step '整理产物 → dist\'
    New-Item -ItemType Directory -Force -Path $dist | Out-Null

    # 目前只有 M0 的自测 CLI；M1 起会有 ElderMonitor.exe，届时自动带上
    $bins = @('em-smoke.exe', 'ElderMonitor.exe')
    $copied = 0
    foreach ($b in $bins) {
        $src = Join-Path $outDir $b
        if (Test-Path $src) {
            Copy-Item $src $dist -Force
            $copied++
        }
    }
    if ($copied -eq 0) { Fail "在 $outDir 里没找到任何可执行文件" }

    Copy-Item $dll $dist -Force

    # 便携配置目录：只有空目录，用户配置不进仓库
    New-Item -ItemType Directory -Force -Path (Join-Path $dist 'config') | Out-Null

    Write-Step '产物：'
    Get-ChildItem $dist -File |
        Sort-Object Name |
        ForEach-Object { Write-Host ("         {0,-24} {1,8:N1} MB" -f $_.Name, ($_.Length / 1MB)) }
}

# ---------- 4. 冒烟 ----------
if ($Smoke -ne '') {
    $exe = Join-Path $dist 'em-smoke.exe'
    if (-not (Test-Path $exe)) {
        $exe = Join-Path $outDir 'em-smoke.exe'
    }
    Write-Step "冒烟：$Smoke"
    $code = Invoke-Native -Exe $exe -Arguments @($Smoke)
    if ($code -ne 0) { Fail "冒烟未通过（exit $code）" }
}

Write-Step '完成'

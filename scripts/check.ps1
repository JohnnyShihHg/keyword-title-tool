# 一個指令跑完所有檢查：Rust 測試 → clippy（警告＝錯誤）→ JS（eslint）。CI 打包前也跑這支。
#   ./scripts/check.ps1
# 任何一步失敗就停下、回傳非 0。
$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot -Parent)

# 本機的 PATH 可能還沒有 cargo／node（剛裝完、或從別的殼開）
foreach ($dir in @("$env:USERPROFILE\.cargo\bin", "C:\Program Files\nodejs")) {
    if ((Test-Path $dir) -and -not (($env:Path -split ";") -contains $dir)) { $env:Path = "$dir;$env:Path" }
}

function Step($name, [scriptblock]$run) {
    Write-Host "`n== $name" -ForegroundColor Cyan
    & $run
    if ($LASTEXITCODE -ne 0) {
        Write-Host "✖ $name 沒過" -ForegroundColor Red
        exit $LASTEXITCODE
    }
}

Step "Rust 測試（core＋app）" { cargo test --workspace --quiet }
Step "clippy（警告＝錯誤）" { cargo clippy --workspace --all-targets --quiet -- -D warnings }

if (-not (Get-Command node -ErrorAction SilentlyContinue)) {
    Write-Host "✖ 找不到 node，JS 檢查跑不了（winget install OpenJS.NodeJS.LTS）" -ForegroundColor Red
    exit 1
}
if (-not (Test-Path node_modules\.bin\eslint.cmd)) {
    Step "安裝 eslint（第一次）" { npm ci --no-audit --no-fund }
}
Step "JS 檢查（eslint）" { npx --no-install eslint app/src }

Write-Host "`n✔ 全部通過" -ForegroundColor Green

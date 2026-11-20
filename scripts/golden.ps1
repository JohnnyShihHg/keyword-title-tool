# 自動切段＋遮蓋的回歸比對（golden test）：用固定的側錄、文稿、靜圖、掃描快取離線重跑一次，
# 比對 report.txt 的段落和遮罩是不是跟確認過的答案一樣。
#
#   ./scripts/golden.ps1            比對（動到自動切段、遮蓋就要跑）
#   ./scripts/golden.ps1 -Update    把這次的結果存成新的答案（確認過結果是對的才用！）
#
# 樣本放在 D:\kwt\golden\<案例>\（或環境變數 KWT_GOLDEN）。太大不進 repo，建法見 docs/交接.md「golden 測試」：
#   case.json     {"watch_dir": "側錄資料夾", "since": "21:50", "end_cutoff": "23:59:30", "only": [1,2]（選填）}
#   titles.txt    當天文稿
#   stills\       當天的替換畫面（凍結的複本，不要再動）
#   cache\        .autocut 的掃描快取（第一次 -Update 時存進來）
#   expected.json 確認過的答案；expected-report.txt 是同一份的 report.txt，給人看
#
# 判定：段落起訖差超過 1 格＝失敗；遮罩**多遮**（答案沒有遮的地方遮了）＝失敗；少遮只警告（寧可少遮）。
# 樣本或側錄找不到（例如連不到側錄主機）＝跳過並提示，不算失敗。
param([switch]$Update, [string]$Root = $(if ($env:KWT_GOLDEN) { $env:KWT_GOLDEN } else { "D:\kwt\golden" }))
$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot -Parent)
$repo = Get-Location
foreach ($dir in @("$env:USERPROFILE\.cargo\bin")) {
    if ((Test-Path $dir) -and -not (($env:Path -split ";") -contains $dir)) { $env:Path = "$dir;$env:Path" }
}
$Frame = 1 / 29.97

function Say($text, $color = "Gray") { Write-Host $text -ForegroundColor $color }

if (-not (Test-Path $Root)) {
    Say "⚠ 找不到 golden 樣本資料夾 $Root，跳過（建法見 docs/交接.md）" Yellow
    exit 0
}
$cases = @(Get-ChildItem $Root -Directory | Where-Object { Test-Path (Join-Path $_.FullName "case.json") })
if (-not $cases.Count) {
    Say "⚠ $Root 裡沒有任何案例（要有 case.json），跳過" Yellow
    exit 0
}

Say "== 建置（release）" Cyan
cargo build -p keyword-title-tool --release --quiet
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$exe = Join-Path $repo "target\release\keyword-title-tool.exe"

# 「00:12:34.567」→ 秒
function Sec($tc) {
    $p = $tc.Split(":")
    return [double]$p[0] * 3600 + [double]$p[1] * 60 + [double]::Parse($p[2], [Globalization.CultureInfo]::InvariantCulture)
}

# report.txt 的「== 段落 ==」→ [{n, label, pieces:[[s,e]], masks:[{s,e,layout,still,why}]}]
function Parse-Report($path) {
    $segs = @()
    $in = $false
    $cur = $null
    foreach ($line in [IO.File]::ReadAllLines($path, [Text.Encoding]::UTF8)) {
        if ($line -eq "== 段落 ==") { $in = $true; continue }
        if (-not $in) { continue }
        if ($line.StartsWith("== ")) { break }
        if ($line -match '^第 (\d+) 段(.*?)：長 (\S+)，遮罩 (\d+) 段') {
            $cur = [ordered]@{ n = [int]$matches[1]; label = $matches[2]; length = (Sec $matches[3]); pieces = @(); masks = @() }
            $segs += $cur
        } elseif ($cur -and $line -match '^  側錄 (\S+) – (\S+)') {
            $cur.pieces += , @((Sec $matches[1]), (Sec $matches[2]))
        } elseif ($cur -and $line -match '^    遮 (\S+) – (\S+) ｜ (.+?) ｜ (.*?) ｜ (.+?)( ｜ 輪播.*)?$') {
            $cur.masks += [ordered]@{ s = (Sec $matches[1]); e = (Sec $matches[2]); layout = $matches[3]; why = $matches[4]; still = $matches[5] }
        }
    }
    return , $segs
}

# 區間 a 減掉區間清單 bs（容許 tol 秒）之後剩下的總長
function Uncovered($s, $e, $bs, $tol) {
    $left = @(, @($s, $e))
    foreach ($b in $bs) {
        $next = @()
        foreach ($r in $left) {
            $bs0 = $b[0] - $tol; $be0 = $b[1] + $tol
            if ($be0 -le $r[0] -or $bs0 -ge $r[1]) { $next += , $r; continue }
            if ($bs0 -gt $r[0]) { $next += , @($r[0], $bs0) }
            if ($be0 -lt $r[1]) { $next += , @($be0, $r[1]) }
        }
        $left = $next
    }
    return ($left | ForEach-Object { $_[1] - $_[0] } | Measure-Object -Sum).Sum
}

$failed = 0
foreach ($case in $cases) {
    $dir = $case.FullName
    $name = $case.Name
    Say "`n== 案例 $name" Cyan
    $c = Get-Content (Join-Path $dir "case.json") -Raw -Encoding UTF8 | ConvertFrom-Json
    if (-not (Test-Path -LiteralPath $c.watch_dir)) {
        Say "⚠ 連不到側錄資料夾（$($c.watch_dir)），跳過這個案例" Yellow
        continue
    }
    $out = Join-Path $env:TEMP "kwt-golden\$name"
    if (Test-Path $out) { Remove-Item $out -Recurse -Force }
    New-Item -ItemType Directory -Force "$out\.autocut" | Out-Null
    $cache = Join-Path $dir "cache"
    if (Test-Path $cache) { Copy-Item "$cache\*" "$out\.autocut\" -Force }
    elseif (-not $Update) { Say "⚠ 還沒有掃描快取（cache\），請先確認結果後跑 -Update 建立" Yellow; continue }

    $cfg = [ordered]@{
        watch_dir = $c.watch_dir; stills_dir = (Join-Path $dir "stills"); output_dir = $out
        titles = @{ kind = "File"; value = (Join-Path $dir "titles.txt") }
        auto_render = $false; make_all = $false; since = $c.since; end_cutoff = $c.end_cutoff
        rotate_secs = 5.0; ai_key = $null; gain_db = 0.0; watermarks = @()
    }
    if ($c.only) { $cfg.only = @($c.only) }
    $cfgPath = Join-Path $out "cfg.json"
    [IO.File]::WriteAllText($cfgPath, ($cfg | ConvertTo-Json -Depth 4), (New-Object Text.UTF8Encoding $false))
    $sw = [Diagnostics.Stopwatch]::StartNew()
    & $exe --autocut-batch $cfgPath *> (Join-Path $out "batch.log")
    Say ("  重跑 {0:N0} 秒" -f $sw.Elapsed.TotalSeconds)
    $report = Join-Path $out "report.txt"
    if (-not (Test-Path $report)) { Say "✖ 沒產生 report.txt（看 $out\batch.log）" Red; $failed++; continue }
    $got = Parse-Report $report

    if ($Update) {
        New-Item -ItemType Directory -Force $cache | Out-Null
        Get-ChildItem "$out\.autocut" -File | Where-Object { $_.Name -match '\.cache$|\.ocr\.tsv$|^seams\.tsv$|^ai\.tsv$' } | Copy-Item -Destination $cache -Force
        [IO.File]::WriteAllText((Join-Path $dir "expected.json"), (ConvertTo-Json @($got) -Depth 6), (New-Object Text.UTF8Encoding $false))
        Copy-Item $report (Join-Path $dir "expected-report.txt") -Force
        Say "  已存成新的答案：$($got.Count) 段、$(($got | ForEach-Object { $_.masks.Count } | Measure-Object -Sum).Sum) 個遮罩（請看 expected-report.txt 確認是對的）" Green
        continue
    }

    $want = @(Get-Content (Join-Path $dir "expected.json") -Raw -Encoding UTF8 | ConvertFrom-Json)
    $bad = 0
    foreach ($w in $want) {
        $g = $got | Where-Object { $_.n -eq $w.n } | Select-Object -First 1
        if (-not $g) { Say "  ✖ 第 $($w.n) 段不見了" Red; $bad++; continue }
        if ($g.label -ne $w.label) { Say "  ✖ 第 $($w.n) 段狀態「$($g.label)」，答案是「$($w.label)」" Red; $bad++ }
        $wp = @($w.pieces); $gp = @($g.pieces)
        $sameCount = $wp.Count -eq $gp.Count
        $off = if ($sameCount) { (0..($wp.Count - 1) | ForEach-Object { [math]::Max([math]::Abs($wp[$_][0] - $gp[$_][0]), [math]::Abs($wp[$_][1] - $gp[$_][1])) } | Measure-Object -Maximum).Maximum } else { 999 }
        if (-not $sameCount -or $off -gt $Frame * 1.5) {
            Say ("  ✖ 第 {0} 段起訖不同：長 {1:N3} 秒（答案 {2:N3}），最多差 {3:N3} 秒" -f $w.n, $g.length, $w.length, $off) Red
            $bad++
        }
        # 遮罩用「蓋到的時間」比，同版型（滿版／雙框）分開算
        foreach ($layout in @(@($w.masks) + @($g.masks) | ForEach-Object { $_.layout } | Sort-Object -Unique)) {
            $wm = @($w.masks | Where-Object { $_.layout -eq $layout } | ForEach-Object { , @($_.s, $_.e) })
            $gm = @($g.masks | Where-Object { $_.layout -eq $layout } | ForEach-Object { , @($_.s, $_.e) })
            foreach ($m in $g.masks | Where-Object { $_.layout -eq $layout }) {
                $extra = Uncovered $m.s $m.e $wm ($Frame * 1.5)
                if ($extra -gt 0.001) {
                    Say ("  ✖ 第 {0} 段多遮：{1:N3}–{2:N3}（{3}，{4}，{5}）多了 {6:N2} 秒" -f $w.n, $m.s, $m.e, $layout, $m.why, $m.still, $extra) Red
                    $bad++
                }
            }
            foreach ($m in $w.masks | Where-Object { $_.layout -eq $layout }) {
                $miss = Uncovered $m.s $m.e $gm ($Frame * 1.5)
                if ($miss -gt 0.001) {
                    Say ("  ⚠ 第 {0} 段少遮：{1:N3}–{2:N3}（{3}，{4}）少了 {5:N2} 秒" -f $w.n, $m.s, $m.e, $layout, $m.still, $miss) Yellow
                }
            }
        }
    }
    foreach ($g in $got | Where-Object { $n = $_.n; -not ($want | Where-Object { $_.n -eq $n }) }) {
        Say "  ✖ 多出第 $($g.n) 段" Red
        $bad++
    }
    if ($bad) { $failed++; Say "  ✖ $name 有 $bad 個問題（這次的報告：$report）" Red }
    else { Say "  ✔ $name 一樣" Green }
}

if ($failed) { Say "`n✖ golden 有 $failed 個案例沒過" Red; exit 1 }
Say "`n✔ golden 全部通過" Green

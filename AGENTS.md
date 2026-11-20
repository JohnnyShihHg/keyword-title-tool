# AGENTS.md

《關鍵時刻》剪輯工具：整理上片文稿、從側錄自動切段＋遮蓋翻攝畫面＋出片、串接 promo、上傳 YouTube。
Tauri 2 桌面程式（Rust `core/` 純邏輯＋`app/src-tauri/` 執行；前端 `app/src/` 原生 JS）。開發細節見 [docs/交接.md](docs/交接.md)，使用者說明見 [docs/使用說明.md](docs/使用說明.md)。

## 指令

```powershell
./scripts/check.ps1            # 測試＋clippy＋JS 檢查，全部要過
./scripts/golden.ps1           # 切段＋遮蓋的回歸比對（要有 D:\kwt\golden 樣本，見 docs/交接.md）
cargo build -p keyword-title-tool
keyword-title-tool.exe --autocut-batch cfg.json   # 離線重跑一次監看，不開視窗、不上傳
```

## 功能名稱（報告、測試、commit 都用這組）

文稿｜一次貼上｜匯入｜共用同步｜影片合成｜自動切段｜遮蓋｜串接｜上傳｜自動更新

## 一定要做

- **完成任何修改前，執行 `./scripts/check.ps1` 並全部通過。** 動到自動切段或遮蓋，另外跑 `./scripts/golden.ps1`。
- **回報完成時附上證據**：check 的輸出，以及實際跑過的結果（report.txt 的段長、輸出檔路徑、截圖或 log）。不能只說「應該可以了」。
- **寧可少遮，不能多遮。** 多遮了重出也沒用；golden 比對多出一個遮罩就是失敗。
- 每修一個 bug，補一個重現它的測試；使用者給的實際輸入（文稿、批次貼上）存成 `core/tests/fixtures/` 的樣本。
- 畫面上的說明一句話、講人話；細節寫進 `docs/使用說明.md`。
- 判斷用的門檻數字寫成具名常數＋一行註解說為什麼是這個值，不要直接寫在條件裡。
- 依日期存的設定只用 `dayLoad`／`daySave`（app.js）或 `acDayLoad`／`acDaySave`（autocut.js），不要自己寫 localStorage；存失敗要顯示在畫面上。
- 前端事件一律用 `addEventListener`，不要寫 `el.onX = (e) => a && b`（回傳 false 會吃掉按鍵和點擊，eslint 會擋）。
- 動到哪個 `.rs` 檔，順手 `cargo fmt` 那個檔（整個專案還沒統一格式，不要一次全部重排）。

## 不要做

- 使用者沒說「發版」就不要發版。發版只用 `scripts/release.sh X.Y.Z "說明"`（commit 訊息格式：`版本 X：修正／改善／新增：…`）。
- 不要用 11/10 的資料測試（使用者指定）。
- 不要用 PowerShell 的 `Set-Content`／`Out-File` 改 UTF-8 檔案（5.1 版會加 BOM）；用編輯工具或 `sed`。例外：`.ps1` 一定要有 BOM，否則 Windows PowerShell 5.1 讀不懂中文。
- 不要真的上傳 YouTube、呼叫 Gemini 來測試；用離線批次模式和已存的 `ai.tsv`。
- 沒問過使用者，不要新增相依套件。
- **repo 是公開的：不要寫入內網位址或內部主機名稱**（IP、`\\主機\分享` 的真實名稱），程式、測試、註解、文件都一樣；用 `\\server\share`、`\\伺服器\…`、`<側錄主機>` 代替。

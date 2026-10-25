# 關鍵標題整理工具

把每天分段送來的【關鍵時刻】標題稿，整理成 YouTube 上片用的文稿格式（含全集章節時間點）。Tauri 桌面應用（Rust core + 原生 HTML/JS 前端），Windows 安裝檔。

## 功能

- **六段輸入**：每段貼上原始標題稿，開場、講者名字、「一段」標記照貼即可，會自動拿掉
- **即時預覽**：右邊整份文稿跟著更新，可「全部複製」或「存成 txt」
- **全形轉換**：`! ? : ( )` 轉全形、`"` 轉「」，`.`、數字、英文維持原樣
- **時間點**：可打 `2708`、`27:08`、`0:27:08`，自動統一成 `00:27:08`，輸出到全集區
- **草稿自動保存**：關掉重開內容還在
- **設定**：主持人、節目名稱、輸出格式、講者名字判斷字數、六種畫面配色
- **自動更新**：開啟時檢查新版，設定頁一鍵下載安裝
- **影片－單集**：完整影片 ⚡ YT 影片（選填，可裁切）｜ 45 秒 promo
- **影片－全集**：第一段 ｜ 20 秒 promo ⚡ 第二段 ⚡ … ⚡ 第六段 ｜ 45 秒 promo，可把各段章節時間填回文稿
  - ⚡ 白閃過場（xfade fadewhite，預設 0.5 秒）、｜ 直接接
  - 輸出 1920×1080、29.97fps、H.264 + AAC，自動偵測 NVIDIA／Intel／AMD 硬體編碼
  - 整體音量 +6dB（＝剪映的 +6）並加限幅器避免爆音，可在設定調整
  - ffmpeg、yt-dlp 第一次使用時從設定頁下載到程式資料夾
- **自動切段**：監看側錄資料夾，每錄完一個 10 分鐘檔就掃描；用標題條判斷節目／廣告、OCR 對文稿分段、依靜圖遮蓋翻攝畫面，完整一段就輸出 `第N段.mp4`，各段到齊合成 `全集.mp4`（OCR 需要 Windows 繁體中文 OCR）

## 安裝

到 [Releases](../../releases/latest) 下載 `KeywordTitleTool_x.y.z_x64-setup.exe` 執行。

首次啟動若出現「Windows 已保護您的電腦」，點「其他資訊」→「仍要執行」。這是因為本程式未購買程式碼簽章憑證，屬預期行為。之後的新版會在程式內提示，按「下載並安裝」即可。

## 輸出格式

```
第一段

（段標題）【關鍵時刻】20261016-1張炤和

（標題列表）
=======
第二段
……
=======
全集

（00：00：00）Part1：（段標題）【關鍵時刻】20261016－1張炤和
```

- 段標題：有「開場」用開場那行；沒寫開場但第一個講者前有標題，那行當開場；都沒有就用第一條
- 空的段落不輸出（稿子分批進來，沒填就是還沒拿到）
- 設定可切回原範本格式（`open`、`1.` 編號行、串一～串六檔頭）

## 開發

```bash
cargo test -p keyword-title-core   # 轉換規則與範本逐字比對
cargo run -p keyword-title-tool    # 開發模式執行
```

| 路徑 | 內容 |
|---|---|
| `core/` | 純函式核心：解析、全形轉換、時間點、排版、設定 |
| `core/src/autocut/` | 自動切段的判斷規則（從 mask-detector 照搬，常數改動前先用三天側錄重跑比對） |
| `core/tests/fixtures/20261014/` | 範本 `expected.txt` 與反推的輸入，golden test 逐字比對 |
| `app/src-tauri/` | Tauri 外殼：IPC 指令、設定檔、存檔、自動更新；`video.rs` 預留 ffmpeg |
| `app/src/` | 前端（不需建置，原樣打包） |
| `scripts/release.sh` | 發布新版 |

## 發布新版

```bash
scripts/release.sh 0.1.1 "這次改了什麼"
```

會改版本號、跑測試、commit、打 `v0.1.1` 標籤並推上 GitHub。GitHub Actions 約 10～15 分鐘建好安裝檔並發布 Release，已安裝的程式下次開啟就會看到更新提示。

### 注意

- **`productName` 必須維持純 ASCII 且不含空格**。GitHub Release 會改寫非 ASCII 檔名，`tauri-action` 對不上 `.sig` 就會靜默跳過 `latest.json`，自動更新整個失效但 CI 仍顯示成功。中文名稱放在視窗標題。
- 簽章私鑰在 `~/.tauri/keyword-title-tool.key`（不進 git），repo secrets 需要：
  - `TAURI_SIGNING_PRIVATE_KEY`：私鑰檔內容
  - `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`：空字串
- **私鑰遺失就無法再發布更新**，已安裝的版本會停在原地。請另外備份到密碼管理器。
- `Cargo.lock` 有鎖定版本：`zip` 4.x 已在 crates.io 下架，重新產生 lockfile 會解析失敗，不要刪掉它。
- 圖示用 Noto Serif TC Black（SIL OFL）產生。

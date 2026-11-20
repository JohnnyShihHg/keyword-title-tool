# 專案檢證報告（第 1～4 階段）

檢查日期：2026-11-20｜版本：0.6.38（工作區另有一個未 commit 的修正：`app/src/youtube.js` 播放清單搜尋欄）
依據：`project-audit-playbook.md`。第 1～4 階段只讀，沒有改任何程式。

---

## 1. 盤點

**用途**：把《關鍵時刻》每天的標題稿整理成上片文稿，並自動從側錄切段、遮蓋翻攝畫面、出片、串接 promo、上傳 YouTube。

**技術**：Tauri 2 桌面程式。Rust 分兩個 crate：`core/` 放純邏輯，`app/src-tauri/` 放 ffmpeg、OCR、網路、檔案。前端是原生 HTML/JS，沒有打包工具，也沒有 `package.json`。
主要相依套件：`serde`、`image`、`ureq`、`zip`、`trash`、`windows`（Media_Ocr）、tauri plugins（dialog、updater、process、single-instance）。

**功能清單**（之後的報告、測試、規則檔都用這組名稱）

| 名稱 | 說明 | 主要位置 |
|---|---|---|
| 文稿 | 六段標題稿 → 上片文稿＋全集時間表 | `core/src/parse.rs`、`render.rs`、`fullwidth.rs`、`app/src/app.js` |
| 一次貼上 | 整批文稿依段號拆開，放進空著的段落 | `core/src/batch.rs` |
| 匯入 | 讀回已存的 txt | `core/src/import.rs` |
| 共用同步 | 跟同事共用當天文稿 txt | `core/src/sync.rs`、`app/src-tauri/src/sync.rs` |
| 影片合成 | 單集／全集＋閃白＋promo | `core/src/video.rs`、`flash.rs`、`app/src-tauri/src/video.rs`、`app/src/video.js` |
| 自動切段 | 監看側錄、OCR 分段、找開頭結尾 | `core/src/autocut/plan.rs`、`titles.rs`、`seam.rs`、`app/src-tauri/src/autocut.rs` |
| 遮蓋 | 靜圖比對、AI 補判、手動遮罩、壓 BAR | `core/src/autocut/stills.rs`、`frame.rs`、`bars.rs`、`plan.rs`、`app/src/masks.js`、`bars.js`、`stills.js` |
| 串接 | 每段後接影片＋promo | `app/src-tauri/src/autocut.rs`（`promo_like`、chain） |
| 上傳 | YouTube 私人上傳、播放清單、縮圖 | `core/src/youtube.rs`、`app/src-tauri/src/youtube.rs`、`app/src/youtube.js` |
| 自動更新 | 讀 GitHub Release 的 latest.json | `tauri-plugin-updater`、`.github/workflows/release.yml` |

**目錄**（兩層）
```
core/src/            純邏輯（可測試）  autocut/ 切段與遮蓋的判斷
core/tests/          template.rs、fixtures/（文稿、一次貼上樣本）
app/src-tauri/src/   main.rs（指令進入點）、autocut.rs（監看／出片／串接）、video.rs、youtube.rs、sync.rs
app/src/             index.html、styles.css、app.js、autocut.js、video.js、youtube.js、masks.js、bars.js、stills.js
docs/使用說明.md     使用者說明
scripts/release.sh   發版腳本
.github/workflows/release.yml   推 v* 標籤 → 測 core → 打包簽章 → 發布
```

**進入點**
- 啟動：`cargo tauri dev`，或建置後執行 `target/release/keyword-title-tool.exe`
- 離線批次：`keyword-title-tool.exe --autocut-batch cfg.json`（用複製出來的快取重跑切段，沒有副作用）
- 測試：`cargo test --workspace`
- 發版：`scripts/release.sh X.Y.Z "說明"`

**現有檢查工具**：只有 `cargo test`。沒有 `rustfmt.toml`、`clippy.toml`，也沒有任何 JS 檢查（這台電腦沒有 node）。CI 只跑 `cargo test -p keyword-title-core`。

**AI 規則檔**：repo 裡沒有 `AGENTS.md` 或 `CLAUDE.md`。規則分散在兩個地方：一是 `docs/交接.md`（未加入 git，只在這台電腦），二是 Claude 的個人記憶，例如「寧可少遮」「說 Release 才發版」「不用 11/10 測試」。

**設定與機密**
- 程式碼沒有寫死的 API key 或密碼。Gemini 金鑰和 YouTube OAuth 都在設定頁，由使用者填。
- 簽章私鑰只放在 repo secret，`.gitignore` 有排除 `*.key`。
- 寫死的路徑只出現在測試裡（`autocut.rs:3646`、`stills.rs:337`、`core/src/video.rs:862`），沒有問題。
- 每天的手動遮罩、壓 BAR、串接設定只存在瀏覽器的 localStorage（`masks.js:18-27`、`bars.js`、`autocut.js:505-528`），以日期為 key，開始監看時才傳給後端（`autocut.js:248`）。

---

## 2. 健檢

| 指令 | 結果 | 數量 | 說明 |
|---|---|---|---|
| `cargo test --workspace` | ✅ 通過 | core 147＋3、app 11 | 有 1 個 ignored |
| `cargo build --release` | ✅ 通過 | — | |
| `cargo fmt --all --check` | ❌ | 645 處不符 | 檔案從沒跑過 rustfmt。最多的是 `autocut.rs` 237 處、`plan.rs` 122 處。就算改成 `max_width=140`，也還有 344 處 |
| `cargo clippy --workspace --all-targets` | ⚠ 18 個警告，0 錯誤 | 18 | `is_multiple_of` 5 個、`u32 as u32` 4 個、`chunks_exact` 3 個、複雜型別 3 個，其他 3 個。位置在 `autocut.rs:326、1785、2126…`、`frame.rs:289-290`、`plan.rs:622`、`stills.rs:275`、`fullwidth.rs:56` |
| JS lint／語法檢查 | 無 | — | 沒有 node，也沒有 eslint 設定 |

沒跑的指令：上傳、AI 呼叫、真的開始監看。這些有副作用，依原則 4 不跑。

---

## 3. 發現

### 3.1 不一致

1. **localStorage 存取寫了 6 份**：`app.js`、`autocut.js`、`bars.js`、`masks.js`、`video.js`、`youtube.js` 各有一組 `load/save`，而且都是 `try { … } catch {}`。建議統一成一個 helper，存失敗時要提示使用者。
2. **事件處理有兩種寫法**：`el.onX = …` 和 `addEventListener`。`onX = (e) => 條件 && 動作` 這種寫法，條件不成立時會回傳 `false`，等於取消瀏覽器的預設行為。**今天播放清單搜尋欄不能打英文、不能刪、不能貼，就是這個原因**（原本在 `youtube.js:310`，已修）。同樣的寫法還有一處：`video.js:148` 的 `box.onclick = (e) => e.target === box && close(false)`，目前沒影響，但哪天對話框裡加了勾選框就會出事。建議新程式一律用 `addEventListener`。
3. **門檻數字有兩種寫法**：有 74 個具名常數，但 `plan.rs` 裡也有直接寫在程式裡的數字，例如 `plan.rs:187-188` 的 `0.8`、`plan.rs:342-343` 的 `0.6／0.25`、`plan.rs:364` 和 `446` 的 `0.6／0.55`。AI 新增判斷時會隨便抄其中一種。建議以具名常數＋註解為準。
4. **發版有兩種做法**：有 `scripts/release.sh`，但實際上 AI 每次都手動改版本號。原因是工作區裡有未追蹤的 `docs/交接.md`，腳本檢查到「有未 commit 的修改」就不讓發。手動改已經出過一次錯：11/19 用 PowerShell 改版本號時，`tauri.conf.json` 被加上 BOM，當場發現後還原了。

### 3.2 風險點

| 項目 | 證據 | 說明 |
|---|---|---|
| 指紋檔寫入失敗被忽略 | `autocut.rs:3115`、`3155`、`3381` 的 `let _ = std::fs::write(...)` | 寫不進去時，下一輪會重新出片。推測：如果一直寫不進去，就會每 10 分鐘重出一次，而且沒有任何提示 |
| 監看中改設定失敗被忽略 | `autocut.js:149`、`155` 的 `.catch(() => {})` | 0.6.34 才修好「監看中改設定馬上生效」。如果這個呼叫失敗，畫面看起來已經改了，實際上沒生效 |
| 遮罩、壓 BAR、串接設定存檔失敗被忽略 | `masks.js:27`、`bars.js:28`、`autocut.js:528` 的 `catch {}` | 手動遮罩一旦沒存到，重開程式就不見了，可能造成漏遮。推測：發生機率低，但損失大 |
| CI 註解過時，app 的測試沒在 CI 跑 | `release.yml:21-23` 寫「app crate 沒有測試」 | 其實 app 有 11 個測試，包括指紋、since 解析、網路路徑。CI 只跑 core，這些壞了照樣會發版 |
| 過長的函式 | `video.js:307 createPanel` 605 行、`autocut.rs:1576 replan` 400 行、`plan.rs:384 plan_masks` 152 行、`autocut.rs:2157 ai_classify_shots` 150 行、`autocut.rs:3430 render_segment` 143 行 | 超過 80 行的共有 19 個 |
| 過大的檔案 | `autocut.rs` 3680 行、`video.js` 1939 行、`plan.rs` 1914 行、`app.js` 990 行、`core/src/video.rs` 953 行 | |
| 重複程式碼 | 見 3.1 第 1 項，6 份 localStorage helper | 其他沒有發現超過 15 行的明顯重複 |
| 沒用到的程式碼 | 沒有發現。編譯器和 clippy 都沒報 dead_code | |
| 已經跟程式不符的註解 | `release.yml:22`（同上）、`batch.rs` 檔頭（11/19 已更新） | |

### 3.3 歷史上出過的錯

共 121 個 commit，其中 36 個是修正。因為每個「版本」commit 都包含程式改動，用檔案統計很粗，下面直接整理修正的內容：

| 類型 | 例子（版本） | 現在有沒有測試 |
|---|---|---|
| **切段開頭結尾判斷** | 開場提要被當成第 1 段（0.6.35）、Logo 殘影（開頭往後推 3 格）、第六段廣告後接回切錯、最後一段沒收黑一直「錄製中」、側錄接縫重複一秒多、跨檔找不到 Logo 轉場（0.6.36） | 部分有：`plan.rs` 有 27 個單元測試。但真正的驗收是 AI 手動用 11/06、11/09、11/12 離線重跑後比對秒數，**沒有自動化** |
| **遮蓋多遮或少遮** | 一行來源字被誤判成多行導致到處誤遮、棚內螢幕被當成同一場遮掉、AI 接上相鄰鏡頭後只遮一個、手動遮罩超過段長整個不遮 | 單元測試只有一部分。多遮的後果最嚴重（見「寧可少遮」原則），**也沒有自動化的 golden 比對** |
| **監看狀態和重出** | 改設定不生效（0.6.34）、往前讀檔後已出好的段又重出（0.6.36，指紋） | 指紋有測試（`autocut.rs` 的 anchor test），狀態流轉沒有 |
| **串接和 promo 規格** | promo 每次重轉、規格不符就整段重編碼（0.6.36） | 沒有測試，靠出片前後比對規格 |
| **文稿輸入格式** | 一次貼上不認得 `2-1 名字`、`二段` 接 `2-2`（0.6.37、0.6.38）、讀不到文稿、清空後載入沒反應 | 一次貼上有 fixture（11/19 三批實際文稿）。其他是單元測試 |
| **前端互動** | 今天的播放清單搜尋欄不能打字 | **沒有任何 JS 檢查** |
| **YouTube** | 登入過期時自動上傳卡住 | `core/src/youtube.rs` 有 6 個測試，但流程本身沒有測試 |

最近一個月 AI 常被糾正的事，取自對話和交接：
- 改了遮蓋或切段邏輯，沒用舊資料離線驗證就想發版
- 回報「完成」但沒有親自驗證
- 畫面上的說明文字太長（規定：畫面一句話，細節放 `docs/使用說明.md`）
- 用 11/10 的資料測試（使用者說過不要）
- 沒說「發版」就自己發版

### 3.4 沒有保護的關鍵流程

| 流程 | 壞掉的損失 | 目前的保護 |
|---|---|---|
| **遮蓋**（自動＋手動） | 多遮蓋到自己的棚、漏遮造成版權問題，都是直接播出的內容 | 有單元測試；離線比對靠人工，**沒有固定的 golden 測試** |
| **切段時間** | 段落切錯，播出後要重剪 | 同上 |
| **出片指紋／重出** | 重複出片覆蓋同事檢查過的檔，或不該重出卻沒重出 | 有 anchor 單元測試 |
| **上傳** | 傳錯段或錯標題（固定是私人，不會公開） | 寫死 `privacyStatus: private`（`youtube.rs:575`），這點很好。沒有 dry-run 模式 |
| **共用同步** | 蓋掉同事的文稿 | `sync.rs` 有衝突詢問；core 2 個測試、app 3 個測試 |
| **清空資料夾** | 誤刪 | 用 `trash`（移到垃圾桶），這點很好 |
| **手動遮罩設定** | 存在 localStorage，存失敗不會提示、換電腦不會跟著走 | 無 |
| **發版** | 版本號不一致、BOM、漏帶檔案 | `release.sh` 有檢查，但實際上沒在用 |

---

## 4. 報告

### 4.1 摘要

程式本身狀態不錯：測試 161 個全過，邏輯集中在可以測試的 `core/`，機密沒有進 repo，上傳固定私人、刪檔走垃圾桶。最大的缺口是**最貴的錯**（遮蓋多遮、切段切錯）只靠 AI 每次手動離線比對，沒有固定、可重跑的 golden 測試。最值得處理的三件事：

1. **建立切段＋遮蓋的 golden 回歸測試**：用 11/06、11/09、11/12 的快取跑 `--autocut-batch`，比對段長和遮罩清單。
2. **一個 `check` 指令，CI 也跑同一套**：`cargo test --workspace`＋clippy；JS 至少檢查語法，並擋下 `onX = … && …` 這種寫法。
3. **在 repo 放 `AGENTS.md`**，把散在記憶和 `交接.md` 的規則（寧可少遮、說發版才發版、驗證要附證據）收進來。

### 4.2 發現清單

| # | 問題 | 證據 | 影響 | 建議放在哪一層 | 優先度 |
|---|---|---|---|---|---|
| 1 | 遮蓋和切段沒有自動化的 golden 回歸測試 | 3.3、3.4；每次靠人工比對秒數 | 多遮、切錯直接上播；AI 可能跳過驗證 | 3 測試／驗證 | **高** |
| 2 | CI 只測 core，app 的 11 個測試不會擋發版；註解也過時 | `release.yml:21-23` | 指紋、since 解析壞了照樣發版 | 2 自動檢查（CI 改 `--workspace`） | **高**（改一行） |
| 3 | 沒有單一的 `check` 指令 | 第 2 節 | AI 每次自己決定要跑哪些 | 2 自動檢查 | **高** |
| 4 | JS 完全沒有檢查；`onX = (e) => a && b` 會吃掉按鍵 | `youtube.js:310`（今天的 bug）、`video.js:148` | 前端互動壞掉，只能等使用者發現 | 2 自動檢查（eslint `no-restricted-syntax`，CI 用 node 跑；本機沒 node 就跳過） | 中 |
| 5 | 沒有 `AGENTS.md`；規則散在記憶和未追蹤的 `交接.md` | 第 1 節 | 換 AI、換電腦、換工作階段就忘 | 4 規則檔 | **高**（成本低） |
| 6 | 發版腳本沒在用，手動改版本出過 BOM | 3.1 第 4 項 | 版本號不一致、檔案編碼壞掉 | 1 架構（讓 `release.sh` 忽略未追蹤檔，規定只用它發版） | 中 |
| 7 | 指紋檔寫入失敗被忽略 | `autocut.rs:3115/3155/3381` | 推測：無聲地一直重出 | 3 改成寫 log／report | 低 |
| 8 | 監看中改設定失敗被忽略 | `autocut.js:149/155` | 以為改了其實沒生效 | 3 失敗時顯示訊息 | 中 |
| 9 | 手動遮罩等設定只在 localStorage，存失敗不提示 | `masks.js:18-27`、`bars.js:28`、`autocut.js:528` | 推測：遮罩消失造成漏遮 | 1 架構（統一 helper，失敗提示；或存進成品資料夾） | 中，見 4.3 |
| 10 | localStorage helper 寫了 6 份 | 3.1 第 1 項 | AI 會再抄一份 | 1 架構（併入 #9） | 低 |
| 11 | 門檻數字一部分沒有命名 | `plan.rs:187、342、364、446` | 調門檻時漏改、AI 亂抄 | 4 規則檔（新門檻一律用具名常數＋註解） | 低 |
| 12 | 從沒跑過 rustfmt（645 處） | 第 2 節 | 格式不一；AI 改一處就連帶整段重排，diff 很亂 | 2 自動檢查，但一次全格式化成本高 | 低，見 4.3 |
| 13 | clippy 18 個小警告 | 第 2 節 | 不清的話，`-D warnings` 開不起來 | 2 自動檢查（清掉後開 `-D warnings`） | 低 |
| 14 | 過長的函式、過大的檔案 | 3.2 | AI 改大函式容易漏看前後文 | 5 不處理（拆分本身風險高，等要改那塊時再順手拆） | 低 |

### 4.3 要你決定的問題

1. **golden 測試的樣本放哪？** 側錄快取很大，不能放進 repo。建議固定放在 `D:\kwt\golden\`（1106、1109、1112 各一份快取和預期結果），找不到樣本時測試會跳過並提示。另一台電腦要不要也能跑？
2. **golden 比對到什麼程度？** 建議比對：每段起訖時間（誤差 ±1 格）、遮罩區間清單（多一個就失敗，少一個只警告，符合「寧可少遮」）。這樣可以嗎？
3. **手動遮罩、壓 BAR、串接設定要不要改存到檔案？** 例如當天成品資料夾的 `.autocut\manual.json`，這樣換電腦、清瀏覽器資料都不會不見。還是維持 localStorage，只加上「存失敗要提示」？
4. **rustfmt 要不要一次全格式化？** 一次整理會有很大的 diff，之後 `git blame` 比較難看。選項有三個：
   - (a) 加 `rustfmt.toml`（max_width=140），一次整理完
   - (b) 不管格式，只開 clippy
   - (c) 只格式化新改的檔案
5. **JS 檢查要不要在本機也能跑？** 這台電腦沒有 node。選項有兩個：
   - (a) 只在 CI 跑 eslint，本機不跑
   - (b) 在本機裝 node（約 30MB），`check` 指令也跑得到
6. **`docs/交接.md` 要不要加進 git？** 還是內容併進 `AGENTS.md`、本機那份保持不追蹤？
7. **規則確認**：「說『發版』才發版」「不用 11/10 的資料測試」「畫面一句話、細節放使用說明」這三條要寫進 `AGENTS.md` 嗎？

### 4.4 第 5 階段實作計畫（等你勾選）

| 項目 | 新增／修改的檔案 | 工作量 |
|---|---|---|
| A. CI 改跑 `cargo test --workspace`，修正過時註解 | `.github/workflows/release.yml` | 小 |
| B. `check` 單一指令：test＋clippy（先清掉 18 個警告，再開 `-D warnings`）＋（依 4.3-5）eslint | `scripts/check.ps1`（或 `check.sh`）、`clippy.toml`、`eslint.config.js`、修警告的幾個檔案 | 小～中 |
| C. eslint 規則：禁止 `onX = (e) => a && b`（訊息寫「請改用 addEventListener，回傳 false 會取消預設行為」）、`no-empty`（既有的 catch {} 逐行標「待清，見 AUDIT #8/#9」） | `eslint.config.js`、各 JS 檔標註 | 小 |
| D. 切段＋遮蓋 golden 測試：固定樣本＋預期結果，`cargo test -- --ignored golden` 或 `scripts/golden.ps1` | `app/src-tauri/tests/golden.rs`（或腳本）、`D:\kwt\golden\*`、README 說明 | **中～大** |
| E. `AGENTS.md`＋`CLAUDE.md`（內容只有 `@AGENTS.md`）：功能清單、check 指令、完成前必跑 check 並附證據、寧可少遮、發版規則 | `AGENTS.md`、`CLAUDE.md` | 小 |
| F. 發版只走 `release.sh`：忽略未追蹤檔、發版前跑 check | `scripts/release.sh` | 小 |
| G. 失敗不再無聲：指紋寫入失敗寫 log、改設定失敗顯示訊息、localStorage 統一 helper＋失敗提示（依 4.3-3 可能改存檔案） | `autocut.rs`、`autocut.js`、新 `app/src/store.js`、`masks.js`、`bars.js` 等 | 小～中 |
| H.（選）rustfmt 一次整理 | 全部 `.rs`、`rustfmt.toml` | 小，但 diff 大 |

建議順序：A → E → B/C → F → D → G →（H）。

**到這裡停下，等你確認要做哪幾項、4.3 的問題怎麼決定。**

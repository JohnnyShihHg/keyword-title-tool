const { invoke } = window.__TAURI__.core;

const SEGMENT_NAMES = ["一", "二", "三", "四", "五", "六"];
const DRAFT_KEY = "keyword-title-draft-v1";

const $ = (id) => document.getElementById(id);
const dateInput = $("date");
const output = $("output");
const segmentsEl = $("segments");

/** 每段的 DOM 參照：{ root, text, time, warn } */
const segments = SEGMENT_NAMES.map((name, i) => {
  const root = $("segmentTpl").content.firstElementChild.cloneNode(true);
  root.querySelector(".segment-name").textContent = `第${name}段`;
  const seg = {
    root,
    text: root.querySelector(".text"),
    time: root.querySelector(".time"),
    link: root.querySelector(".link"),
    warn: root.querySelector(".segment-warn"),
  };
  if (i === 0) seg.time.title = "沒填就是 00:00:00";
  root.querySelector(".clear-seg").addEventListener("click", () => {
    seg.text.value = "";
    seg.time.value = "";
    seg.link.value = "";
    changed();
  });
  seg.text.addEventListener("input", changed);
  seg.time.addEventListener("input", changed);
  seg.link.addEventListener("input", changed);
  seg.time.addEventListener("blur", () => normalizeTime(i));
  segmentsEl.appendChild(root);
  return seg;
});

// 全集的 YouTube 連結，放在六段下面
const fullLinkInput = (() => {
  const card = document.createElement("div");
  card.className = "segment";
  card.innerHTML = `<label class="field link-field"><strong>全集</strong> YT 連結
    <input class="link" spellcheck="false" placeholder="全集上傳後貼上 YouTube 連結（會填在檔頭「全集」後面）" /></label>`;
  segmentsEl.appendChild(card);
  const input = card.querySelector("input");
  input.addEventListener("input", changed);
  return input;
})();

function today() {
  const d = new Date();
  const p = (n) => String(n).padStart(2, "0");
  return `${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}`;
}

/** 設定後台的值，啟動時從 Rust 端的 settings.json 載入。 */
let settings = null;

function collect() {
  return {
    date: dateInput.value.trim(),
    segments: segments.map((s) => ({ text: s.text.value, time: s.time.value, link: s.link.value })),
    full_link: fullLinkInput.value,
    settings,
  };
}

// --- 草稿：稿子分批進來（10 點、11 點各一次），關掉重開要還在 ---

function saveDraft() {
  try {
    // 同步狀態也存著：重開後知道上次同步到哪個版本，不會把自己寫的誤認成別人改的
    const { path, mtime, text, dirty, detached } = sync;
    localStorage.setItem(DRAFT_KEY, JSON.stringify({ ...collect(), sync: { path, mtime, text, dirty, detached } }));
  } catch (_) {
    /* 存不了就算了，不影響轉換 */
  }
}

function loadDraft() {
  let draft = null;
  try {
    draft = JSON.parse(localStorage.getItem(DRAFT_KEY) || "null");
  } catch (_) {
    draft = null;
  }
  dateInput.value = draft?.date || today();
  (draft?.segments || []).forEach((s, i) => {
    if (!segments[i]) return;
    segments[i].text.value = s.text || "";
    segments[i].time.value = s.time || "";
    segments[i].link.value = s.link || "";
  });
  fullLinkInput.value = draft?.full_link || "";
  Object.assign(sync, { path: null, mtime: null, text: null, dirty: false, detached: null }, draft?.sync);
}

function hasContent() {
  return segments.some((s) => s.text.value.trim() || s.time.value.trim() || s.link.value.trim()) || !!fullLinkInput.value.trim();
}

/** 把匯入／共用資料夾讀到的內容放進六段（空的就清空）。 */
function applyDoc(doc) {
  segments.forEach((s, i) => {
    const seg = doc.segments[i] ?? { text: "", time: "", link: "" };
    s.text.value = seg.text;
    s.time.value = seg.time;
    s.link.value = seg.link;
  });
  fullLinkInput.value = doc.full_link;
}

// --- 轉換 ---

let lastRendered = null;
let renderTimer = null;
let renderSeq = 0;

/** 使用者改了內容（打字、貼上、匯入、設定）：記成還沒同步，稍後寫回共用資料夾。 */
function changed() {
  sync.rev++;
  sync.dirty = true;
  scheduleSyncWrite();
  updateSyncButtons();
  saveDraft();
  clearTimeout(renderTimer);
  renderTimer = setTimeout(refresh, 120);
}

async function refresh() {
  const seq = ++renderSeq;
  const rendered = await invoke("render_document", { doc: collect() });
  if (seq !== renderSeq) return; // 已有更新的輸入，丟掉舊結果
  lastRendered = rendered;

  // 保留使用者在預覽區的捲動位置
  const scroll = output.scrollTop;
  output.value = rendered.text;
  output.scrollTop = scroll;

  showWarnings(rendered.warnings);
}

function showWarnings(warnings) {
  const list = $("warnings");
  list.replaceChildren(
    ...warnings.map((w) => {
      const li = document.createElement("li");
      li.textContent = w.segment ? `第${SEGMENT_NAMES[w.segment - 1]}段：${w.message}` : w.message;
      return li;
    }),
  );
  $("warnSummary").textContent = warnings.length ? `⚠ ${warnings.length} 項待確認` : "";

  segments.forEach((s, i) => {
    const mine = warnings.filter((w) => w.segment === i + 1);
    s.warn.textContent = mine.length ? `⚠ ${mine.length}` : "";
    s.warn.title = mine.map((w) => w.message).join("\n");
    s.time.classList.toggle("invalid", s.time.value.trim() !== "" && lastRendered?.times[i] == null);
  });
  dateInput.classList.toggle("invalid", warnings.some((w) => w.segment == null));
}

/** 離開時間欄時把 2708、27:08 之類的寫法統一成 00:27:08。 */
async function normalizeTime(i) {
  await refresh();
  const t = lastRendered?.times[i];
  if (t && segments[i].time.value !== t) {
    segments[i].time.value = t;
    saveDraft();
  }
}

// --- 按鈕 ---

function flash(msg) {
  const status = $("status");
  status.textContent = msg;
  clearTimeout(flash.timer);
  flash.timer = setTimeout(() => (status.textContent = ""), 2500);
}

$("copyBtn").addEventListener("click", async () => {
  try {
    await navigator.clipboard.writeText(output.value);
    flash("已複製");
  } catch (e) {
    output.select();
    flash("自動複製失敗，已全選，請按 Ctrl+C");
  }
});

$("saveBtn").addEventListener("click", async () => {
  try {
    // 開了同步就預設開在共用資料夾的當天資料夾；存到那裡就等於建立了今天的 txt，開始自動同步
    const dir = syncOn() && sync.path ? sync.path.replace(/[\\/][^\\/]*$/, "") : null;
    const saved = await invoke("save_text_file", { date: dateInput.value.trim(), content: output.value, dir });
    if (saved) {
      flash("已存檔");
      if (syncOn() && sync.path) await syncAfterSave(saved);
    }
  } catch (e) {
    flash(`存檔失敗：${e}`);
  }
});

$("todayBtn").addEventListener("click", () => {
  dateInput.value = today();
  changed();
  syncOpenSoon();
});

dateInput.addEventListener("input", () => {
  changed();
  syncOpenSoon();
});

// 匯入之前匯出的 txt：日期、六段（由上往下 1–6）、時間、YT 連結；檔案裡空的就留空
$("importBtn").addEventListener("click", async () => {
  const path = await window.__TAURI__.dialog.open({ multiple: false, filters: [{ name: "文字檔", extensions: ["txt"] }] });
  if (!path) return;
  let doc;
  try {
    doc = await invoke("import_text_file", { path, settings });
  } catch (e) {
    $("status").textContent = `⚠ ${e}`;
    return;
  }
  if (hasContent()) {
    const ok = await window.__TAURI__.dialog.ask("匯入會覆蓋目前六段的內容、時間和 YT 連結，確定嗎？", {
      title: "匯入 txt",
      kind: "warning",
      okLabel: "匯入",
      cancelLabel: "取消",
    });
    if (!ok) return;
  }
  applyDoc(doc);
  if (doc.date) dateInput.value = doc.date;
  changed();
  const count = doc.segments.filter((s) => s.text).length;
  $("status").textContent = `已匯入 ${count} 段：${path.split(/[\\/]/).pop()}`;
});

// 清空全部前用系統對話框確認（不用 window.confirm()，它在 WebView 裡會卡住）
$("clearAllBtn").addEventListener("click", async () => {
  // 在影片頁按：清空目前這邊（單集或全集）的影片，不動文稿
  if (!$("page-video").classList.contains("hidden") && window.onClearVideo) return window.onClearVideo();
  const shared = syncOn() ? "\n\n只清空這台電腦畫面上的卡片，共用資料夾的 txt 不會動；之後要按「載入」才會再同步。" : "";
  const ok = await window.__TAURI__.dialog.ask(`確定要清空六段的內容、時間和 YT 連結嗎？${shared}`, {
    title: "清空全部",
    kind: "warning",
    okLabel: "清空",
    cancelLabel: "取消",
  });
  if (!ok) return;
  segments.forEach((s) => {
    s.text.value = "";
    s.time.value = "";
    s.link.value = "";
  });
  fullLinkInput.value = "";
  dateInput.value = today();
  changed();
  await syncDetach();
});

// --- 一次貼上：整批文稿依序放進還空著的段落 ---
// 製作單位 10 點、11 點各給一批，每批都從「一段」開始編號，所以不看段號、只看哪幾段還空著。

const batchPanel = $("batchPanel");
const batchText = $("batchText");
let batchUndo = null;

function openBatch(open) {
  batchPanel.classList.toggle("hidden", !open);
  batchText.value = "";
  if (open) {
    segmentsEl.scrollTop = 0;
    batchText.focus();
  }
}

function showBatchResult(text, undoable) {
  $("batchResultText").textContent = text;
  $("batchUndoBtn").classList.toggle("hidden", !undoable);
  $("batchResult").classList.remove("hidden");
  clearTimeout(showBatchResult.timer);
  showBatchResult.timer = setTimeout(() => {
    $("batchResult").classList.add("hidden");
    batchUndo = null;
  }, 30000);
}

async function applyBatch() {
  const parts = await invoke("split_batch", { text: batchText.value });
  if (!parts.length) return batchText.focus();
  const empty = segments.map((s, i) => (s.text.value.trim() ? -1 : i)).filter((i) => i >= 0);
  if (!empty.length) {
    openBatch(false);
    return showBatchResult("⚠ 六段都已經有內容，沒有空的段落可以放", false);
  }
  batchUndo = segments.map((s) => s.text.value);
  const used = empty.slice(0, parts.length);
  used.forEach((seg, k) => (segments[seg].text.value = parts[k]));
  openBatch(false);
  changed();
  const names = used.map((i) => SEGMENT_NAMES[i]).join("、");
  const extra = parts.length - used.length;
  showBatchResult(`已帶入第${names}段` + (extra > 0 ? `，⚠ 還有 ${extra} 段沒有空位放` : ""), true);
}

$("batchBtn").addEventListener("click", () => openBatch(batchPanel.classList.contains("hidden")));
$("batchCancelBtn").addEventListener("click", () => openBatch(false));
$("batchApplyBtn").addEventListener("click", applyBatch);
// 貼上就自動帶入（等貼上的文字進到輸入框再處理）
batchText.addEventListener("paste", () => setTimeout(applyBatch, 0));
batchText.addEventListener("keydown", (e) => {
  if (e.key === "Escape") openBatch(false);
  if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) applyBatch();
});
$("batchUndoBtn").addEventListener("click", () => {
  if (!batchUndo) return;
  batchUndo.forEach((text, i) => (segments[i].text.value = text));
  batchUndo = null;
  changed();
  showBatchResult("已復原", false);
});

// --- 共用資料夾同步 ---
// 當天資料夾的 txt（路徑見設定）。工具不建資料夾、也不自己建 txt：第一個人按「寫入」或
// 「存成 txt」存到當天資料夾後，大家才開始自動同步。開啟時、開始編輯前看一次有沒有人改過；
// 修改後約 2 秒寫回，寫入前比對內容，別人在我修改時也改過就詢問，不會自動蓋掉別人的。

const sync = {
  path: null, // 目前同步的檔案；null＝關閉或日期不完整
  mtime: null, // 上次讀到／寫入時的修改時間（只用來判斷要不要重讀）
  text: null, // 上次讀到／寫入的內容；null＝還不知道有這個檔案（不會自動寫）
  folder: false, // 當天資料夾在不在
  dirty: false, // 有修改還沒寫回
  detached: null, // 清空全部後暫停同步的檔案路徑（只能按載入恢復）
  rev: 0, // 每次修改 +1，寫入途中又改了就再寫一次
  busy: false,
  conflict: false, // 衝突對話框開著，或選了「先不要」
  writeTimer: null,
  openTimer: null,
};

const syncOn = () => !!settings?.sync_enabled;
const syncWrites = () => syncOn() && settings.sync_auto_write;
const syncLoads = () => syncOn() && settings.sync_auto_load;
const syncPaused = () => !!sync.path && sync.detached === sync.path;
const clock = () => new Date().toLocaleTimeString("zh-TW", { hour12: false });

function setSyncStatus(text, kind = "") {
  const el = $("syncStatus");
  el.textContent = text;
  el.className = `sync-status muted ${kind}`;
  el.title = sync.path || "";
  updateSyncButtons();
}

/**
 * 資料夾還沒建立、畫面是空的、清空後暫停中都不能寫（清空後只能載入）。
 * 「載入」一律可以按：按下去才讀，沒有 txt 會在狀態列說明（不能靠之前讀到的狀態決定，
 * 清空後換了日期或暫停中都不會再自動讀，按鈕會一直灰掉）。
 */
function updateSyncButtons() {
  $("syncBtns").classList.toggle("hidden", !syncOn() || !sync.path);
  const write = $("syncWriteBtn");
  const why = !sync.folder ? "當天資料夾尚未建立" : syncPaused() ? "已清空畫面，只能載入" : !hasContent() ? "畫面上沒有內容可以寫入" : "";
  write.disabled = !!why;
  write.title = why || "把畫面上的內容寫進共用資料夾（還沒有 txt 時會建立）";
}

function syncFailed(e, doing) {
  if (e?.kind === "nofolder") sync.folder = false;
  const msg = {
    unreachable: "⚠ 連不上共用資料夾，先存在這台電腦",
    permission: "⚠ 沒有共用資料夾的權限" + (doing === "write" ? "，無法寫入（仍可載入）" : ""),
    nofolder: "⚠ 當天資料夾尚未建立",
    nofile: "☁ 共用資料夾還沒有今天的 txt，按「寫入」或「存成 txt」建立後開始同步",
    verify: "⚠ 寫入後比對不一致，請檢查網路或手動「存成 txt」",
  }[e?.kind];
  setSyncStatus(msg || `⚠ 同步失敗：${e?.message ?? e}`, e?.kind === "nofile" ? "" : "bad");
}

/** 日期或同步設定改了：換到對應的檔案再讀一次。 */
function syncOpenSoon() {
  clearTimeout(sync.openTimer);
  sync.openTimer = setTimeout(syncOpen, 600);
}

async function syncOpen() {
  if (!syncOn()) {
    sync.path = null;
    return setSyncStatus("");
  }
  const path = await invoke("sync_path", { settings, date: dateInput.value.trim() });
  if (path !== sync.path) {
    Object.assign(sync, { path, mtime: null, text: null, folder: false, conflict: false });
  }
  if (!path) return setSyncStatus("⚠ 日期不完整，暫停同步", "bad");
  await syncPull({ load: syncLoads() });
}

/** 讀共用資料夾。load：別人改過且自己沒在改就載入（雙方都改過就詢問）；false 只更新狀態。 */
async function syncPull({ load = true } = {}) {
  if (!sync.path || sync.busy) return;
  sync.busy = true;
  const rev = sync.rev;
  try {
    const f = await invoke("sync_read", { path: sync.path });
    sync.folder = f.folder;
    if (!f.exists) {
      sync.mtime = null;
      sync.text = null;
      return syncFailed({ kind: f.folder ? "nofile" : "nofolder" });
    }
    if (syncPaused()) return setSyncStatus("⏸ 已清空畫面，自動同步暫停（按「載入」恢復）");
    // 檔案內容就是畫面上這份（例如剛「存成 txt」存到這裡）：直接接上
    const mine = (await invoke("render_document", { doc: collect() })).text.replace(/\r?\n/g, "\r\n");
    if (f.text === sync.text || f.text === mine) {
      sync.text = f.text;
      sync.mtime = f.mtime;
      if (f.text === mine && rev === sync.rev) sync.dirty = false;
      if (sync.dirty) scheduleSyncWrite();
      else setSyncStatus(`☁ 已同步 ${clock()}`, "ok");
      saveDraft();
    } else if (!load) {
      setSyncStatus("☁ 共用資料夾有更新的版本（自動載入已關閉，可按「載入」）");
    } else if ((!sync.dirty || !hasContent()) && rev === sync.rev) {
      await loadSynced(f);
    } else {
      showConflict(f);
    }
  } catch (e) {
    syncFailed(e, "read");
  } finally {
    sync.busy = false;
  }
}

async function loadSynced(f) {
  const doc = await invoke("import_text", { text: f.text, settings });
  applyDoc(doc);
  Object.assign(sync, { text: f.text, mtime: f.mtime, dirty: false, conflict: false, detached: null });
  saveDraft();
  refresh();
  const count = doc.segments.filter((s) => s.text).length;
  setSyncStatus(`☁ 已載入共用資料夾的版本（${count} 段）${clock()}`, "ok");
}

function cancelSyncWrite() {
  clearTimeout(sync.writeTimer);
  sync.writeTimer = null;
}

/** 自動寫回：只在已經有今天的 txt、沒有暫停、沒有衝突時。 */
function scheduleSyncWrite() {
  cancelSyncWrite();
  if (!syncWrites() || !sync.path || sync.text === null || sync.conflict || syncPaused()) return;
  sync.writeTimer = setTimeout(() => syncPush(), 2000);
}

/** create：還沒有 txt 時建立（只有按「寫入」才會）。force：衝突時選了用自己的蓋過去。 */
async function syncPush({ force = false, create = false } = {}) {
  cancelSyncWrite();
  if (!sync.path || !hasContent()) return false; // 空白不寫，免得把共用資料夾的稿子洗掉
  if (sync.busy) {
    scheduleSyncWrite();
    return false;
  }
  sync.busy = true;
  const rev = sync.rev;
  setSyncStatus("☁ 寫入中…");
  try {
    const rendered = await invoke("render_document", { doc: collect() });
    const w = await invoke("sync_write", { path: sync.path, content: rendered.text, known: sync.text, force, create });
    Object.assign(sync, { mtime: w.mtime, text: w.text, conflict: false, detached: null, folder: true });
    if (rev === sync.rev) sync.dirty = false;
    else scheduleSyncWrite();
    saveDraft();
    setSyncStatus(`☁ 已同步 ${clock()}（已讀回確認）`, "ok");
    return true;
  } catch (e) {
    if (e?.kind === "conflict") {
      sync.busy = false;
      const f = await invoke("sync_read", { path: sync.path }).catch(() => null);
      if (f?.exists) showConflict(f);
      return false;
    }
    syncFailed(e, "write");
    return false;
  } finally {
    sync.busy = false;
  }
}

/** 「存成 txt」剛好存到共用資料夾的今天的檔案：就當作建立了，開始同步。 */
async function syncAfterSave(savedPath) {
  const norm = (p) => p.replace(/\\/g, "/").toLowerCase();
  if (norm(savedPath) !== norm(sync.path)) return;
  sync.detached = null;
  sync.rev++;
  await syncPull({ load: false });
}

/** 清空全部：只清畫面，這個檔案暫停同步，不會把空白寫回共用資料夾。 */
async function syncDetach() {
  cancelSyncWrite();
  if (!syncOn()) return;
  const path = await invoke("sync_path", { settings, date: dateInput.value.trim() });
  if (path !== sync.path) Object.assign(sync, { path, mtime: null, text: null, conflict: false });
  Object.assign(sync, { detached: path, dirty: false });
  saveDraft();
  if (sync.path) setSyncStatus("⏸ 已清空畫面，自動同步暫停（按「載入」恢復）");
}

let conflictFile = null;
function showConflict(f) {
  conflictFile = f;
  sync.conflict = true;
  cancelSyncWrite();
  $("syncConflictMessage").textContent =
    `共用資料夾的 ${sync.path.split(/[\\/]/).pop()} 和你畫面上的內容不一樣（有人改過）。` +
    "「載入對方的版本」會丟掉你這次的修改；「用我的蓋過去」會丟掉對方的修改。";
  $("syncConflict").classList.remove("hidden");
  setSyncStatus("⚠ 和共用資料夾的版本不一樣，尚未同步", "bad");
}

function closeConflict() {
  $("syncConflict").classList.add("hidden");
}

$("conflictTheirsBtn").addEventListener("click", async () => {
  closeConflict();
  await loadSynced(conflictFile);
});
$("conflictMineBtn").addEventListener("click", async () => {
  closeConflict();
  sync.conflict = false;
  await syncPush({ force: true });
});
$("conflictLaterBtn").addEventListener("click", () => {
  // 保持暫停寫入；按「載入」或「寫入」再處理
  closeConflict();
});

$("syncLoadBtn").addEventListener("click", async () => {
  if (!sync.path) return;
  if (sync.busy) return setSyncStatus("☁ 正在讀取共用資料夾，請稍候再按一次");
  sync.busy = true;
  setSyncStatus("☁ 讀取中…");
  try {
    const f = await invoke("sync_read", { path: sync.path });
    sync.folder = f.folder;
    if (!f.exists) return syncFailed({ kind: f.folder ? "nofile" : "nofolder" });
    if (hasContent() && f.text !== sync.text) {
      const ok = await window.__TAURI__.dialog.ask("用共用資料夾裡的 txt 取代畫面上的內容嗎？", {
        title: "載入",
        okLabel: "載入",
        cancelLabel: "取消",
      });
      if (!ok) return setSyncStatus(syncPaused() ? "⏸ 已清空畫面，自動同步暫停（按「載入」恢復）" : "已取消載入");
    }
    await loadSynced(f);
  } catch (e) {
    syncFailed(e, "read");
  } finally {
    sync.busy = false;
  }
});

$("syncWriteBtn").addEventListener("click", async () => {
  if (syncPaused() || !hasContent()) return;
  sync.conflict = false;
  await syncPush({ create: true });
});

// 開始編輯前（點進任何一段）看一次別人有沒有改過，自己沒在改就先載入，避免之後衝突
let lastFocusCheck = 0;
segmentsEl.addEventListener("focusin", async () => {
  if (!syncLoads() || !sync.path || sync.busy || sync.conflict || sync.dirty || syncPaused()) return;
  if (Date.now() - lastFocusCheck < 5000) return;
  lastFocusCheck = Date.now();
  try {
    const m = await invoke("sync_stat", { path: sync.path });
    if ((m ?? null) !== sync.mtime) await syncPull();
  } catch (e) {
    syncFailed(e, "read");
  }
});

/** 開啟時：開了自動載入就換成今天的檔案（前一天還沒寫回的先寫回）。 */
async function syncStartup() {
  if (!syncOn()) return;
  if (syncLoads() && dateInput.value.trim() !== today()) {
    if (sync.dirty && hasContent() && sync.text !== null && !syncPaused()) {
      sync.path = await invoke("sync_path", { settings, date: dateInput.value.trim() });
      if (!(await syncPush())) {
        // 寫不回去就保留前一天的內容，免得遺失
        return setSyncStatus(`⚠ ${dateInput.value} 的修改還沒寫回共用資料夾，先保留在這裡`, "bad");
      }
    }
    applyDoc({ segments: [], full_link: "" });
    dateInput.value = today();
    sync.dirty = false;
    saveDraft();
    refresh();
  }
  await syncOpen();
}

// 關視窗前把還沒寫回的修改寫完（Rust 端最多等 5 秒）
window.__TAURI__.event.listen("app-close-requested", async () => {
  try {
    if (sync.dirty && sync.writeTimer) {
      await Promise.race([syncPush(), new Promise((r) => setTimeout(r, 4000))]);
    }
  } finally {
    invoke("finish_close");
  }
});

// --- 分頁 ---

document.querySelectorAll(".tab").forEach((tab) => {
  tab.addEventListener("click", () => {
    document.querySelectorAll(".tab").forEach((t) => t.classList.toggle("active", t === tab));
    document.querySelectorAll(".page").forEach((p) => p.classList.toggle("hidden", p.id !== `page-${tab.dataset.page}`));
  });
});

// --- 設定後台 ---

const settingsForm = $("settingsForm");

// 配色主題：id 對應 styles.css 的 [data-theme]
const THEMES = [
  { id: "light", name: "淺色（預設）" },
  { id: "dark", name: "深色剪輯風" },
  { id: "paper", name: "護眼米色" },
  { id: "green", name: "豆沙綠" },
  { id: "slate", name: "深藍灰" },
  { id: "contrast", name: "高對比" },
];

function applyTheme(id) {
  document.documentElement.dataset.theme = THEMES.some((t) => t.id === id) ? id : "light";
}

// 每個選項自己套上該主題的 data-theme，色塊直接用主題的色票畫出來
$("themeList").replaceChildren(
  ...THEMES.map((t) => {
    const label = document.createElement("label");
    label.className = "theme-option";
    label.innerHTML = `<input type="radio" name="theme" value="${t.id}" />
      <span class="theme-swatch" data-theme="${t.id}" style="background:var(--panel);color:var(--text)">關鍵 Aa</span>
      <span>${t.name}</span>`;
    return label;
  }),
);

function fillSettingsForm() {
  for (const [key, value] of Object.entries(settings)) {
    const input = settingsForm.elements[key];
    if (!input) continue;
    if (input.type === "checkbox") input.checked = value;
    else input.value = value;
  }
  applyTheme(settings.theme);
  showSettingsExample();
}

function syncSettingsKey(s) {
  return JSON.stringify([s.sync_enabled, s.sync_root, s.sync_folder_format, s.sync_auto_load, s.sync_auto_write]);
}

function readSettingsForm() {
  const f = settingsForm.elements;
  const max = parseInt(f.max_speaker_chars.value, 10);
  return {
    host: f.host.value.trim() || settings.host,
    program_tag: f.program_tag.value.trim() || settings.program_tag,
    label_segments: f.label_segments.checked,
    number_line: f.number_line.checked,
    show_header: f.show_header.checked,
    halfwidth_timestamps: f.halfwidth_timestamps.checked,
    max_speaker_chars: Number.isFinite(max) && max > 0 ? max : settings.max_speaker_chars,
    theme: f.theme.value || settings.theme,
    promo_20_path: f.promo_20_path.value,
    promo_45_path: f.promo_45_path.value,
    audio_gain_db: Number.isFinite(parseFloat(f.audio_gain_db.value))
      ? Math.min(20, Math.max(-20, parseFloat(f.audio_gain_db.value)))
      : settings.audio_gain_db,
    transition_secs: Math.min(3, Math.max(0.1, parseFloat(f.transition_secs.value) || settings.transition_secs)),
    promo_tail_adjust_secs: Number.isFinite(parseFloat(f.promo_tail_adjust_secs.value))
      ? Math.min(10, Math.max(-10, parseFloat(f.promo_tail_adjust_secs.value)))
      : settings.promo_tail_adjust_secs,
    notify_popup: f.notify_popup.checked,
    notify_sound: f.notify_sound.checked,
    notify_sound_id: f.notify_sound_id.value || settings.notify_sound_id,
    notify_volume: Math.min(100, Math.max(0, parseFloat(f.notify_volume.value))) || 0,
    output_dir: f.output_dir.value,
    encoder: f.encoder.value || settings.encoder,
    sync_enabled: f.sync_enabled.checked,
    sync_root: f.sync_root.value.trim(),
    sync_folder_format: f.sync_folder_format.value.trim() || settings.sync_folder_format,
    sync_auto_load: f.sync_auto_load.checked,
    sync_auto_write: f.sync_auto_write.checked,
  };
}

async function showSettingsExample() {
  const date = dateInput.value.trim() || today();
  $("settingsExample").textContent = `範例：段標題${settings.program_tag}${date}-1${settings.host}`;
  // 關閉同步時下面的選項變灰、不能改
  $("syncOpts").disabled = !settings.sync_enabled;
  const path = await invoke("sync_path", { settings, date });
  $("syncExample").textContent = path ? `今天的檔案：${path}` : "⚠ 根目錄或資料夾格式不完整";
}

let settingsTimer = null;
settingsForm.addEventListener("input", () => {
  const before = syncSettingsKey(settings);
  settings = readSettingsForm();
  if (syncSettingsKey(settings) !== before) syncOpenSoon();
  applyTheme(settings.theme);
  showSettingsExample();
  window.onSettingsChanged?.();
  changed();
  clearTimeout(settingsTimer);
  settingsTimer = setTimeout(persistSettings, 300);
});

/** 檔案選擇器改了唯讀欄位後呼叫：input 事件不會自己觸發。 */
function settingsFieldChanged() {
  settingsForm.dispatchEvent(new Event("input"));
}

async function persistSettings() {
  try {
    await invoke("save_settings", { settings });
    $("settingsStatus").textContent = "已儲存";
  } catch (e) {
    $("settingsStatus").textContent = `儲存失敗：${e}`;
  }
}

$("resetSettingsBtn").addEventListener("click", async () => {
  settings = await invoke("default_settings");
  fillSettingsForm();
  changed();
  syncOpenSoon();
  await persistSettings();
  $("settingsStatus").textContent = "已還原預設";
});

// --- 版本與自動更新 ---

const tauriUpdater = window.__TAURI__.updater;
let pendingUpdate = null;

function setUpdateStatus(text) {
  $("updateStatus").textContent = text;
}

async function showCurrentVersion() {
  try {
    $("versionLabel").textContent = `目前版本 v${await window.__TAURI__.app.getVersion()}`;
  } catch (_) {
    $("versionLabel").textContent = "";
  }
}

/** silent：開啟時的自動檢查，沒新版或沒網路都不出聲。 */
async function checkForUpdate({ silent }) {
  if (!silent) setUpdateStatus("檢查中…");
  let update;
  try {
    update = await tauriUpdater.check();
  } catch (e) {
    if (!silent) setUpdateStatus(`⚠ 檢查更新失敗：${e}`);
    return;
  }
  pendingUpdate = update;
  const badge = $("updateBadge");
  if (!update) {
    badge.classList.add("hidden");
    $("installUpdateBtn").classList.add("hidden");
    if (!silent) setUpdateStatus("已經是最新版本。");
    return;
  }
  badge.textContent = `有新版本 v${update.version}`;
  badge.classList.remove("hidden");
  $("installUpdateBtn").classList.remove("hidden");
  setUpdateStatus(`有新版本 v${update.version}` + (releaseSummary(update.body) ? `：${releaseSummary(update.body)}` : ""));
}

/** 更新說明的第一行實際內容（跳過「## 更新內容」標題、HTML 註解、分隔線）。 */
function releaseSummary(body) {
  return (body || "")
    .replace(/<!--[\s\S]*?-->/g, "")
    .split("\n")
    .map((l) => l.trim())
    .find((l) => l && !l.startsWith("#") && !/^-{3,}$/.test(l)) || "";
}

async function installUpdate() {
  if (!pendingUpdate) return;
  const progress = $("updateProgress");
  const btn = $("installUpdateBtn");
  btn.disabled = true;
  progress.classList.remove("hidden");
  let downloaded = 0;
  let total = 0;
  try {
    await pendingUpdate.downloadAndInstall((event) => {
      if (event.event === "Started") {
        total = event.data.contentLength ?? 0;
        setUpdateStatus("下載更新中…");
      } else if (event.event === "Progress") {
        downloaded += event.data.chunkLength ?? 0;
        if (total) {
          progress.value = downloaded / total;
          setUpdateStatus(`下載更新 ${Math.round((downloaded / total) * 100)}%`);
        }
      } else if (event.event === "Finished") {
        progress.value = 1;
        setUpdateStatus("安裝中…");
      }
    });
    setUpdateStatus("更新完成，即將重新啟動…");
    await window.__TAURI__.process.relaunch();
  } catch (e) {
    setUpdateStatus(`⚠ 更新失敗：${e}`);
    btn.disabled = false;
    progress.classList.add("hidden");
  }
}

$("checkUpdateBtn").addEventListener("click", () => checkForUpdate({ silent: false }));
$("installUpdateBtn").addEventListener("click", installUpdate);
$("updateBadge").addEventListener("click", () => {
  document.querySelector('.tab[data-page="settings"]').click();
});

(async () => {
  showCurrentVersion();
  checkForUpdate({ silent: true });
  settings = await invoke("get_settings");
  fillSettingsForm();
  loadDraft();
  refresh();
  window.onSettingsChanged?.();
  syncStartup();
})();

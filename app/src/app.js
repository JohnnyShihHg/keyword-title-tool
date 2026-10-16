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
    warn: root.querySelector(".segment-warn"),
  };
  if (i === 0) seg.time.title = "沒填就是 00:00:00";
  root.querySelector(".clear-seg").addEventListener("click", () => {
    seg.text.value = "";
    seg.time.value = "";
    changed();
  });
  seg.text.addEventListener("input", changed);
  seg.time.addEventListener("input", changed);
  seg.time.addEventListener("blur", () => normalizeTime(i));
  segmentsEl.appendChild(root);
  return seg;
});

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
    segments: segments.map((s) => ({ text: s.text.value, time: s.time.value })),
    settings,
  };
}

// --- 草稿：稿子分批進來（10 點、11 點各一次），關掉重開要還在 ---

function saveDraft() {
  try {
    localStorage.setItem(DRAFT_KEY, JSON.stringify(collect()));
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
  });
}

// --- 轉換 ---

let lastRendered = null;
let renderTimer = null;
let renderSeq = 0;

function changed() {
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
    const saved = await invoke("save_text_file", { date: dateInput.value.trim(), content: output.value });
    if (saved) flash("已存檔");
  } catch (e) {
    flash(`存檔失敗：${e}`);
  }
});

$("todayBtn").addEventListener("click", () => {
  dateInput.value = today();
  changed();
});

dateInput.addEventListener("input", changed);

// 清空全部前用系統對話框確認（不用 window.confirm()，它在 WebView 裡會卡住）
$("clearAllBtn").addEventListener("click", async () => {
  const ok = await window.__TAURI__.dialog.ask("確定要清空六段的內容和時間嗎？", {
    title: "清空全部",
    kind: "warning",
    okLabel: "清空",
    cancelLabel: "取消",
  });
  if (!ok) return;
  segments.forEach((s) => {
    s.text.value = "";
    s.time.value = "";
  });
  dateInput.value = today();
  changed();
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
  { id: "light", name: "白底黑字" },
  { id: "dark", name: "黑底白字" },
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

function readSettingsForm() {
  const f = settingsForm.elements;
  const max = parseInt(f.max_speaker_chars.value, 10);
  return {
    host: f.host.value.trim() || settings.host,
    program_tag: f.program_tag.value.trim() || settings.program_tag,
    label_segments: f.label_segments.checked,
    number_line: f.number_line.checked,
    include_header: f.include_header.checked,
    max_speaker_chars: Number.isFinite(max) && max > 0 ? max : settings.max_speaker_chars,
    theme: f.theme.value || settings.theme,
    promo_20_path: f.promo_20_path.value,
    promo_45_path: f.promo_45_path.value,
    audio_gain_db: Number.isFinite(parseFloat(f.audio_gain_db.value))
      ? Math.min(20, Math.max(-20, parseFloat(f.audio_gain_db.value)))
      : settings.audio_gain_db,
    transition_secs: Math.min(3, Math.max(0.1, parseFloat(f.transition_secs.value) || settings.transition_secs)),
    output_dir: f.output_dir.value,
    encoder: f.encoder.value || settings.encoder,
  };
}

function showSettingsExample() {
  const date = dateInput.value.trim() || today();
  $("settingsExample").textContent = `範例：段標題${settings.program_tag}${date}-1${settings.host}`;
}

let settingsTimer = null;
settingsForm.addEventListener("input", () => {
  settings = readSettingsForm();
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
})();

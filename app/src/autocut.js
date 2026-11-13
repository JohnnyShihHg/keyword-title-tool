// 自動切段頁：選資料夾、開始／停止監看、段落狀態表、側錄檔清單、各階段耗時、紀錄。
// 判斷與出片都在 Rust 端（autocut.rs）；這裡只顯示 autocut-status 事件。
// 和 app.js／video.js 共用全域的 invoke、$、settings、listen、dialog、showNotice、
// collect、dateInput、persistSettings、fmtTime。

const AC_STATES = {
  waiting: ["等待", ""],
  recording: ["錄製中", ""],
  seeking: ["往前找開頭", ""],
  complete: ["完整", "ok"],
  queued: ["排隊出片", "ok"],
  rendering: ["出片中", "ok"],
  done: ["完成", "done"],
  incomplete: ["不完整", "bad"],
  failed: ["失敗", "bad"],
};
const AC_FILE_STATES = { recording: "錄製中", waiting: "等待", scanning: "掃描中…", ocr: "OCR…", done: "已掃描", error: "★ 失敗" };

let acStatus = null;
let acTitlesTimer = null;

function acOutputDir() {
  return settings.autocut_output_dir || settings.output_dir || "";
}

/** 側錄資料夾最後一層是日期（2026_10_26）時，換成文稿頁的日期，不用每天重選 */
function acWatchDir() {
  const dir = settings.autocut_watch_dir || "";
  const date = dateInput.value.trim();
  if (!/^\d{8}$/.test(date)) return dir;
  const day = `${date.slice(0, 4)}_${date.slice(4, 6)}_${date.slice(6)}`;
  return dir.replace(/(^|[\\/])\d{4}_\d{2}_\d{2}([\\/]?)$/, `$1${day}$2`);
}

function acFillDirs() {
  $("acWatchDir").value = acWatchDir();
  $("acStillsDir").value = settings.autocut_stills_dir || "";
  if (typeof acLoadStills === "function") acLoadStills(); // 替換畫面（stills.js）跟著換資料夾
  $("acOutputDir").value = settings.autocut_output_dir || "";
  $("acOutputDir").placeholder = settings.output_dir ? `空白＝沿用 ${settings.output_dir}` : "尚未指定";
  $("acAutoRender").checked = settings.autocut_auto_render !== false;
  $("acMakeAll").checked = settings.autocut_make_all !== false;
  $("acCopyShare").checked = !!settings.autocut_copy_to_share;
  $("acSince").value = settings.autocut_since ?? "";
  $("acEndCutoff").value = settings.autocut_end_cutoff ?? "";
  $("acRotateSecs").value = settings.autocut_rotate_secs ?? 7;
  $("acTitlesSource").textContent = settings.sync_enabled
    ? "文稿：共用資料夾當天的 txt"
    : "文稿：文稿頁目前的內容（開播後改標題也會跟著更新）";
}

document.querySelectorAll("[data-ac-pick]").forEach((btn) => {
  btn.addEventListener("click", async () => {
    const dir = await dialog.open({ directory: true, multiple: false });
    if (!dir) return;
    settings[btn.dataset.acPick] = dir;
    acFillDirs();
    persistSettings();
  });
});
// 清空：資料夾裡的東西全部移到垃圾桶（資料夾本身和欄位保留）。側錄資料夾、網路磁碟、監看中由 Rust 端擋掉
document.querySelectorAll("[data-ac-clear]").forEach((btn) => {
  btn.addEventListener("click", async () => {
    const dir = settings[btn.dataset.acClear];
    const what = btn.dataset.acClear === "autocut_stills_dir" ? "靜圖資料夾" : "成品資料夾";
    if (!dir) return acMessage(`還沒選${what}`);
    const ok = await dialog.ask(`要把${what}「${dir}」裡的檔案和子資料夾全部移到垃圾桶嗎？\n資料夾本身會留著，放錯可以去垃圾桶還原。`, {
      title: `清空${what}`,
      kind: "warning",
      okLabel: "移到垃圾桶",
      cancelLabel: "取消",
    });
    if (!ok) return;
    try {
      const n = await invoke("clear_folder", { dir, watchDir: settings.autocut_watch_dir || "" });
      acMessage(n ? `已把${what}裡的 ${n} 項移到垃圾桶` : `${what}本來就是空的`);
      if (btn.dataset.acClear === "autocut_stills_dir") window.acLoadStills?.();
    } catch (e) {
      acMessage(`沒有清空：${e}`);
    }
  });
});
/**
 * 時間欄只打數字時換成有冒號的寫法（只用在要填時間的欄位）：
 *   clock（從幾點開始看、最晚幾點播完）：2300→23:00、930→09:30、235930→23:59:30
 *   pos（影片裡的時間：壓 BAR、手動遮罩）：1601→16:01、130→1:30、13025→1:30:25、1601.5→16:01.5；一兩位數＝秒，照舊
 *   tc（手動遮罩）：另外 8 位數＝剪輯時間碼（掉格）00160120→00;16;01;20
 * 已經有冒號、分號或其他字的不動
 */
function acDigitsTime(text, kind) {
  const raw = String(text ?? "");
  const t = raw.trim().replace(/[\uFF10-\uFF19\uFF0E]/g, (c) => String.fromCharCode(c.charCodeAt(0) - 0xfee0));
  const m = t.match(/^(\d+)(\.\d+)?$/);
  if (!m) return raw.trim();
  const [, d, frac = ""] = m;
  if (kind === "clock") {
    if (frac) return t;
    if (d.length === 3 || d.length === 4) {
      const x = d.padStart(4, "0");
      return `${x.slice(0, 2)}:${x.slice(2)}`;
    }
    if (d.length === 5 || d.length === 6) {
      const x = d.padStart(6, "0");
      return `${x.slice(0, 2)}:${x.slice(2, 4)}:${x.slice(4)}`;
    }
    return t;
  }
  if (kind === "tc" && d.length === 8 && !frac) return `${d.slice(0, 2)};${d.slice(2, 4)};${d.slice(4, 6)};${d.slice(6)}`;
  if (d.length <= 2) return t;
  if (d.length <= 4) {
    const x = d.padStart(4, "0");
    return `${Number(x.slice(0, 2))}:${x.slice(2)}${frac}`;
  }
  if (d.length <= 6) {
    const x = d.padStart(6, "0");
    return `${Number(x.slice(0, 2))}:${x.slice(2, 4)}:${x.slice(4)}${frac}`;
  }
  return t;
}

/** 「1601-1610, 1830-1845」這種時段清單：每個時間各自換（分隔的逗號、橫線照原樣） */
function acDigitsRanges(text, kind) {
  return String(text ?? "")
    .split(/([,，、\n])/)
    .map((part, i) => (i % 2 ? part : part.split(/(\s*[-~～–]\s*)/).map((x, j) => (j % 2 || !x.trim() ? x : acDigitsTime(x, kind))).join("")))
    .join("");
}

$("acEndCutoff").addEventListener("change", () => {
  $("acEndCutoff").value = acDigitsTime($("acEndCutoff").value, "clock");
  settings.autocut_end_cutoff = $("acEndCutoff").value.trim();
  persistSettings();
  acPushLive();
});
$("acRotateSecs").addEventListener("change", () => {
  settings.autocut_rotate_secs = Math.max(0, Number($("acRotateSecs").value) || 0);
  persistSettings();
  acPushLive();
});
$("acSince").addEventListener("change", () => {
  $("acSince").value = acDigitsTime($("acSince").value, "clock");
  settings.autocut_since = $("acSince").value.trim();
  persistSettings();
  acPushLive();
});
$("acAutoRender").addEventListener("change", () => {
  settings.autocut_auto_render = $("acAutoRender").checked;
  persistSettings();
  invoke("autocut_set_options", { autoRender: $("acAutoRender").checked }).catch(() => {});
});
// 監看中取消勾選也要馬上生效（以前只在開始監看時讀一次，取消了照樣合成全集）
$("acMakeAll").addEventListener("change", () => {
  settings.autocut_make_all = $("acMakeAll").checked;
  persistSettings();
  invoke("autocut_set_options", { makeAll: $("acMakeAll").checked }).catch(() => {});
});
$("acCopyShare").addEventListener("change", () => {
  settings.autocut_copy_to_share = $("acCopyShare").checked;
  persistSettings();
  if (acStatus) acShow(acStatus);
});

// ───────── 成品複製到當天的共用資料夾（{日期}-{段}.mp4，給人檢查） ─────────
// 每個成品這次開程式只複製一次；目的地已有同名檔就不覆蓋（Rust 端判斷）。
// 例外：這次開程式裡剛出好（含按「重新出片」）的成品，要覆蓋共用資料夾裡的舊檔

let acShare = {}; // 段號 → { state, progress, dest, error }
const acShareTried = new Set();
const acShareFresh = new Set(); // 剛出好的成品路徑：複製時覆蓋同名舊檔

/** 這段要複製的成品：有串接就等串接完成，沒有串接用正片 */
function acShareSource(s) {
  if (s.chain) return s.chain.state === "done" ? s.chain.output : "";
  return s.state === "done" && !s.stale ? s.output : "";
}

function acCopyToShare(st) {
  if (!settings?.autocut_copy_to_share) return;
  const date = dateInput.value.trim();
  if (!/^\d{8}$/.test(date)) return;
  for (const s of st.segments) {
    const src = acShareSource(s);
    if (!src || acShareTried.has(src)) continue;
    acShareTried.add(src);
    const overwrite = acShareFresh.delete(src);
    acShare[s.n] = { state: "copying", progress: 0 };
    invoke("autocut_copy_to_share", { settings, date, n: s.n, src, overwrite }).catch((e) => {
      acShare[s.n] = { state: "failed", error: String(e) };
      acShow(acStatus);
    });
  }
}

function acShareCell(td, n) {
  const c = acShare[n];
  if (!c) return;
  const [label, cls] = {
    copying: [`複製中 ${Math.round((c.progress || 0) * 100)}%`, "ok"],
    done: ["已複製", "done"],
    skipped: ["已有同名檔，未複製", ""],
    failed: ["複製失敗", "bad"],
  }[c.state] || [c.state, ""];
  const pill = document.createElement("span");
  acPill(pill, label, cls, c.state === "copying" ? c.progress || 0 : null);
  pill.title = c.error || c.dest || "";
  td.append(pill);
  if (c.dest && (c.state === "done" || c.state === "skipped")) {
    const open = Object.assign(document.createElement("button"), { type: "button", className: "ghost small", textContent: "開啟" });
    open.onclick = () => invoke("autocut_reveal", { path: c.dest });
    td.append(open);
  }
}

listen("share-copy", (e) => {
  acShare[e.payload.n] = e.payload;
  if (e.payload.state === "done") showNotice({ title: `第 ${e.payload.n} 段已複製到共用資料夾`, message: e.payload.dest });
  if (acStatus) acShow(acStatus);
});

/** 文稿頁目前的內容（跟存成 txt 一樣的格式） */
async function acCurrentTitles() {
  return (await invoke("render_document", { doc: collect() })).text;
}

$("acStartBtn").addEventListener("click", async () => {
  if (!settings.autocut_watch_dir) return acMessage("請先選擇側錄資料夾");
  if (!acOutputDir()) return acMessage("請先選擇成品資料夾");
  const date = dateInput.value.trim();
  let titles;
  if (settings.sync_enabled && /^\d{8}$/.test(date)) titles = { kind: "Sync", value: date };
  else titles = { kind: "Text", value: await acCurrentTitles() };
  try {
    await invoke("autocut_start", {
      config: {
        watch_dir: acWatchDir(),
        stills_dir: settings.autocut_stills_dir,
        output_dir: acOutputDir(),
        titles,
        auto_render: settings.autocut_auto_render !== false,
        make_all: $("acMakeAll").checked,
        // 開始時間、AI、正片加工、串接、全集用的設定（監看中改了由 acPushLive 再送）
        ...(await acLiveOptions(date)),
        date_tag: /^\d{8}$/.test(date) ? date.slice(4) : "",
        chains: Object.fromEntries(
          [1, 2, 3, 4, 5, 6].map((n) => [n, acChainPayload(n)]).filter(([, c]) => c),
        ),
        bars: acBarsConfig(),
        manual_masks: acMasksConfig(),
      },
    });
    acRunDate = date;
    $("acLog").textContent = "";
    acMessage("");
    // 文稿頁的內容每 15 秒送一次（只有不用共用資料夾時才需要）
    clearInterval(acTitlesTimer);
    if (titles.kind === "Text") {
      acTitlesTimer = setInterval(async () => invoke("autocut_set_titles", { text: await acCurrentTitles() }), 15000);
    }
    acShow(await invoke("autocut_status"));
  } catch (e) {
    acMessage(`無法開始：${e}`);
  }
});

$("acStopBtn").addEventListener("click", () => invoke("autocut_stop"));

$("acReportBtn").addEventListener("click", async () => {
  const text = await invoke("autocut_report");
  $("acLog").textContent = text || "還沒有報告（開始監看、掃完第一個檔後才有）";
});

$("acOpenBtn").addEventListener("click", () => {
  const dir = acOutputDir();
  if (!dir) return acMessage("還沒選成品資料夾");
  invoke("autocut_reveal", { path: dir }).catch((e) => acMessage(String(e)));
});

function acMessage(text) {
  $("acMessage").textContent = text;
}

function acTime(sec) {
  return sec == null ? "…" : fmtTime(sec);
}

/** 窄視窗表格變卡片時，每格上面的小字欄名（取自表頭） */
function acLabelCells(tbody) {
  const heads = [...(tbody.closest("table")?.tHead?.rows[0]?.cells || [])].map((c) => c.textContent);
  for (const tr of tbody.rows) [...tr.cells].forEach((td, i) => (td.dataset.label = td.colSpan > 1 ? "" : heads[i] ?? ""));
}

/** 狀態膠囊；progress（0–1）有值時把進度填進膠囊 */
function acPill(el, label, cls, progress = null) {
  el.className = `ac-state ${cls}${progress != null ? " prog" : ""}`;
  el.textContent = label;
  if (progress != null) el.style.setProperty("--p", progress);
  else el.style.removeProperty("--p");
}

function acShow(st) {
  acStatus = st;
  $("acStartBtn").classList.toggle("hidden", st.running);
  $("acStopBtn").classList.toggle("hidden", !st.running);
  $("acRunState").classList.toggle("on", st.running);
  $("acRunState").lastElementChild.textContent = st.running ? "監看中" : "已停止";
  // 監看中鎖住資料夾和文稿日期：換了等於換一集，正在跑的那一輪不會跟著換（換日期還會把別天的遮罩、BAR 送進來）
  const lockTip = st.running ? "監看中不能改，請先停止監看" : "";
  document.querySelectorAll("[data-ac-pick], [data-ac-clear]").forEach((b) => {
    b.disabled = st.running;
    b.title = lockTip || b.dataset.acTitle || b.title;
  });
  for (const el of [dateInput, $("todayBtn")]) {
    el.disabled = st.running;
    el.title = st.running ? "自動切段監看中不能換日期，請先停止監看" : el.dataset.acTitle || "";
  }
  if (st.message) acMessage(st.message);
  else if (st.running) acMessage("每 15 秒檢查一次側錄資料夾");

  $("acSegments").replaceChildren(
    ...st.segments.map((s) => {
      const tr = document.createElement("tr");
      let [label, cls] = AC_STATES[s.state] || [s.state, ""];
      if (s.state === "rendering" && s.progress != null) label += ` ${Math.round(s.progress * 100)}%`;
      if (s.state === "done" && s.stale) {
        label = "靜圖已更新";
        cls = "bad";
      }
      const where = s.rec_start == null ? "" : `${acTime(s.rec_start)} – ${acTime(s.rec_end)}`;
      const len = s.state === "waiting" ? "" : fmtTime(s.recorded);
      tr.innerHTML = `<td>第 ${s.n} 段</td><td><span class="ac-state"></span></td><td>${where}</td><td>${len}</td><td>${s.rec_end == null ? "" : s.masks}</td><td class="ac-chain-cell"></td><td class="ac-share-cell"></td><td class="ac-actions"></td>`;
      acPill(tr.querySelector(".ac-state"), label, cls, s.state === "rendering" ? s.progress : null);
      if (s.error) tr.querySelector(".ac-state").title = s.error;
      const actions = tr.querySelector(".ac-actions");
      if (s.output) {
        const open = document.createElement("button");
        open.className = "ghost small";
        open.textContent = "開啟";
        open.onclick = () => invoke("autocut_reveal", { path: s.output });
        actions.append(open);
      }
      acChainCell(tr.querySelector(".ac-chain-cell"), s, st.running);
      // 壓 BAR：展開下面「壓 BAR」那一段
      const barBtn = document.createElement("button");
      barBtn.type = "button";
      barBtn.className = "ghost small";
      const nb = window.acBarCount?.(s.n) || 0;
      barBtn.textContent = nb ? `壓 BAR（${nb}）` : "壓 BAR";
      barBtn.onclick = () => window.acOpenBars?.(s.n);
      actions.append(barBtn);
      acShareCell(tr.querySelector(".ac-share-cell"), s.n);
      // 完整但沒出（關了自動出片）、出片失敗、靜圖更新後：讓使用者自己按
      if (st.running && (s.state === "complete" || s.state === "failed" || (s.state === "done" && s.stale))) {
        const btn = document.createElement("button");
        btn.className = "small";
        btn.textContent = s.state === "complete" ? "出片" : "重新出片";
        btn.onclick = () => invoke("autocut_render", { n: s.n }).catch((e) => acMessage(String(e)));
        actions.append(btn);
      } else if (st.running && s.state === "done") {
        // 已經出好、設定也沒變：還是可以手動重出（例如換了 BAR 圖但檔名一樣）
        const btn = document.createElement("button");
        btn.className = "ghost small";
        btn.textContent = "重新出片";
        btn.onclick = async () => {
          const ok = await dialog.ask(`第 ${s.n} 段已經出好了，要重新出片嗎？\n會覆蓋「第${s.n}段.mp4」，有串接的話也會重新串接。`, {
            title: "重新出片",
            kind: "warning",
            okLabel: "重新出片",
            cancelLabel: "取消",
          });
          if (ok) invoke("autocut_render", { n: s.n }).catch((e) => acMessage(String(e)));
        };
        actions.append(btn);
      }
      return tr;
    }),
  );
  if (!st.segments.length) {
    $("acSegments").innerHTML = `<tr><td colspan="8" class="muted">按「開始監看」後，偵測到的段落會列在這裡</td></tr>`;
  }
  acLabelCells($("acSegments"));
  acCopyToShare(st);
  $("acAll").textContent =
    st.all_progress != null ? `全集合成中 ${Math.round(st.all_progress * 100)}%` : st.all ? `全集：${st.all}` : "";

  $("acFiles").replaceChildren(
    ...st.files.map((f) => {
      const li = document.createElement("li");
      li.innerHTML = `<span class="name"></span><span class="muted"></span>`;
      li.querySelector(".name").textContent = f.name;
      li.querySelector(".name").title = f.name;
      li.querySelector(".muted").textContent = f.state === "done" ? `${fmtTime(f.frames / (30000 / 1001))}` : AC_FILE_STATES[f.state] || f.state;
      return li;
    }),
  );

  const stages = st.stages.slice(-40);
  const total = st.stages.reduce((a, s) => a + s.secs, 0);
  $("acStages").replaceChildren(
    ...stages.map((s) => {
      const li = document.createElement("li");
      const t = new Date(s.started).toLocaleTimeString("zh-TW", { hour12: false });
      li.textContent = `${t} ${s.name}：${s.secs.toFixed(1)} 秒`;
      return li;
    }),
    ...(st.stages.length ? [Object.assign(document.createElement("li"), { textContent: `合計 ${(total / 60).toFixed(1)} 分鐘`, className: "muted" })] : []),
  );
}

listen("autocut-status", (e) => acShow(e.payload));

listen("autocut-log", (e) => {
  const log = $("acLog");
  const t = new Date().toLocaleTimeString("zh-TW", { hour12: false });
  const atBottom = log.scrollTop + log.clientHeight >= log.scrollHeight - 4;
  log.textContent += `[${t}] ${e.payload}\n`;
  if (atBottom) log.scrollTop = log.scrollHeight;
});

listen("autocut-done", (e) => {
  const { n, path, chain } = e.payload;
  showNotice({ title: chain ? `第 ${n} 段串接完成` : n ? `第 ${n} 段出片完成` : "全集合成完成", message: path });
  // 重新出片後檔名一樣：讓它再複製一次，而且蓋掉共用資料夾的舊檔
  if (n) {
    acShareTried.delete(path);
    acShareFresh.add(path);
    if (acStatus) acCopyToShare(acStatus);
  }
  if (!n && e.payload.chapters?.length) acWriteChapters(e.payload.chapters);
});

/** 全集合成好：各段的章節時間（閃白結束、該段完全出現的那一秒，跟影片頁「填入章節時間」一樣）回寫文稿。
 *  先讀共用資料夾最新的文稿再改（docAutoEdit），寫完全集區有時間了，全集自動上傳才會接著跑 */
async function acWriteChapters(chapters) {
  const filled = [];
  const ok = await docAutoEdit("全集章節時間", () => {
    let any = false;
    filled.length = 0;
    for (const [n, sec] of chapters) {
      const input = segments[n - 1]?.time;
      if (!input) continue;
      const t = fmtTime(Math.ceil(sec - 1e-6));
      filled.push(`第${n}段 ${t}`);
      if (input.value.trim() !== t) {
        input.value = t;
        any = true;
      }
    }
    return any;
  });
  if (ok) {
    acMessage(`全集章節時間已寫回文稿：${filled.join("、")}`);
    showNotice({ title: "全集章節時間已寫回文稿", message: filled.join("、") });
  }
}

// ───────── 每段串接（正片 →（閃白）→ 影片（可以好幾支）→ promo） ─────────

const AC_CHAIN_STATES = {
  need_yt: ["等 YT", "bad"],
  waiting: ["等正片", ""],
  queued: ["排隊", "ok"],
  rendering: ["串接中", "ok"],
  done: ["完成", "done"],
  stale: ["設定已改", "bad"],
  failed: ["失敗", "bad"],
};

/** 一支要串的影片（YT 網址或檔案） */
function acNewVideo(src = "yt") {
  return { src, url: "", ytPath: "", file: "", startText: "", endText: "", start: null, end: null, flash: true, ytStatus: "" };
}

/** 預設：每段都接 45 秒 promo；第三、六段另外串一支 YT（閃白） */
function acChainDefault(n) {
  const yt = n === 3 || n === 6;
  return { promo: 45, yt_mode: yt, videos: yt ? [acNewVideo()] : [] };
}

/** 影片檔：YT 下載好的檔，或直接選的檔案 */
function acChainVideo(v) {
  return v.src === "file" ? v.file : v.ytPath;
}

/** 還沒填網址、也沒選檔案 */
function acVideoEmpty(v) {
  return v.src === "file" ? !v.file : !v.url;
}

function acVideoStatus(v) {
  if (v.src === "file") return v.file ? v.file.split(/[\\/]/).pop() : "";
  return v.ytPath ? "已下載" : "";
}

let acChains = {};
let acChainsDate = null;
let acChainEls = {}; // 段號 → { tr, list, count }

function acChainKey() {
  return `acChains:${dateInput.value.trim() || "today"}`;
}

function acLoadChains() {
  acChainsDate = dateInput.value.trim();
  let saved = null;
  try {
    saved = JSON.parse(localStorage.getItem(acChainKey()) || "null");
  } catch {}
  acChains = {};
  for (const n of [1, 2, 3, 4, 5, 6]) {
    const s = saved?.[n];
    let c;
    if (!s) c = acChainDefault(n);
    else if (Array.isArray(s.videos)) c = { promo: s.promo ?? 45, yt_mode: !!s.yt_mode, videos: s.videos.map((v) => ({ ...acNewVideo(), ...v })) };
    else {
      // 0.6.18 以前：一段只有一支影片，欄位直接放在段上
      const v = { ...acNewVideo(s.src || "yt"), url: s.url || "", ytPath: s.ytPath || s.path || "", file: s.file || "", startText: s.startText || "", endText: s.endText || "", start: s.start ?? null, end: s.end ?? null, flash: s.flash !== false };
      c = { promo: s.promo ?? 45, yt_mode: !!s.yt_mode, videos: s.yt_mode || !acVideoEmpty(v) ? [v] : [] };
    }
    for (const v of c.videos) v.ytStatus = acVideoStatus(v);
    acChains[n] = c;
  }
  acBuildChainRows();
}

function acSaveChains() {
  try {
    localStorage.setItem(acChainKey(), JSON.stringify(acChains));
  } catch {}
}

/** 送給 Rust 的設定；不接東西＝null。空白的那列不算；有填但還沒下載好的＝等著 */
function acChainPayload(n) {
  const c = acChains[n];
  if (!c || !(c.promo || c.yt_mode)) return null;
  const filled = c.yt_mode ? c.videos.filter((v) => !acVideoEmpty(v)) : [];
  const ready = filled.filter((v) => acChainVideo(v));
  return {
    promo: c.promo,
    yt_mode: c.yt_mode,
    videos: ready.map((v) => ({ path: acChainVideo(v), start: v.start, end: v.end, flash: v.flash })),
    waiting: ready.length < filled.length,
  };
}

async function acPushChain(n, retry = false) {
  acSaveChains();
  try {
    await invoke("autocut_set_chain", { n, chain: acChainPayload(n), retry });
  } catch (e) {
    acMessage(String(e));
  }
}

/** 監看中可以改、馬上生效的設定（Rust 端 LiveOptions）。開始監看時也用同一份 */
let acRunDate = "";
async function acLiveOptions(date) {
  return {
    since: $("acSince").value.trim() || null,
    end_cutoff: $("acEndCutoff").value.trim() || null,
    rotate_secs: Math.max(0, Number($("acRotateSecs").value) || 0),
    ai_key: settings.autocut_ai_key || null,
    ai_model: settings.autocut_ai_model || null,
    gain_db: settings.audio_gain_db ?? 6,
    watermarks: await acWatermarks(date),
    promo_20: settings.promo_20_path || "",
    promo_45: settings.promo_45_path || "",
    transition: settings.transition_secs ?? 0.5,
    encoder: await acEncoder(),
  };
}

/** 監看中改了上面那些設定：送給 Rust（半秒內連改只送一次）。11/07 以前只在開始監看時讀一次 */
let acLiveTimer = null;
function acPushLive() {
  if (!acStatus?.running) return;
  clearTimeout(acLiveTimer);
  acLiveTimer = setTimeout(async () => {
    try {
      await invoke("autocut_set_live", { options: await acLiveOptions(acRunDate || dateInput.value.trim()) });
    } catch (e) {
      acMessage(`設定沒送出去：${e}`);
    }
  }, 500);
}

/** 台標＋月份標。月份標檔名是年月（202610.png）時換成節目日期那個月的同資料夾檔案 */
async function acWatermarks(date) {
  const list = [];
  if (settings.autocut_watermark) list.push(settings.autocut_watermark);
  let month = settings.autocut_watermark_month || "";
  const m = month.match(/^(.*[\\/])(\d{6})(\.png)$/i);
  if (m && /^\d{8}$/.test(date) && m[2] !== date.slice(0, 6)) {
    const want = `${m[1]}${date.slice(0, 6)}${m[3]}`;
    if (await invoke("path_exists", { path: want })) month = want;
    else acMessage(`找不到 ${date.slice(0, 6)} 的月份標，先用 ${m[2]}`);
  }
  if (month) list.push(month);
  return list;
}

async function acEncoder() {
  if (settings.encoder && settings.encoder !== "auto") return settings.encoder;
  try {
    return (await invoke("detect_encoder", { force: false })).chosen;
  } catch {
    return "x264";
  }
}

function acBuildChainRows() {
  acChainEls = {};
  $("acChains").replaceChildren(
    ...[1, 2, 3, 4, 5, 6].map((n) => {
      const c = acChains[n];
      const tr = document.createElement("tr");
      tr.dataset.chain = n;
      tr.innerHTML = `<td>第 ${n} 段</td>
        <td><select class="ac-promo"><option value="0">不接</option><option value="20">20 秒</option><option value="45">45 秒</option></select></td>
        <td><input type="checkbox" class="ac-yt switch" /></td>
        <td class="ac-videos-cell"><div class="ac-video-list"></div>
          <button type="button" class="ghost small ac-video-add">＋ 新增影片</button></td>`;
      const q = (s) => tr.querySelector(s);
      q(".ac-promo").value = String(c.promo);
      q(".ac-yt").checked = c.yt_mode;
      q(".ac-promo").onchange = () => {
        c.promo = Number(q(".ac-promo").value);
        acPushChain(n);
      };
      q(".ac-yt").onchange = () => {
        c.yt_mode = q(".ac-yt").checked;
        if (c.yt_mode && !c.videos.length) c.videos.push(acNewVideo());
        acBuildVideoItems(n);
        acPushChain(n);
      };
      q(".ac-video-add").onclick = () => {
        c.videos.push(acNewVideo());
        c.yt_mode = true;
        q(".ac-yt").checked = true;
        acBuildVideoItems(n);
        acPushChain(n);
        acChainEls[n].list.lastElementChild?.querySelector(".ac-url")?.focus();
      };
      acChainEls[n] = { tr, list: q(".ac-video-list") };
      acBuildVideoItems(n);
      return tr;
    }),
  );
  acLabelCells($("acChains"));
}

/** 一段裡的影片列表：每支一列，照順序接在正片後面 */
function acBuildVideoItems(n) {
  const c = acChains[n];
  const el = acChainEls[n];
  el.tr.classList.toggle("ac-chain-off", !c.yt_mode);
  el.list.replaceChildren(
    ...c.videos.map((v, i) => {
      const item = document.createElement("div");
      item.className = "ac-video-item";
      item.innerHTML = `<span class="ac-video-no">${i + 1}</span>
        <select class="ac-src"><option value="yt">YT</option><option value="file">檔案</option></select>
        <input class="ac-url" spellcheck="false" placeholder="https://youtu.be/…" />
        <button type="button" class="ghost small ac-pick">選擇檔案</button>
        <input class="ac-start ac-time" spellcheck="false" placeholder="開頭" title="從這裡開始（空白＝從頭）" />
        <input class="ac-end ac-time" spellcheck="false" placeholder="結尾" title="到這裡結束（空白＝到最後）" />
        <label class="ac-flash-label" title="接在前一支後面時閃白"><input type="checkbox" class="ac-flash switch" />閃白</label>
        <span class="ac-yt-status muted"></span>
        <button type="button" class="ghost small danger ac-video-del" title="刪掉這支">刪除</button>`;
      const q = (s) => item.querySelector(s);
      acVideoEls.set(v, q(".ac-yt-status"));
      q(".ac-src").value = v.src;
      q(".ac-url").value = v.url;
      q(".ac-start").value = v.startText;
      q(".ac-end").value = v.endText;
      q(".ac-flash").checked = v.flash;
      const syncSrc = () => {
        q(".ac-url").classList.toggle("hidden", v.src === "file");
        q(".ac-pick").classList.toggle("hidden", v.src !== "file");
      };
      syncSrc();
      q(".ac-src").onchange = () => {
        v.src = q(".ac-src").value;
        syncSrc();
        acYtStatus(v, acVideoStatus(v));
        acPushChain(n);
      };
      q(".ac-pick").onclick = async () => {
        const path = await pickVideo();
        if (!path) return;
        v.file = path;
        acYtStatus(v, acVideoStatus(v));
        acPushChain(n);
      };
      q(".ac-flash").onchange = () => {
        v.flash = q(".ac-flash").checked;
        acPushChain(n);
      };
      q(".ac-url").onchange = () => {
        v.url = q(".ac-url").value.trim();
        v.ytPath = "";
        acPushChain(n);
        if (v.url) acDownloadYt(n, v);
        else acYtStatus(v, "");
      };
      for (const [sel, key] of [
        [".ac-start", "start"],
        [".ac-end", "end"],
      ]) {
        q(sel).onchange = async () => {
          q(sel).value = acDigitsTime(q(sel).value, "pos");
          const text = q(sel).value.trim();
          try {
            v[key] = text ? await invoke("parse_timecode", { input: text }) : null;
            v[`${key}Text`] = text;
            q(sel).classList.remove("bad");
            acPushChain(n);
          } catch (e) {
            q(sel).classList.add("bad");
            q(sel).title = String(e);
          }
        };
      }
      q(".ac-video-del").onclick = () => {
        c.videos.splice(i, 1);
        if (!c.videos.length) c.yt_mode = false;
        el.tr.querySelector(".ac-yt").checked = c.yt_mode;
        acBuildVideoItems(n);
        acPushChain(n);
      };
      acYtStatus(v, v.ytStatus);
      return item;
    }),
  );
  if (!c.videos.length) el.list.append(Object.assign(document.createElement("div"), { className: "muted ac-video-empty", textContent: "不串影片。按「＋ 新增影片」加一支；可以加好幾支，照順序接。" }));
}

const acVideoEls = new WeakMap(); // 影片 → 它的「影片狀態」欄（不存進 localStorage）

function acYtStatus(v, text) {
  v.ytStatus = text;
  const el = acVideoEls.get(v);
  if (el) el.textContent = text;
}

// YT 一次下載一支
let acYtQueue = Promise.resolve();
let acYtActive = null;
listen("yt-progress", ({ payload }) => {
  if (acYtActive) acYtStatus(acYtActive, `下載中 ${payload.percent.toFixed(0)}%`);
});

function acDownloadYt(n, v) {
  const url = v.url;
  acYtStatus(v, "排隊下載…");
  acYtQueue = acYtQueue.then(async () => {
    if (v.url !== url || !acChains[n].videos.includes(v)) return; // 排隊時網址改了、刪掉了
    acYtActive = v;
    acYtStatus(v, "下載中…");
    try {
      const path = await invoke("download_youtube", { url });
      if (v.url !== url) return;
      v.ytPath = path;
      acYtStatus(v, "已下載");
      acPushChain(n);
    } catch (e) {
      acYtStatus(v, `★ ${e}`);
    } finally {
      acYtActive = null;
    }
  });
}

/** 段落表的「串接」欄 */
function acChainCell(td, s, running) {
  const ch = s.chain;
  if (!ch) return;
  let [label, cls] = AC_CHAIN_STATES[ch.state] || [ch.state, ""];
  if (ch.state === "rendering" && ch.progress != null) label += ` ${Math.round(ch.progress * 100)}%`;
  const span = document.createElement("span");
  acPill(span, label, cls, ch.state === "rendering" ? ch.progress : null);
  if (ch.error) span.title = ch.error;
  td.append(span);
  if (ch.output && (ch.state === "done" || ch.state === "stale")) {
    const open = document.createElement("button");
    open.className = "ghost small";
    open.textContent = "開啟";
    open.onclick = () => invoke("autocut_reveal", { path: ch.output });
    td.append(open);
  }
  if (running && (ch.state === "failed" || ch.state === "stale")) {
    const btn = document.createElement("button");
    btn.className = "small";
    btn.textContent = "重新串接";
    btn.onclick = () => acPushChain(s.n, true);
    td.append(btn);
  }
}

// 設定頁：水印選圖、清除
document.querySelectorAll("[data-pick-image]").forEach((btn) => {
  btn.addEventListener("click", async () => {
    const path = await dialog.open({ multiple: false, filters: [{ name: "圖片", extensions: ["png"] }] });
    if (!path) return;
    settingsForm.elements[btn.dataset.pickImage].value = path;
    settingsFieldChanged();
  });
});
document.querySelectorAll("[data-clear-field]").forEach((btn) => {
  btn.addEventListener("click", () => {
    settingsForm.elements[btn.dataset.clearField].value = "";
    settingsFieldChanged();
  });
});

// 換日期＝換一組串接設定
dateInput.addEventListener("change", () => {
  if (dateInput.value.trim() !== acChainsDate) acLoadChains();
});

// 文稿頁更新後（打字、按「今天」、匯入、跨日）側錄資料夾跟著換日期
window.onDocRefreshed = () => {
  if (settings) acFillDirs();
};

// 設定載入／改變後更新畫面上的資料夾
const acPrevSettingsChanged = window.onSettingsChanged;
window.onSettingsChanged = () => {
  acPrevSettingsChanged?.();
  acFillDirs();
  acPushLive();
};
(async function acInit() {
  for (const el of [...document.querySelectorAll("[data-ac-pick], [data-ac-clear]"), dateInput, $("todayBtn")]) el.dataset.acTitle = el.title || "";
  while (!settings) await new Promise((r) => setTimeout(r, 100));
  acFillDirs();
  acLoadChains();
  acShow(await invoke("autocut_status"));
})();

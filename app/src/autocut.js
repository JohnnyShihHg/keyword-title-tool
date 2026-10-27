// 自動切段頁：選資料夾、開始／停止監看、段落狀態表、側錄檔清單、各階段耗時、紀錄。
// 判斷與出片都在 Rust 端（autocut.rs）；這裡只顯示 autocut-status 事件。
// 和 app.js／video.js 共用全域的 invoke、$、settings、listen、dialog、showNotice、
// collect、dateInput、persistSettings、fmtTime。

const AC_STATES = {
  waiting: ["等待", ""],
  recording: ["錄製中", ""],
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
  $("acOutputDir").value = settings.autocut_output_dir || "";
  $("acOutputDir").placeholder = settings.output_dir ? `空白＝沿用 ${settings.output_dir}` : "尚未指定";
  $("acAutoRender").checked = settings.autocut_auto_render !== false;
  $("acCopyShare").checked = !!settings.autocut_copy_to_share;
  $("acSince").value = settings.autocut_since ?? "";
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
document.querySelectorAll("[data-ac-clear]").forEach((btn) => {
  btn.addEventListener("click", () => {
    settings[btn.dataset.acClear] = "";
    acFillDirs();
    persistSettings();
  });
});
$("acSince").addEventListener("change", () => {
  settings.autocut_since = $("acSince").value.trim();
  persistSettings();
});
$("acAutoRender").addEventListener("change", () => {
  settings.autocut_auto_render = $("acAutoRender").checked;
  persistSettings();
});
$("acCopyShare").addEventListener("change", () => {
  settings.autocut_copy_to_share = $("acCopyShare").checked;
  persistSettings();
  if (acStatus) acShow(acStatus);
});

// ───────── 成品複製到當天的共用資料夾（{日期}-{段}.mp4，給人檢查） ─────────
// 每個成品這次開程式只複製一次；目的地已有同名檔就不覆蓋（Rust 端判斷）

let acShare = {}; // 段號 → { state, progress, dest, error }
const acShareTried = new Set();

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
    acShare[s.n] = { state: "copying", progress: 0 };
    invoke("autocut_copy_to_share", { settings, date, n: s.n, src }).catch((e) => {
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
        since: $("acSince").value.trim() || null,
        ai_key: settings.autocut_ai_key || null,
        ai_model: settings.autocut_ai_model || null,
        // 正片加工（跟影片頁單集一樣）、串接、全集用
        gain_db: settings.audio_gain_db ?? 6,
        watermarks: await acWatermarks(date),
        promo_20: settings.promo_20_path || "",
        promo_45: settings.promo_45_path || "",
        transition: settings.transition_secs ?? 0.5,
        encoder: await acEncoder(),
        date_tag: /^\d{8}$/.test(date) ? date.slice(4) : "",
        chains: Object.fromEntries(
          [1, 2, 3, 4, 5, 6].map((n) => [n, acChainPayload(n)]).filter(([, c]) => c),
        ),
        bars: acBarsConfig(),
      },
    });
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
});

// ───────── 每段串接（正片 →（閃白）→ YT → promo） ─────────

const AC_CHAIN_STATES = {
  need_yt: ["等 YT", "bad"],
  waiting: ["等正片", ""],
  queued: ["排隊", "ok"],
  rendering: ["串接中", "ok"],
  done: ["完成", "done"],
  stale: ["設定已改", "bad"],
  failed: ["失敗", "bad"],
};

/** 預設：第三、六段串 YT（閃白）；其他段接 45 秒 promo */
function acChainDefault(n) {
  const yt = n === 3 || n === 6;
  return { promo: yt ? 0 : 45, yt_mode: yt, src: "yt", url: "", ytPath: "", file: "", startText: "", endText: "", start: null, end: null, flash: true, ytStatus: "" };
}

/** 要串的影片：YT 下載好的檔，或直接選的檔案 */
function acChainVideo(c) {
  return c.src === "file" ? c.file : c.ytPath;
}

function acVideoStatus(c) {
  if (c.src === "file") return c.file ? c.file.split(/[\\/]/).pop() : "";
  return c.ytPath ? "已下載" : "";
}

let acChains = {};
let acChainsDate = null;

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
    const c = { ...acChainDefault(n), ...(saved?.[n] || {}) };
    if (c.path && !c.ytPath) c.ytPath = c.path; // 0.6.1 存的 YT 檔
    delete c.path;
    c.ytStatus = acVideoStatus(c);
    acChains[n] = c;
  }
  acBuildChainRows();
}

function acSaveChains() {
  try {
    localStorage.setItem(acChainKey(), JSON.stringify(acChains));
  } catch {}
}

/** 送給 Rust 的設定；不接東西＝null */
function acChainPayload(n) {
  const c = acChains[n];
  if (!c || !(c.promo || c.yt_mode)) return null;
  const video = acChainVideo(c);
  return {
    promo: c.promo,
    yt_mode: c.yt_mode,
    yt: c.yt_mode && video ? { path: video, start: c.start, end: c.end } : null,
    flash: c.flash,
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
  $("acChains").replaceChildren(
    ...[1, 2, 3, 4, 5, 6].map((n) => {
      const c = acChains[n];
      const tr = document.createElement("tr");
      tr.dataset.chain = n;
      tr.innerHTML = `<td>第 ${n} 段</td>
        <td><select class="ac-promo"><option value="0">不接</option><option value="20">20 秒</option><option value="45">45 秒</option></select></td>
        <td><input type="checkbox" class="ac-yt switch" /></td>
        <td class="ac-src-cell"><select class="ac-src"><option value="yt">YT</option><option value="file">檔案</option></select>
          <input class="ac-url" spellcheck="false" placeholder="https://youtu.be/…" />
          <button type="button" class="ghost small ac-pick">選擇檔案</button></td>
        <td><input class="ac-start ac-time" spellcheck="false" placeholder="開頭" /></td>
        <td><input class="ac-end ac-time" spellcheck="false" placeholder="結尾" /></td>
        <td><input type="checkbox" class="ac-flash switch" /></td>
        <td class="ac-yt-status muted"></td>`;
      const q = (s) => tr.querySelector(s);
      q(".ac-promo").value = String(c.promo);
      q(".ac-yt").checked = c.yt_mode;
      q(".ac-url").value = c.url;
      q(".ac-start").value = c.startText;
      q(".ac-end").value = c.endText;
      q(".ac-flash").checked = c.flash;
      q(".ac-src").value = c.src;
      const syncEnabled = () => {
        for (const s of [".ac-src", ".ac-url", ".ac-pick", ".ac-start", ".ac-end", ".ac-flash"]) q(s).disabled = !c.yt_mode;
        q(".ac-url").classList.toggle("hidden", c.src === "file");
        q(".ac-pick").classList.toggle("hidden", c.src !== "file");
      };
      syncEnabled();
      q(".ac-src").onchange = () => {
        c.src = q(".ac-src").value;
        syncEnabled();
        acYtStatus(n, acVideoStatus(c));
        acPushChain(n);
      };
      q(".ac-pick").onclick = async () => {
        const path = await pickVideo();
        if (!path) return;
        c.file = path;
        acYtStatus(n, acVideoStatus(c));
        acPushChain(n);
      };
      q(".ac-promo").onchange = () => {
        c.promo = Number(q(".ac-promo").value);
        acPushChain(n);
      };
      q(".ac-yt").onchange = () => {
        c.yt_mode = q(".ac-yt").checked;
        syncEnabled();
        acPushChain(n);
      };
      q(".ac-flash").onchange = () => {
        c.flash = q(".ac-flash").checked;
        acPushChain(n);
      };
      q(".ac-url").onchange = () => {
        c.url = q(".ac-url").value.trim();
        c.ytPath = "";
        acPushChain(n);
        if (c.url) acDownloadYt(n);
        else acYtStatus(n, "");
      };
      for (const [sel, key] of [
        [".ac-start", "start"],
        [".ac-end", "end"],
      ]) {
        q(sel).onchange = async () => {
          const text = q(sel).value.trim();
          try {
            c[key] = text ? await invoke("parse_timecode", { input: text }) : null;
            c[`${key}Text`] = text;
            q(sel).classList.remove("bad");
            acPushChain(n);
          } catch (e) {
            q(sel).classList.add("bad");
            q(sel).title = String(e);
          }
        };
      }
      return tr;
    }),
  );
  for (const n of [1, 2, 3, 4, 5, 6]) acYtStatus(n, acChains[n].ytStatus);
  acLabelCells($("acChains"));
}

function acYtStatus(n, text) {
  acChains[n].ytStatus = text;
  const el = document.querySelector(`[data-chain="${n}"] .ac-yt-status`);
  if (el) el.textContent = text;
}

// YT 一次下載一支
let acYtQueue = Promise.resolve();
let acYtActive = null;
listen("yt-progress", ({ payload }) => {
  if (acYtActive != null) acYtStatus(acYtActive, `下載中 ${payload.percent.toFixed(0)}%`);
});

function acDownloadYt(n) {
  const url = acChains[n].url;
  acYtStatus(n, "排隊下載…");
  acYtQueue = acYtQueue.then(async () => {
    if (acChains[n].url !== url) return; // 排隊時網址又改了
    acYtActive = n;
    acYtStatus(n, "下載中…");
    try {
      const path = await invoke("download_youtube", { url });
      if (acChains[n].url !== url) return;
      acChains[n].ytPath = path;
      acYtStatus(n, "已下載");
      acPushChain(n);
    } catch (e) {
      acYtStatus(n, `★ ${e}`);
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
};
(async function acInit() {
  while (!settings) await new Promise((r) => setTimeout(r, 100));
  acFillDirs();
  acLoadChains();
  acShow(await invoke("autocut_status"));
})();

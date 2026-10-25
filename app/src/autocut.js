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
  done: ["完成", "ok"],
  incomplete: ["不完整", "bad"],
  failed: ["失敗", "bad"],
};
const AC_FILE_STATES = { recording: "錄製中", waiting: "等待", scanning: "掃描中…", ocr: "OCR…", done: "已掃描", error: "★ 失敗" };

let acStatus = null;
let acTitlesTimer = null;

function acOutputDir() {
  return settings.autocut_output_dir || settings.output_dir || "";
}

function acFillDirs() {
  $("acWatchDir").value = settings.autocut_watch_dir || "";
  $("acStillsDir").value = settings.autocut_stills_dir || "";
  $("acOutputDir").value = settings.autocut_output_dir || "";
  $("acOutputDir").placeholder = settings.output_dir ? `空白＝沿用 ${settings.output_dir}` : "尚未指定";
  $("acAutoRender").checked = settings.autocut_auto_render !== false;
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
$("acAutoRender").addEventListener("change", () => {
  settings.autocut_auto_render = $("acAutoRender").checked;
  persistSettings();
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
        watch_dir: settings.autocut_watch_dir,
        stills_dir: settings.autocut_stills_dir,
        output_dir: acOutputDir(),
        titles,
        auto_render: settings.autocut_auto_render !== false,
        make_all: $("acMakeAll").checked,
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

function acShow(st) {
  acStatus = st;
  $("acStartBtn").classList.toggle("hidden", st.running);
  $("acStopBtn").classList.toggle("hidden", !st.running);
  if (st.message) acMessage(st.message);
  else if (st.running) acMessage("監看中，每 15 秒檢查一次側錄資料夾");

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
      tr.innerHTML = `<td>第 ${s.n} 段</td><td><span class="ac-state ${cls}"></span></td><td>${where}</td><td>${len}</td><td>${s.rec_end == null ? "" : s.masks}</td><td class="ac-actions"></td>`;
      tr.querySelector(".ac-state").textContent = label;
      if (s.error) tr.querySelector(".ac-state").title = s.error;
      const actions = tr.querySelector(".ac-actions");
      if (s.output) {
        const open = document.createElement("button");
        open.className = "ghost small";
        open.textContent = "開啟";
        open.onclick = () => invoke("autocut_reveal", { path: s.output });
        actions.append(open);
      }
      // 完整但沒出（關了自動出片）、出片失敗、靜圖更新後：讓使用者自己按
      if (st.running && (s.state === "complete" || s.state === "failed" || (s.state === "done" && s.stale))) {
        const btn = document.createElement("button");
        btn.className = "small";
        btn.textContent = s.state === "complete" ? "出片" : "重新出片";
        btn.onclick = () => invoke("autocut_render", { n: s.n }).catch((e) => acMessage(String(e)));
        actions.append(btn);
      }
      return tr;
    }),
  );
  $("acAll").textContent = st.all ? `全集：${st.all}` : "";

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
  const { n, path } = e.payload;
  showNotice({ title: n ? `第 ${n} 段出片完成` : "全集合成完成", message: path });
});

// 設定載入／改變後更新畫面上的資料夾
const acPrevSettingsChanged = window.onSettingsChanged;
window.onSettingsChanged = () => {
  acPrevSettingsChanged?.();
  acFillDirs();
};
(async function acInit() {
  while (!settings) await new Promise((r) => setTimeout(r, 100));
  acFillDirs();
  acShow(await invoke("autocut_status"));
})();

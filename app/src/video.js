// 影片頁。
//   單集：完整影片 ⚡ YT 影片（選填）｜ 45 秒 promo
//   全集：第一段 ｜ 20 秒 promo ⚡ 第二段 ⚡ … ⚡ 第六段 ｜ 45 秒 promo
// ⚡ = 閃白過場，｜ = 直接接。和 app.js 共用全域的 invoke、$、settings、lastRendered、
// segments、changed、SEGMENT_NAMES、settingsForm、settingsFieldChanged。

const { listen } = window.__TAURI__.event;
const dialog = window.__TAURI__.dialog;
const { convertFileSrc } = window.__TAURI__.core;

const VIDEO_EXTENSIONS = ["mp4", "mov", "mkv", "m4v", "avi", "mxf", "webm"];
const OUTPUT = { width: 1920, height: 1080, fps: "30000/1001" };
/** 整條 timeline 大約放幾張縮圖 */
const TIMELINE_THUMBS = 36;

/**
 * 所有素材，key → { path, info: ProbeInfo, start, end }。
 * 單集：main、yt；全集：s1～s6；兩邊共用：promo20、promo45（來自設定）。
 * start/end 只有 YT 影片會用（裁切）。
 */
const store = {};
const probeCache = new Map();
const thumbCache = new Map();

function fmtTime(sec) {
  const s = Math.max(0, Math.round(sec));
  const p = (n) => String(n).padStart(2, "0");
  return `${p(Math.floor(s / 3600))}:${p(Math.floor((s / 60) % 60))}:${p(s % 60)}`;
}

function baseName(path) {
  return path.split(/[\\/]/).pop();
}

function usedDuration(c) {
  const start = Math.min(c.start ?? 0, c.info.duration);
  const end = Math.min(c.end ?? c.info.duration, c.info.duration);
  return Math.max(0, end - start);
}

function describe(info) {
  const fps = info.fps ? ` · ${info.fps.toFixed(2)}fps` : "";
  const audio = info.has_audio ? "" : " · 無聲音";
  return `${fmtTime(info.duration)} · ${info.width}×${info.height}${fps}${audio}`;
}

async function probe(path) {
  if (!probeCache.has(path)) probeCache.set(path, await invoke("probe_video", { path }));
  return probeCache.get(path);
}

function pickVideo() {
  return dialog.open({ multiple: false, filters: [{ name: "影片", extensions: VIDEO_EXTENSIONS }] });
}

/** 設定素材（path 為空就移除），完成後重畫兩個 panel。 */
async function setClip(key, path, extra = {}) {
  if (!path) delete store[key];
  else store[key] = { path, info: await probe(path), start: null, end: null, ...extra };
  panels.forEach((p) => p.refresh());
  return store[key];
}

// --- 工具狀態 ---

async function refreshTools() {
  const s = await invoke("tools_status");
  const ok = s.ffmpeg && s.ffprobe;
  $("toolsWarning").classList.toggle("hidden", !!ok);
  const row = (name, path) => `<div>${path ? "✓" : "✗"} ${name}：<span class="muted">${path || "未安裝"}</span></div>`;
  $("toolsStatus").innerHTML = row("ffmpeg", s.ffmpeg) + row("ffprobe", s.ffprobe) + row("yt-dlp", s.ytdlp);
  $("installToolsBtn").textContent = ok && s.ytdlp ? "重新下載全部" : "下載工具";
  $("installToolsBtn").disabled = !s.can_install;
  $("updateYtdlpBtn").disabled = !s.can_install;
  if (!s.can_install) $("toolsMsg").textContent = "這個系統不支援自動下載，請自行安裝 ffmpeg 和 yt-dlp";
  return s;
}

async function installTools(which) {
  const btns = [$("installToolsBtn"), $("updateYtdlpBtn")];
  btns.forEach((b) => (b.disabled = true));
  const progress = $("toolsProgress");
  progress.classList.remove("hidden");
  const unlisten = await listen("tools-progress", ({ payload: p }) => {
    progress.value = p.total ? p.downloaded / p.total : 0;
    const mb = (n) => (n / 1048576).toFixed(0);
    $("toolsMsg").textContent = `下載 ${p.label}：${mb(p.downloaded)}${p.total ? ` / ${mb(p.total)}` : ""} MB`;
  });
  try {
    await invoke("install_tools", { which });
    $("toolsMsg").textContent = "下載完成";
  } catch (e) {
    $("toolsMsg").textContent = `⚠ ${e}`;
  } finally {
    unlisten();
    progress.classList.add("hidden");
    btns.forEach((b) => (b.disabled = false));
    await refreshTools();
  }
}

$("installToolsBtn").addEventListener("click", () => installTools("all"));
$("updateYtdlpBtn").addEventListener("click", () => installTools("ytdlp"));
$("gotoToolsBtn").addEventListener("click", () => document.querySelector('.tab[data-page="settings"]').click());

// 設定頁的檔案／資料夾選擇按鈕
document.querySelectorAll("[data-pick]").forEach((btn) => {
  btn.addEventListener("click", async () => {
    const path = await pickVideo();
    if (!path) return;
    settingsForm.elements[btn.dataset.pick].value = path;
    settingsFieldChanged();
  });
});
document.querySelectorAll("[data-pick-dir]").forEach((btn) => {
  btn.addEventListener("click", async () => {
    const dir = await dialog.open({ directory: true, multiple: false });
    if (!dir) return;
    settingsForm.elements[btn.dataset.pickDir].value = dir;
    settingsFieldChanged();
  });
});

// --- Promo（檔案在設定裡指定） ---

async function refreshPromos() {
  for (const [key, secs, path] of [
    ["promo20", 20, settings.promo_20_path],
    ["promo45", 45, settings.promo_45_path],
  ]) {
    const labels = document.querySelectorAll(`.${key}-info`);
    const show = (text) => labels.forEach((l) => (l.textContent = text));
    if (!path) {
      show(`⚠ 還沒指定 ${secs} 秒 promo，請到設定選擇檔案`);
      if (store[key]) await setClip(key, null);
      continue;
    }
    try {
      if (store[key]?.path !== path) await setClip(key, path);
      show(`${secs} 秒：${baseName(path)}（${fmtTime(store[key].info.duration)}）`);
    } catch (e) {
      show(`⚠ ${secs} 秒 promo：${e}`);
    }
  }
}

// --- Panel：timeline、預覽、輸出，單集和全集共用 ---

const SLOT_CLASS = { main: "clip-main", yt: "clip-yt", promo: "clip-promo" };

/**
 * @param prefix  HTML id 前綴（single / full）
 * @param buildSequence 回傳 [{ key, label, kind: main|yt|promo, join: flash|cut, segment? }]，
 *                      沒選檔案的項目會自動略過
 * @param defaultName 檔名欄空白時的預設檔名
 */
function createPanel(prefix, buildSequence, defaultName) {
  const el = (name) => $(`${prefix}${name}`);
  const panel = { sequence: () => buildSequence().filter((item) => store[item.key]) };
  let lastOutput = null;
  let overwriteArmed = null;
  let drawSeq = 0;

  function baseSpec(seq, extra = {}) {
    return {
      clips: seq.map((item) => {
        const c = store[item.key];
        return {
          path: c.path,
          duration: c.info.duration,
          has_audio: c.info.has_audio,
          start: c.start,
          end: c.end,
          join: item.join || "flash",
        };
      }),
      transition: settings?.transition_secs ?? 0.5,
      ...OUTPUT,
      encoder: "x264",
      output: "",
      gain_db: settings?.audio_gain_db ?? 6,
      ...extra,
    };
  }

  /** 每段在成品上的位置（由 Rust 端計算，和實際輸出一致）。 */
  panel.plan = async () => {
    const seq = panel.sequence();
    if (!seq.length) return { seq, placements: [], total: 0 };
    const [placements, total] = await invoke("plan_video", { spec: baseSpec(seq) });
    return { seq, placements, total };
  };

  panel.refresh = async () => {
    const mySeq = ++drawSeq;
    const plan = await panel.plan();
    if (mySeq !== drawSeq) return;
    draw(plan);
    loadThumbs(plan.seq);
  };

  function sumUsed(seq) {
    return seq.reduce((sum, item) => sum + usedDuration(store[item.key]), 0);
  }

  function thumbCount(used, total) {
    return Math.max(3, Math.round((TIMELINE_THUMBS * used) / Math.max(total, 1)));
  }

  function thumbKey(clip, count) {
    return `${clip.path}|${clip.start ?? 0}|${usedDuration(clip)}|${count}`;
  }

  function draw({ seq, placements, total }) {
    const timeline = el("Timeline");
    const ruler = el("Ruler");
    timeline.replaceChildren();
    ruler.replaceChildren();
    if (!seq.length) {
      timeline.innerHTML = '<div class="timeline-empty muted">選擇影片後會顯示在這裡</div>';
      el("Total").textContent = "";
      return;
    }
    el("Total").textContent = `成品長度 ${fmtTime(total)}`;

    // 照實際比例的話 20 秒 promo 在 30 分鐘影片旁邊只剩一條線，所以每段至少佔一定寬度
    const used_total = sumUsed(seq);
    const minShare = seq.length > 4 ? 0.06 : 0.15;
    const widthOf = (used) => String(Math.max(used, used_total * minShare));

    seq.forEach((item, i) => {
      const clip = store[item.key];
      const used = usedDuration(clip);
      if (i > 0) {
        const join = document.createElement("div");
        join.className = item.join === "cut" ? "cut" : "flash";
        join.title = item.join === "cut" ? "直接接" : `閃白 ${settings.transition_secs} 秒`;
        timeline.appendChild(join);
      }
      const block = document.createElement("div");
      block.className = `clip ${SLOT_CLASS[item.kind]}`;
      block.style.flexGrow = widthOf(used);
      block.title = `${item.label}：${baseName(clip.path)}（${fmtTime(used)}）`;
      block.innerHTML = `<div class="clip-name">${item.label} · ${fmtTime(used)}</div><div class="thumbs"></div>`;
      const thumbs = block.querySelector(".thumbs");
      const cached = thumbCache.get(thumbKey(clip, thumbCount(used, used_total)));
      if (cached) {
        cached.forEach((src) => {
          const img = document.createElement(src ? "img" : "div");
          if (src) img.src = src;
          thumbs.appendChild(img);
        });
      } else {
        thumbs.innerHTML = '<span class="muted">縮圖讀取中…</span>';
      }
      block.addEventListener("click", (ev) => {
        const rect = block.getBoundingClientRect();
        const frac = Math.min(1, Math.max(0, (ev.clientX - rect.left) / rect.width));
        previewAt(item, (clip.start ?? 0) + frac * used);
      });
      timeline.appendChild(block);

      const mark = document.createElement("span");
      mark.style.flexGrow = widthOf(used);
      mark.textContent = fmtTime(placements[i].visible_at);
      ruler.appendChild(mark);
    });
    const end = document.createElement("span");
    end.className = "ruler-end";
    end.textContent = fmtTime(total);
    ruler.appendChild(end);
  }

  async function loadThumbs(seq) {
    const total = sumUsed(seq);
    for (const item of seq) {
      const clip = store[item.key];
      if (!clip) continue;
      const used = usedDuration(clip);
      const count = thumbCount(used, total);
      const key = thumbKey(clip, count);
      if (thumbCache.has(key)) continue;
      try {
        const thumbs = await invoke("video_thumbnails", { path: clip.path, start: clip.start ?? 0, duration: used, count });
        thumbCache.set(key, thumbs);
        draw(await panel.plan());
      } catch (_) {
        /* 縮圖失敗不影響輸出 */
      }
    }
  }

  function previewAt(item, seconds) {
    const clip = store[item.key];
    const player = el("Player");
    player.classList.remove("hidden");
    if (player.dataset.path !== clip.path) {
      player.src = convertFileSrc(clip.path);
      player.dataset.path = clip.path;
    }
    const seek = () => (player.currentTime = seconds);
    if (player.readyState >= 1) seek();
    else player.addEventListener("loadedmetadata", seek, { once: true });
    el("PlayerLabel").textContent = `預覽：${item.label} ${fmtTime(seconds)}`;
  }

  /** 決定輸出路徑；有設定資料夾就直接組，沒有就跳另存新檔。 */
  async function resolveOutputPath(nameInput) {
    if (!nameInput.value.trim()) nameInput.value = defaultName();
    const name = nameInput.value.trim();
    if (!name) throw new Error("請先輸入檔名");
    if (settings.output_dir) return invoke("output_path", { dir: settings.output_dir, name });
    const picked = await dialog.save({
      defaultPath: `${name.replace(/[<>:"/\\|?*]/g, "_")}.mp4`,
      filters: [{ name: "MP4", extensions: ["mp4"] }],
    });
    return picked || null;
  }

  /** @param validate 回傳錯誤訊息字串，沒問題回傳 null */
  panel.render = async (nameInput, validate) => {
    const status = el("Status");
    const problem = await validate();
    if (problem) return (status.textContent = `⚠ ${problem}`);

    let output;
    try {
      output = await resolveOutputPath(nameInput);
    } catch (e) {
      return (status.textContent = `⚠ ${e.message || e}`);
    }
    if (!output) return;

    // 檔案已存在要按第二次才覆蓋（另存新檔對話框已經問過就不再問）
    if (settings.output_dir && (await invoke("path_exists", { path: output })) && overwriteArmed !== output) {
      overwriteArmed = output;
      status.textContent = "⚠ 檔案已存在，再按一次「開始輸出」會覆蓋";
      return;
    }
    overwriteArmed = null;

    let encoder = settings.encoder;
    if (encoder === "auto") {
      status.textContent = "偵測硬體編碼…";
      try {
        encoder = await invoke("detect_encoder");
      } catch (e) {
        return (status.textContent = `⚠ ${e}`);
      }
    }

    const spec = baseSpec(panel.sequence(), { encoder, output });
    const progress = el("Progress");
    document.querySelectorAll(".render-btn").forEach((b) => (b.disabled = true));
    el("CancelBtn").classList.remove("hidden");
    el("PreviewBtn").classList.add("hidden");
    progress.classList.remove("hidden");
    progress.value = 0;
    const encoderName = { x264: "CPU", nvenc: "NVIDIA", qsv: "Intel", amf: "AMD" }[encoder] || encoder;
    status.textContent = `輸出中（${encoderName}）…`;
    const startedAt = Date.now();
    const unlisten = await listen("render-progress", ({ payload: p }) => {
      const frac = p.total ? Math.min(1, p.seconds / p.total) : 0;
      progress.value = frac;
      const elapsed = (Date.now() - startedAt) / 1000;
      const remain = frac > 0.02 ? (elapsed / frac) * (1 - frac) : null;
      status.textContent =
        `輸出中（${encoderName}）${Math.round(frac * 100)}%` + (remain != null ? ` · 剩約 ${fmtTime(remain)}` : "");
    });
    try {
      lastOutput = await invoke("render_video", { spec });
      progress.value = 1;
      status.textContent = `✓ 完成（花了 ${fmtTime((Date.now() - startedAt) / 1000)}）：${lastOutput}`;
      el("PreviewBtn").classList.remove("hidden");
    } catch (e) {
      status.textContent = `⚠ ${e}`;
    } finally {
      unlisten();
      document.querySelectorAll(".render-btn").forEach((b) => (b.disabled = false));
      el("CancelBtn").classList.add("hidden");
      if (progress.value < 1) progress.classList.add("hidden");
    }
  };

  el("CancelBtn").addEventListener("click", () => invoke("cancel_render"));
  el("PreviewBtn").addEventListener("click", () => {
    if (!lastOutput) return;
    const player = el("Player");
    player.classList.remove("hidden");
    player.src = convertFileSrc(lastOutput);
    player.dataset.path = lastOutput;
    player.play();
    el("PlayerLabel").textContent = `預覽成品：${baseName(lastOutput)}`;
  });

  return panel;
}

// --- 單集 ---

const single = createPanel(
  "single",
  () => [
    { key: "main", label: "完整影片", kind: "main" },
    { key: "yt", label: "YT 影片", kind: "yt", join: "flash" },
    { key: "promo45", label: "Promo 45 秒", kind: "promo", join: "cut" },
  ],
  () => $("partSelect").value,
);

$("pickMainBtn").addEventListener("click", async () => {
  const path = await pickVideo();
  if (!path) return;
  $("mainInfo").textContent = "讀取中…";
  try {
    const c = await setClip("main", path);
    $("mainInfo").textContent = `${baseName(path)} · ${describe(c.info)}`;
  } catch (e) {
    $("mainInfo").textContent = `⚠ ${e}`;
  }
});

$("ytDownloadBtn").addEventListener("click", async () => {
  const url = $("ytUrl").value.trim();
  if (!url) return ($("ytInfo").textContent = "請先貼上 YouTube 連結");
  const btn = $("ytDownloadBtn");
  const progress = $("ytProgress");
  btn.disabled = true;
  progress.classList.remove("hidden");
  progress.value = 0;
  $("ytInfo").textContent = "下載中…";
  const unlisten = await listen("yt-progress", ({ payload }) => {
    progress.value = payload.percent;
    $("ytInfo").textContent = `下載中 ${payload.percent.toFixed(0)}%`;
  });
  try {
    const path = await invoke("download_youtube", { url });
    const trim = await readYtTrim();
    await setClip("yt", path, { start: trim.start, end: trim.end });
    $("ytInfo").textContent = `已下載 · ${describe(store.yt.info)}`;
  } catch (e) {
    $("ytInfo").textContent = `⚠ ${e}`;
  } finally {
    unlisten();
    btn.disabled = false;
    progress.classList.add("hidden");
  }
});

$("ytClearBtn").addEventListener("click", () => {
  ["ytUrl", "ytStart", "ytEnd"].forEach((id) => ($(id).value = ""));
  $("ytInfo").textContent = "";
  setClip("yt", null);
});

/** 讀開始／結束欄位；格式錯誤的欄位標紅，ok 為 false。 */
async function readYtTrim() {
  const read = async (input) => {
    try {
      const v = await invoke("parse_timecode", { input: input.value });
      input.classList.remove("invalid");
      return { ok: true, value: v };
    } catch (e) {
      input.classList.add("invalid");
      input.title = String(e);
      return { ok: false, value: null };
    }
  };
  const [s, e] = await Promise.all([read($("ytStart")), read($("ytEnd"))]);
  return { ok: s.ok && e.ok, start: s.value, end: e.value };
}

async function applyYtTrim() {
  const trim = await readYtTrim();
  if (!store.yt) return trim.ok;
  store.yt.start = trim.start;
  store.yt.end = trim.end;
  if (trim.ok && usedDuration(store.yt) <= 0) {
    $("ytInfo").textContent = "⚠ 結束時間要晚於開始時間";
    return false;
  }
  single.refresh();
  return trim.ok;
}

let trimTimer = null;
["ytStart", "ytEnd"].forEach((id) =>
  $(id).addEventListener("input", () => {
    clearTimeout(trimTimer);
    trimTimer = setTimeout(applyYtTrim, 500);
  }),
);

function refreshPartOptions() {
  const select = $("partSelect");
  const current = select.value;
  const parts = lastRendered?.parts || [];
  select.replaceChildren(
    new Option(parts.length ? "選擇段落…" : "（文稿頁還沒有段落）", ""),
    ...parts.map((p) => new Option(`第${SEGMENT_NAMES[p.number - 1]}段：${p.title_line}`, p.title_line)),
  );
  if (parts.some((p) => p.title_line === current)) select.value = current;
}

$("partSelect").addEventListener("change", () => {
  if ($("partSelect").value) $("singleName").value = $("partSelect").value;
});

$("singleRenderBtn").addEventListener("click", () =>
  single.render($("singleName"), async () => {
    if (!store.main) return "請先選擇完整影片";
    if (!store.promo45) return "請先在設定指定 45 秒 promo 檔案";
    if ($("ytUrl").value.trim() && !store.yt) return "YT 連結還沒下載，請按「下載」或按「移除」";
    if (store.yt && !(await applyYtTrim())) return "YT 開始／結束時間有誤";
    return null;
  }),
);

// --- 全集 ---

const full = createPanel(
  "full",
  () => {
    const present = [1, 2, 3, 4, 5, 6].filter((n) => store[`s${n}`]);
    const items = [];
    present.forEach((n, i) => {
      items.push({ key: `s${n}`, label: `第${SEGMENT_NAMES[n - 1]}段`, kind: "main", join: "flash", segment: n });
      // 20 秒 promo 接在第一段（缺第一段時接在第一個有選的段落）後面
      if (i === 0) items.push({ key: "promo20", label: "Promo 20 秒", kind: "promo", join: "cut" });
    });
    if (present.length) items.push({ key: "promo45", label: "Promo 45 秒", kind: "promo", join: "cut" });
    return items;
  },
  () => `${$("date").value.trim()}關鍵時刻全集`,
);

$("fullSegments").replaceChildren(
  ...SEGMENT_NAMES.map((name, i) => {
    const n = i + 1;
    const row = document.createElement("div");
    row.className = "vrow";
    row.innerHTML = `<span class="vlabel">第${name}段</span>
      <button class="ghost">選擇檔案…</button>
      <button class="ghost small">移除</button>
      <span class="vinfo muted">尚未選擇</span>`;
    const [pick, clear] = row.querySelectorAll("button");
    const info = row.querySelector(".vinfo");
    pick.addEventListener("click", async () => {
      const path = await pickVideo();
      if (!path) return;
      info.textContent = "讀取中…";
      try {
        const c = await setClip(`s${n}`, path);
        info.textContent = `${baseName(path)} · ${describe(c.info)}`;
      } catch (e) {
        info.textContent = `⚠ ${e}`;
      }
    });
    clear.addEventListener("click", () => {
      info.textContent = "尚未選擇";
      setClip(`s${n}`, null);
    });
    return row;
  }),
);

/** 章節時間＝閃白結束、該段完全出現的那一秒（無條件進位，確保已經完全出現）。 */
$("fillChaptersBtn").addEventListener("click", async () => {
  const { seq, placements } = await full.plan();
  const filled = [];
  seq.forEach((item, i) => {
    if (!item.segment) return;
    const t = fmtTime(Math.ceil(placements[i].visible_at - 1e-6));
    segments[item.segment - 1].time.value = t;
    filled.push(`第${SEGMENT_NAMES[item.segment - 1]}段 ${t}`);
  });
  if (!filled.length) return ($("chapterInfo").textContent = "⚠ 還沒選任何段落");
  changed();
  $("chapterInfo").textContent = `已填入：${filled.join("、")}`;
});

$("fullRenderBtn").addEventListener("click", () =>
  full.render($("fullName"), async () => {
    if (![1, 2, 3, 4, 5, 6].some((n) => store[`s${n}`])) return "請至少選擇一段影片";
    if (!store.promo20) return "請先在設定指定 20 秒 promo 檔案";
    if (!store.promo45) return "請先在設定指定 45 秒 promo 檔案";
    return null;
  }),
);

const panels = [single, full];

// --- 輸出資料夾 ---

function refreshOutDir() {
  document.querySelectorAll(".out-dir-label").forEach((l) => {
    l.textContent = settings?.output_dir || "未指定（輸出時選擇存檔位置）";
  });
}

document.querySelectorAll(".pick-out-dir").forEach((btn) =>
  btn.addEventListener("click", async () => {
    const dir = await dialog.open({ directory: true, multiple: false });
    if (!dir) return;
    settingsForm.elements.output_dir.value = dir;
    settingsFieldChanged();
  }),
);

// --- 與 app.js 的接點 ---

/** 設定改變時（promo 檔案、閃白長度、音量、輸出資料夾）同步到影片頁。 */
window.onSettingsChanged = async () => {
  if (!settings) return;
  refreshOutDir();
  await refreshPromos();
  panels.forEach((p) => p.refresh());
};

document.querySelector('.tab[data-page="video"]').addEventListener("click", () => {
  refreshPartOptions();
  refreshTools();
});
document.querySelector('.tab[data-page="settings"]').addEventListener("click", refreshTools);

// app.js 可能已經先載好設定（設定讀取比這個檔案載入還快時），補跑一次
window.onSettingsChanged();
refreshTools();

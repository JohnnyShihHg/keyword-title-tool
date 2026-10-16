// 影片頁：單集 = 完整影片 →（YT 影片）→ promo，中間閃白。
// 和 app.js 共用全域的 invoke、$、settings、lastRendered。

const { listen } = window.__TAURI__.event;
const dialog = window.__TAURI__.dialog;
const { convertFileSrc } = window.__TAURI__.core;

const VIDEO_EXTENSIONS = ["mp4", "mov", "mkv", "m4v", "avi", "mxf", "webm"];
const OUTPUT = { width: 1920, height: 1080, fps: "30000/1001" };
/** 整條 timeline 大約放幾張縮圖 */
const TIMELINE_THUMBS = 36;

/**
 * 每個素材：{ path, info: ProbeInfo, start, end, thumbs: string[] }
 * start/end 只有 YT 影片會用（裁切）。
 */
const clips = { main: null, yt: null, promo: null };
const probeCache = new Map();

function fmtTime(sec) {
  const s = Math.max(0, Math.round(sec));
  const p = (n) => String(n).padStart(2, "0");
  return `${p(Math.floor(s / 3600))}:${p(Math.floor((s / 60) % 60))}:${p(s % 60)}`;
}

function fileName(path) {
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
    const path = await dialog.open({ multiple: false, filters: [{ name: "影片", extensions: VIDEO_EXTENSIONS }] });
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

// --- 素材 ---

async function setClip(slot, path, extra = {}) {
  if (!path) {
    clips[slot] = null;
    renderTimeline();
    return null;
  }
  const info = await probe(path);
  clips[slot] = { path, info, start: null, end: null, thumbs: [], ...extra };
  renderTimeline();
  loadThumbs();
  return info;
}

$("pickMainBtn").addEventListener("click", async () => {
  const path = await dialog.open({ multiple: false, filters: [{ name: "影片", extensions: VIDEO_EXTENSIONS }] });
  if (!path) return;
  $("mainInfo").textContent = "讀取中…";
  try {
    const info = await setClip("main", path);
    $("mainInfo").textContent = `${fileName(path)} · ${describe(info)}`;
  } catch (e) {
    $("mainInfo").textContent = `⚠ ${e}`;
  }
});

function selectedPromo() {
  return parseInt(document.querySelector('input[name="promo"]:checked')?.value || settings.default_promo, 10);
}

async function refreshPromo() {
  const secs = selectedPromo();
  const path = secs === 45 ? settings.promo_45_path : settings.promo_20_path;
  if (!path) {
    $("promoInfo").textContent = `⚠ 還沒指定 ${secs} 秒 promo，請到設定選擇檔案`;
    await setClip("promo", null);
    return;
  }
  if (clips.promo?.path === path) return;
  try {
    const info = await setClip("promo", path);
    $("promoInfo").textContent = `${fileName(path)} · ${describe(info)}`;
  } catch (e) {
    $("promoInfo").textContent = `⚠ ${e}`;
  }
}

document.querySelectorAll('input[name="promo"]').forEach((r) => r.addEventListener("change", refreshPromo));

$("ytDownloadBtn").addEventListener("click", async () => {
  const url = $("ytUrl").value.trim();
  if (!url) {
    $("ytInfo").textContent = "請先貼上 YouTube 連結";
    return;
  }
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
    await setClip("yt", path);
    await applyYtTrim();
    $("ytInfo").textContent = `已下載 · ${describe(clips.yt.info)}`;
  } catch (e) {
    $("ytInfo").textContent = `⚠ ${e}`;
  } finally {
    unlisten();
    btn.disabled = false;
    progress.classList.add("hidden");
  }
});

$("ytClearBtn").addEventListener("click", () => {
  $("ytUrl").value = "";
  $("ytStart").value = "";
  $("ytEnd").value = "";
  $("ytInfo").textContent = "";
  setClip("yt", null);
});

/** 讀開始／結束欄位套到 YT 素材；格式錯誤會標紅並回傳 false。 */
async function applyYtTrim() {
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
  if (!clips.yt) return s.ok && e.ok;
  clips.yt.start = s.value;
  clips.yt.end = e.value;
  if (s.ok && e.ok && usedDuration(clips.yt) <= 0) {
    $("ytInfo").textContent = "⚠ 結束時間要晚於開始時間";
    return false;
  }
  renderTimeline();
  loadThumbs(["yt"]);
  return s.ok && e.ok;
}

let trimTimer = null;
["ytStart", "ytEnd"].forEach((id) =>
  $(id).addEventListener("input", () => {
    clearTimeout(trimTimer);
    trimTimer = setTimeout(applyYtTrim, 500);
  }),
);

// --- Timeline ---

const SLOT_NAMES = { main: "完整影片", yt: "YT 影片", promo: "Promo" };

function sequence() {
  return ["main", "yt", "promo"].filter((k) => clips[k]).map((k) => ({ slot: k, clip: clips[k] }));
}

function transitionSecs() {
  return settings?.transition_secs ?? 0.5;
}

function renderTimeline() {
  const seq = sequence();
  const timeline = $("timeline");
  const ruler = $("ruler");
  if (!seq.length) {
    timeline.innerHTML = '<div class="timeline-empty muted">選擇完整影片後會顯示在這裡</div>';
    ruler.replaceChildren();
    $("timelineTotal").textContent = "";
    return;
  }
  const t = transitionSecs();
  const total = seq.reduce((sum, s) => sum + usedDuration(s.clip), 0) - t * (seq.length - 1);
  $("timelineTotal").textContent = `成品長度 ${fmtTime(total)}`;

  timeline.replaceChildren();
  ruler.replaceChildren();
  // 照實際比例的話 20 秒 promo 在 30 分鐘影片旁邊只剩一條線，所以每段至少佔 15% 寬
  const sumUsed = seq.reduce((sum, s) => sum + usedDuration(s.clip), 0);
  const widthOf = (used) => String(Math.max(used, sumUsed * 0.15));
  let outputStart = 0;
  seq.forEach(({ slot, clip }, i) => {
    const used = usedDuration(clip);
    if (i > 0) {
      const flash = document.createElement("div");
      flash.className = "flash";
      flash.title = `閃白 ${t} 秒`;
      timeline.appendChild(flash);
    }
    const block = document.createElement("div");
    block.className = `clip clip-${slot}`;
    block.style.flexGrow = widthOf(used);
    block.innerHTML = `<div class="clip-name">${SLOT_NAMES[slot]} · ${fmtTime(used)}</div><div class="thumbs"></div>`;
    block.title = `${SLOT_NAMES[slot]}：${fileName(clip.path)}（${fmtTime(used)}）`;
    const thumbs = block.querySelector(".thumbs");
    if (clip.thumbs.length) {
      clip.thumbs.forEach((src) => {
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
      previewAt(slot, (clip.start ?? 0) + frac * used);
    });
    timeline.appendChild(block);

    const mark = document.createElement("span");
    mark.style.flexGrow = widthOf(used);
    mark.textContent = fmtTime(outputStart);
    ruler.appendChild(mark);
    outputStart += used - t;
  });
  const end = document.createElement("span");
  end.className = "ruler-end";
  end.textContent = fmtTime(total);
  ruler.appendChild(end);
}

let thumbSeq = 0;
/** 依各段在 timeline 上的比例分配縮圖數量。 */
async function loadThumbs(slots = ["main", "yt", "promo"]) {
  const seq = sequence();
  const total = seq.reduce((sum, s) => sum + usedDuration(s.clip), 0);
  const mySeq = ++thumbSeq;
  for (const { slot, clip } of seq) {
    if (!slots.includes(slot)) continue;
    const used = usedDuration(clip);
    const count = Math.max(3, Math.round((TIMELINE_THUMBS * used) / total));
    try {
      const thumbs = await invoke("video_thumbnails", {
        path: clip.path,
        start: clip.start ?? 0,
        duration: used,
        count,
      });
      if (mySeq !== thumbSeq || clips[slot] !== clip) continue; // 期間素材換掉了
      clip.thumbs = thumbs;
      renderTimeline();
    } catch (_) {
      /* 縮圖失敗不影響輸出 */
    }
  }
}

function previewAt(slot, seconds) {
  const clip = clips[slot];
  if (!clip) return;
  const player = $("player");
  player.classList.remove("hidden");
  const src = convertFileSrc(clip.path);
  if (player.dataset.path !== clip.path) {
    player.src = src;
    player.dataset.path = clip.path;
  }
  const seek = () => {
    player.currentTime = seconds;
  };
  if (player.readyState >= 1) seek();
  else player.addEventListener("loadedmetadata", seek, { once: true });
  $("playerLabel").textContent = `預覽：${SLOT_NAMES[slot]} ${fmtTime(seconds)}`;
}

// --- 檔名與輸出 ---

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
  if ($("partSelect").value) $("fileName").value = $("partSelect").value;
});

function refreshOutDir() {
  $("outDirLabel").textContent = settings?.output_dir || "未指定（輸出時選擇存檔位置）";
}

$("pickOutDirBtn").addEventListener("click", async () => {
  const dir = await dialog.open({ directory: true, multiple: false });
  if (!dir) return;
  settingsForm.elements.output_dir.value = dir;
  settingsFieldChanged();
});

/** 決定輸出路徑；有設定資料夾就直接組，沒有就跳另存新檔。 */
async function resolveOutputPath() {
  const name = $("fileName").value.trim();
  if (!name) throw new Error("請先選擇或輸入檔名");
  if (settings.output_dir) return invoke("output_path", { dir: settings.output_dir, name });
  const picked = await dialog.save({
    defaultPath: `${name.replace(/[<>:"/\\|?*]/g, "_")}.mp4`,
    filters: [{ name: "MP4", extensions: ["mp4"] }],
  });
  return picked || null;
}

let overwriteArmed = null;
let lastOutput = null;

$("renderBtn").addEventListener("click", async () => {
  const status = $("renderStatus");
  if (!clips.main) return (status.textContent = "⚠ 請先選擇完整影片");
  if (!clips.promo) return (status.textContent = "⚠ 請先在設定指定 promo 檔案");
  if ($("ytUrl").value.trim() && !clips.yt) return (status.textContent = "⚠ YT 連結還沒下載，請按「下載」或按「移除」");
  if (clips.yt && !(await applyYtTrim())) return (status.textContent = "⚠ YT 開始／結束時間有誤");

  let output;
  try {
    output = await resolveOutputPath();
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

  const spec = {
    clips: sequence().map(({ clip }) => ({
      path: clip.path,
      duration: clip.info.duration,
      has_audio: clip.info.has_audio,
      start: clip.start,
      end: clip.end,
    })),
    transition: transitionSecs(),
    ...OUTPUT,
    encoder,
    output,
  };

  const progress = $("renderProgress");
  const btn = $("renderBtn");
  btn.disabled = true;
  $("cancelRenderBtn").classList.remove("hidden");
  $("previewOutputBtn").classList.add("hidden");
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
    $("previewOutputBtn").classList.remove("hidden");
  } catch (e) {
    status.textContent = `⚠ ${e}`;
  } finally {
    unlisten();
    btn.disabled = false;
    $("cancelRenderBtn").classList.add("hidden");
    if (progress.value < 1) progress.classList.add("hidden");
  }
});

$("cancelRenderBtn").addEventListener("click", () => invoke("cancel_render"));

$("previewOutputBtn").addEventListener("click", () => {
  if (!lastOutput) return;
  const player = $("player");
  player.classList.remove("hidden");
  player.src = convertFileSrc(lastOutput);
  player.dataset.path = lastOutput;
  player.play();
  $("playerLabel").textContent = `預覽成品：${fileName(lastOutput)}`;
});

// --- 與 app.js 的接點 ---

/** 設定改變時（promo 檔案、閃白長度、輸出資料夾）同步到影片頁。 */
window.onSettingsChanged = () => {
  if (!settings) return;
  if (!document.querySelector('input[name="promo"]:checked')) {
    const r = document.querySelector(`input[name="promo"][value="${settings.default_promo}"]`);
    if (r) r.checked = true;
  }
  refreshPromo();
  refreshOutDir();
  renderTimeline();
};

document.querySelector('.tab[data-page="video"]').addEventListener("click", () => {
  refreshPartOptions();
  refreshTools();
});
document.querySelector('.tab[data-page="settings"]').addEventListener("click", refreshTools);

// app.js 可能已經先載好設定（設定讀取比這個檔案載入還快時），補跑一次
window.onSettingsChanged();
refreshTools();

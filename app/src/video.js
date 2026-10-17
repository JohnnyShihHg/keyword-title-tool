// 影片頁。
//   單集：完整影片 ⚡ YT 影片（選填）｜ 45 秒 promo
//   全集：第一段 ｜ 20 秒 promo ⚡ 第二段 ⚡ … ⚡ 第六段 ｜ 45 秒 promo
// ⚡ = 閃白過場，｜ = 直接接。和 app.js 共用全域的 invoke、$、settings、lastRendered、
// segments、changed、SEGMENT_NAMES、settingsForm、settingsFieldChanged。

const { listen } = window.__TAURI__.event;
const dialog = window.__TAURI__.dialog;
const { convertFileSrc } = window.__TAURI__.core;

const ENCODER_NAMES = { x264: "CPU", nvenc: "NVIDIA", qsv: "Intel", amf: "AMD", mf: "Windows 硬體加速" };

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
  const row = (name, path) => `<div>${path ? "✓" : "✗"} ${name}：<span class="muted">${path || "未安裝"}</span></div>`;
  $("toolsStatus").innerHTML =
    row("ffmpeg", s.ffmpeg) +
    row("ffprobe", s.ffprobe) +
    row("yt-dlp", s.ytdlp) +
    (s.ffmpeg_outdated
      ? '<div class="tools-outdated">⚠ ffmpeg 有新的相容版本（支援較舊的 NVIDIA 顯示卡），請按「重新下載全部」</div>'
      : "");
  $("toolsWarning").innerHTML = s.ffmpeg_outdated
    ? '影片工具需要更新（讓舊的 NVIDIA 顯示卡也能硬體加速）。<button id="gotoToolsBtn2" class="small">前往設定下載</button>'
    : '還沒有影片工具（ffmpeg、yt-dlp）。<button id="gotoToolsBtn2" class="small">前往設定下載</button>';
  $("toolsWarning").classList.toggle("hidden", !!ok && !s.ffmpeg_outdated);
  $("gotoToolsBtn2").addEventListener("click", () => document.querySelector('.tab[data-page="settings"]').click());
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
    if (which !== "ytdlp") {
      $("encoderReport").textContent = "偵測中…";
      invoke("detect_encoder", { force: true }).then(showEncoderReport).catch(() => {});
    }
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
  // 輸出當下的時間軸（不含檔名、編碼器），用來判斷成品是不是目前時間軸的樣子
  let lastOutputKey = null;
  let lastOutputStamp = 0;
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
          signature: c.info.copy_signature,
          label: item.label,
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
    if (!seq.length) return { seq, placements: [], total: 0, fastBlockers: [] };
    const r = await invoke("plan_video", { spec: baseSpec(seq) });
    return { seq, placements: r.placements, total: r.total, fastBlockers: r.fast_blockers };
  };

  /** 時間軸的指紋：同樣的片段、裁切、接法、閃白秒數、音量就會一樣。 */
  function timelineKey(seq) {
    return JSON.stringify(baseSpec(seq));
  }

  /** 時間軸改過之後，「觀看成品」要講清楚播的是上次輸出的檔案。 */
  function updatePreviewBtn() {
    const btn = el("PreviewBtn");
    const stale = lastOutput && timelineKey(panel.sequence()) !== lastOutputKey;
    btn.textContent = stale ? "觀看上次輸出的成品（時間軸已變更，需重新輸出）" : "觀看成品";
    btn.title = lastOutput ? lastOutput : "";
  }

  panel.refresh = async () => {
    const mySeq = ++drawSeq;
    const plan = await panel.plan();
    if (mySeq !== drawSeq) return;
    draw(plan);
    updatePreviewBtn();
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

  function draw({ seq, placements, total, fastBlockers }) {
    // 時間軸播放跟著最新的時間軸走；播放中改了時間軸，下一格就照新的播
    tl.seq = seq;
    tl.placements = placements;
    tl.total = total;
    tl.blocks = [];
    if (tl.idx >= seq.length) tlStop();
    const timeline = el("Timeline");
    const ruler = el("Ruler");
    const fastInfo = el("FastInfo");
    if (fastInfo) {
      fastInfo.innerHTML = !seq.length
        ? ""
        : fastBlockers.length
          ? `<span class="fast-no">需要完整重新編碼（較慢）：</span>${fastBlockers.map(escapeHtml).join("；")}`
          : '<span class="fast-yes">⚡ 快速模式：畫面直接複製，只重新編碼聲音，幾十秒內完成</span>';
    }
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
      block.innerHTML = `<div class="clip-name">${item.label} · ${fmtTime(used)}</div><div class="thumbs"></div><div class="playhead hidden"></div>`;
      tl.blocks.push(block);
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
        tlGoto(i, clipRange(item).start + frac * used, false);
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

  // --- 播放時間軸：照順序連播原始檔，不用先輸出 ---
  //
  // 近似成品：閃白用白色遮罩模擬（前一段最後半個過場淡到白、下一段前半個過場從白淡出），
  // 不像 xfade 那樣兩段重疊，音量也沒有 +6dB。拿來確認順序、裁切點、有沒有放錯片。
  const tl = { idx: -1, seq: [], placements: [], total: 0, blocks: [], raf: 0, switching: false };

  function clipRange(item) {
    const c = store[item.key];
    const start = Math.min(c.start ?? 0, c.info.duration);
    return { start, end: start + usedDuration(c) };
  }

  function loadInto(player, path) {
    if (player.dataset.path === path && player.readyState >= 1) return Promise.resolve();
    return new Promise((resolve) => {
      player.addEventListener("loadedmetadata", () => resolve(), { once: true });
      player.dataset.path = path;
      player.src = convertFileSrc(path);
    });
  }

  async function tlGoto(i, seconds, play) {
    const item = tl.seq[i];
    if (!item || !store[item.key]) return tlStop();
    tl.idx = i;
    tl.switching = true;
    const player = el("Player");
    player.classList.remove("hidden");
    await loadInto(player, store[item.key].path);
    player.currentTime = seconds;
    tl.switching = false;
    if (play) await player.play().catch(() => {});
    tlTick();
  }

  function tlStop() {
    tl.idx = -1;
    if (tl.raf) cancelAnimationFrame(tl.raf);
    tl.raf = 0;
    el("Flash").style.opacity = "0";
    tl.blocks.forEach((b) => b.querySelector(".playhead")?.classList.add("hidden"));
  }

  function tlFrame() {
    tl.raf = 0;
    if (tl.idx < 0) return;
    tlTick();
    if (!el("Player").paused) tl.raf = requestAnimationFrame(tlFrame);
  }

  function tlTick() {
    const item = tl.seq[tl.idx];
    if (!item || tl.switching) return;
    const clip = store[item.key];
    if (!clip) return tlStop();
    const player = el("Player");
    const playing = !player.paused || player.ended;
    const { start, end } = clipRange(item);
    // 播放中把這格換了別的片，就重新載入
    if (player.dataset.path !== clip.path) return tlGoto(tl.idx, start, playing);
    const t = player.currentTime;
    if (t < start - 0.2) {
      player.currentTime = start;
      return;
    }

    const half = (settings?.transition_secs ?? 0.5) / 2;
    const next = tl.seq[tl.idx + 1];
    let white = 0;
    if (next && next.join !== "cut" && half > 0 && t > end - half) white = (t - (end - half)) / half;
    if (tl.idx > 0 && item.join !== "cut" && half > 0 && t < start + half) white = Math.max(white, 1 - (t - start) / half);
    el("Flash").style.opacity = String(Math.min(1, Math.max(0, white)));

    const used = end - start;
    tl.blocks.forEach((b, i) => {
      const head = b.querySelector(".playhead");
      if (!head) return;
      head.classList.toggle("hidden", i !== tl.idx);
      if (i === tl.idx) head.style.left = `${Math.min(100, Math.max(0, ((t - start) / Math.max(used, 0.001)) * 100))}%`;
    });
    const place = tl.placements[tl.idx];
    const outAt = place ? (place.transition_at ?? place.visible_at) + Math.max(0, t - start) : 0;
    el("PlayerLabel").textContent =
      `播放時間軸（尚未輸出，閃白與音量為近似）：${item.label} · 成品約 ${fmtTime(outAt)} / ${fmtTime(tl.total)}`;

    if (t >= end - 0.03 || player.ended) {
      if (next) tlGoto(tl.idx + 1, clipRange(next).start, playing);
      else {
        player.pause();
        el("Flash").style.opacity = "0";
      }
    }
  }

  {
    const player = el("Player");
    player.addEventListener("play", () => {
      if (tl.idx >= 0 && !tl.raf) tl.raf = requestAnimationFrame(tlFrame);
    });
    // rAF 在視窗不在前景時會停，timeupdate 補位
    player.addEventListener("timeupdate", () => tl.idx >= 0 && tlTick());
    player.addEventListener("seeked", () => tl.idx >= 0 && tlTick());
  }

  /** 播放器目前停在 key 那個檔案的第幾秒（原始檔時間）；播放器不在那個檔案就回傳 null。 */
  panel.playerTimeIn = (key) => {
    const player = el("Player");
    const clip = store[key];
    if (!clip || player.classList.contains("hidden") || player.dataset.path !== clip.path) return null;
    return player.currentTime;
  };

  el("PlayTimelineBtn").addEventListener("click", () => {
    if (!tl.seq.length) return;
    tlGoto(0, clipRange(tl.seq[0]).start, true);
  });

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
        const report = await invoke("detect_encoder", { force: false });
        showEncoderReport(report);
        encoder = report.chosen;
      } catch (e) {
        return (status.textContent = `⚠ ${e}`);
      }
    }

    const renderSeq = panel.sequence();
    const renderKey = timelineKey(renderSeq);
    const spec = baseSpec(renderSeq, { encoder, output });
    const progress = el("Progress");
    document.querySelectorAll(".render-btn").forEach((b) => (b.disabled = true));
    el("CancelBtn").classList.remove("hidden");
    el("PreviewBtn").classList.add("hidden");
    progress.classList.remove("hidden");
    progress.value = 0;
    const encoderName = ENCODER_NAMES[encoder] || encoder;
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
      const result = await invoke("render_video", { spec });
      lastOutput = result.output;
      lastOutputKey = renderKey;
      lastOutputStamp = Date.now();
      progress.value = 1;
      const mode = result.fast ? "快速模式，畫面未重新編碼" : encoderName;
      status.textContent = `✓ 完成（${mode}，花了 ${fmtTime((Date.now() - startedAt) / 1000)}）：${lastOutput}`;
      el("PreviewBtn").classList.remove("hidden");
      updatePreviewBtn();
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
    tlStop();
    const player = el("Player");
    player.classList.remove("hidden");
    // 覆蓋同一個檔名時網址不變，播放器會沿用舊檔的快取；加上輸出時間讓它重新讀檔
    player.src = `${convertFileSrc(lastOutput)}?v=${lastOutputStamp}`;
    player.dataset.path = lastOutput;
    player.load();
    player.play();
    el("PlayerLabel").textContent = `觀看成品：${baseName(lastOutput)}`;
  });

  return panel;
}

// --- 單集：積木 ---
//
// blocks 依序排列；type 為 video / yt / promo20 / promo45 / flash。
// 兩個影片積木中間有閃白積木就閃白，沒有就直接接。積木的影片資料放在 store[block.id]。

const BLOCKS_KEY = "keyword-title-blocks-v1";
const BLOCK_TYPES = {
  video: { label: "影片檔", kind: "main" },
  yt: { label: "YT 影片", kind: "yt" },
  promo20: { label: "Promo 20 秒", kind: "promo" },
  promo45: { label: "Promo 45 秒", kind: "promo" },
  flash: { label: "⚡ 閃白", kind: "flash" },
};
const PRESETS = {
  basic: ["video", "promo45"],
  yt: ["video", "flash", "yt", "promo45"],
};

/** @type {{id: string, type: string, path?: string, url?: string, startText?: string, endText?: string, status?: string}[]} */
let blocks = [];
let blockSeq = 0;
const newId = () => `b${Date.now().toString(36)}${blockSeq++}`;

function saveBlocks() {
  try {
    const plain = blocks.map(({ id, type, path, url, startText, endText }) => ({ id, type, path, url, startText, endText }));
    localStorage.setItem(BLOCKS_KEY, JSON.stringify(plain));
  } catch (_) {
    /* 存不了不影響使用 */
  }
}

function promoPath(type) {
  return type === "promo20" ? settings?.promo_20_path : type === "promo45" ? settings?.promo_45_path : "";
}

/** 依積木順序組出 timeline / 輸出用的序列。 */
function blockSequence() {
  const items = [];
  let flashPending = false;
  for (const b of blocks) {
    if (b.type === "flash") {
      if (items.length) flashPending = true;
      continue;
    }
    if (!store[b.id]) continue;
    const t = BLOCK_TYPES[b.type];
    const label = b.type === "video" && store[b.id] ? baseName(store[b.id].path).replace(/\.[^.]+$/, "") : t.label;
    items.push({ key: b.id, label, kind: t.kind, join: flashPending ? "flash" : "cut" });
    flashPending = false;
  }
  return items;
}

/** 閃白積木有沒有作用：前後都要有影片積木（中間不能隔另一個閃白）。 */
function flashIsEffective(index) {
  const prev = blocks[index - 1];
  const next = blocks[index + 1];
  return !!(prev && next && prev.type !== "flash" && next.type !== "flash");
}

function addBlock(type, at = blocks.length) {
  const b = { id: newId(), type, startText: "", endText: "" };
  blocks.splice(at, 0, b);
  if (type === "promo20" || type === "promo45") loadPromoBlock(b);
  renderBlocks();
  saveBlocks();
  return b;
}

async function loadPromoBlock(b) {
  const path = promoPath(b.type);
  if (!path) {
    b.status = `⚠ 還沒指定 ${b.type === "promo20" ? 20 : 45} 秒 promo，請到設定選擇檔案`;
    await setClip(b.id, null);
  } else if (store[b.id]?.path !== path) {
    try {
      await setClip(b.id, path);
      b.status = `${baseName(path)} · ${describe(store[b.id].info)}`;
    } catch (e) {
      b.status = `⚠ ${e}`;
    }
  }
  renderBlocks();
}

async function loadFileBlock(b, path) {
  b.path = path;
  b.status = "讀取中…";
  renderBlocks();
  try {
    const trim = await readTrim(b);
    await setClip(b.id, path, { start: trim.start, end: trim.end });
    b.status = `${baseName(path)} · ${describe(store[b.id].info)}`;
  } catch (e) {
    b.status = `⚠ ${e}`;
    await setClip(b.id, null);
  }
  renderBlocks();
  saveBlocks();
}

async function downloadYtBlock(b) {
  if (!b.url?.trim()) {
    b.status = "請先貼上 YouTube 連結";
    return renderBlocks();
  }
  b.status = "下載中…";
  b.downloading = true;
  renderBlocks();
  const unlisten = await listen("yt-progress", ({ payload }) => {
    const el = document.querySelector(`[data-block="${b.id}"] .block-status`);
    if (el) el.textContent = `下載中 ${payload.percent.toFixed(0)}%`;
  });
  try {
    const path = await invoke("download_youtube", { url: b.url });
    await loadFileBlock(b, path);
  } catch (e) {
    b.status = `⚠ ${e}`;
  } finally {
    unlisten();
    b.downloading = false;
    renderBlocks();
  }
}

/** 讀積木的開始／結束時間；格式錯誤時 ok 為 false。 */
async function readTrim(b) {
  const parse = async (text) => {
    try {
      return { ok: true, value: await invoke("parse_timecode", { input: text || "" }) };
    } catch (e) {
      return { ok: false, value: null, error: String(e) };
    }
  };
  const [s, e] = await Promise.all([parse(b.startText), parse(b.endText)]);
  return { ok: s.ok && e.ok, start: s.value, end: e.value, error: s.error || e.error };
}

async function applyTrim(b) {
  const trim = await readTrim(b);
  const card = document.querySelector(`[data-block="${b.id}"]`);
  card?.querySelectorAll(".time").forEach((i) => i.classList.toggle("invalid", !trim.ok));
  if (!trim.ok || !store[b.id]) return trim.ok;
  store[b.id].start = trim.start;
  store[b.id].end = trim.end;
  if (usedDuration(store[b.id]) <= 0) {
    card?.querySelectorAll(".time").forEach((i) => i.classList.add("invalid"));
    return false;
  }
  single.refresh();
  return true;
}

function moveBlock(from, to) {
  if (to < 0 || to >= blocks.length || from === to) return;
  const [b] = blocks.splice(from, 1);
  blocks.splice(to, 0, b);
  renderBlocks();
  saveBlocks();
  single.refresh();
}

let dragFrom = null;

function renderBlocks() {
  const list = $("blockList");
  if (!blocks.length) {
    list.innerHTML = '<div class="block-empty muted">還沒有積木。用上面的「加入」或「常用組合」開始。</div>';
    single.refresh();
    return;
  }
  list.replaceChildren(
    ...blocks.map((b, i) => {
      const card = document.createElement("div");
      card.className = `block block-${b.type}`;
      card.dataset.block = b.id;
      card.draggable = true;
      const t = BLOCK_TYPES[b.type];
      const head = `<span class="block-grip" title="拖拉調整順序">⋮⋮</span><strong class="block-title">${t.label}</strong>`;
      const tools = `<span class="spacer"></span>
        <button class="ghost small" data-act="up" title="上移">↑</button>
        <button class="ghost small" data-act="down" title="下移">↓</button>
        <button class="ghost small danger" data-act="remove" title="移除">✕</button>`;
      if (b.type === "flash") {
        const ok = flashIsEffective(i);
        card.innerHTML = `${head}<span class="muted">${settings?.transition_secs ?? 0.5} 秒</span>
          ${ok ? "" : '<span class="block-warn">⚠ 前後都要有影片才有作用</span>'}${tools}`;
      } else {
        const trim =
          b.type === "video" || b.type === "yt"
            ? `<label class="field">開始 <input class="time" data-field="startText" placeholder="00:00:00" value="${b.startText || ""}"></label>
               <label class="field">結束 <input class="time" data-field="endText" placeholder="到結尾" value="${b.endText || ""}"></label>`
            : "";
        const source =
          b.type === "video"
            ? `<button class="ghost small" data-act="pick">${b.path ? "換檔案…" : "選擇檔案…"}</button>`
            : b.type === "yt"
              ? `<input class="grow yt-url" data-field="url" placeholder="貼上 YouTube 連結" value="${escapeHtml(b.url || "")}">
                 <button class="ghost small" data-act="download" ${b.downloading ? "disabled" : ""}>下載</button>`
              : "";
        card.innerHTML = `<div class="block-row">${head}${source}${tools}</div>
          <div class="block-row">${trim}<span class="block-status vinfo muted">${escapeHtml(b.status || (b.type === "video" ? "尚未選擇" : ""))}</span></div>`;
      }

      card.addEventListener("click", async (ev) => {
        const act = ev.target.dataset?.act;
        if (!act) return;
        if (act === "up") moveBlock(i, i - 1);
        if (act === "down") moveBlock(i, i + 1);
        if (act === "remove") {
          blocks.splice(i, 1);
          delete store[b.id];
          renderBlocks();
          saveBlocks();
          single.refresh();
        }
        if (act === "pick") {
          const path = await pickVideo();
          if (path) loadFileBlock(b, path);
        }
        if (act === "download") downloadYtBlock(b);
      });
      card.querySelectorAll("[data-field]").forEach((input) =>
        input.addEventListener("input", () => {
          b[input.dataset.field] = input.value;
          saveBlocks();
          if (input.classList.contains("time")) {
            clearTimeout(b.trimTimer);
            b.trimTimer = setTimeout(() => applyTrim(b), 500);
          }
        }),
      );
      card.addEventListener("dragstart", (ev) => {
        // 在輸入框裡拖曳是選字，不是搬積木
        if (ev.target.closest("input")) return ev.preventDefault();
        dragFrom = i;
        ev.dataTransfer.effectAllowed = "move";
        ev.dataTransfer.setData("text/plain", b.id);
        card.classList.add("dragging");
      });
      card.addEventListener("dragend", () => card.classList.remove("dragging"));
      card.addEventListener("dragover", (ev) => {
        ev.preventDefault();
        ev.dataTransfer.dropEffect = "move";
        card.classList.add("drop-target");
      });
      card.addEventListener("dragleave", () => card.classList.remove("drop-target"));
      card.addEventListener("drop", (ev) => {
        ev.preventDefault();
        card.classList.remove("drop-target");
        if (dragFrom !== null) moveBlock(dragFrom, i);
        dragFrom = null;
      });
      return card;
    }),
  );
  single.refresh();
}

const single = createPanel("single", blockSequence, () => $("partSelect").value);

document.querySelectorAll("[data-add]").forEach((btn) =>
  btn.addEventListener("click", async () => {
    const b = addBlock(btn.dataset.add);
    if (b.type === "video") {
      const path = await pickVideo();
      if (path) loadFileBlock(b, path);
    }
  }),
);

document.querySelectorAll("[data-preset]").forEach((btn) =>
  btn.addEventListener("click", async () => {
    blocks.forEach((b) => delete store[b.id]);
    blocks = [];
    PRESETS[btn.dataset.preset].forEach((type) => addBlock(type));
    const first = blocks.find((b) => b.type === "video");
    const path = first && (await pickVideo());
    if (path) loadFileBlock(first, path);
  }),
);

$("clearBlocksBtn").addEventListener("click", async () => {
  if (!blocks.length) return;
  const ok = await dialog.ask("確定要移除單集的所有積木嗎？", { title: "全部移除", kind: "warning", okLabel: "移除", cancelLabel: "取消" });
  if (!ok) return;
  blocks.forEach((b) => delete store[b.id]);
  blocks = [];
  renderBlocks();
  saveBlocks();
});

/** 啟動時還原上次的積木（影片檔重新讀取；找不到的檔案會標示錯誤）。 */
async function restoreBlocks() {
  let saved = [];
  try {
    saved = JSON.parse(localStorage.getItem(BLOCKS_KEY) || "[]");
  } catch (_) {
    saved = [];
  }
  blocks = saved.length ? saved : PRESETS.basic.map((type) => ({ id: newId(), type, startText: "", endText: "" }));
  renderBlocks();
  for (const b of blocks) {
    if (b.type === "promo20" || b.type === "promo45") await loadPromoBlock(b);
    else if (b.path) await loadFileBlock(b, b.path);
  }
}

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
    if (!blockSequence().length) return "請先加入影片積木並選擇檔案";
    for (const b of blocks) {
      const name = BLOCK_TYPES[b.type].label;
      if (b.type === "video" && !store[b.id]) return `有一塊「${name}」還沒選檔案，請選擇或移除`;
      if (b.type === "yt" && !store[b.id]) return "有一塊「YT 影片」還沒下載，請按「下載」或移除";
      if ((b.type === "promo20" || b.type === "promo45") && !store[b.id]) return `請先在設定指定「${name}」的檔案`;
      if ((b.type === "video" || b.type === "yt") && !(await applyTrim(b))) return `「${name}」的開始／結束時間有誤`;
    }
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

// 砍掉片尾：單集輸出的檔案尾巴接著 45 秒 promo（第三、六段前面還多接了 YT），
// 全集要把它砍掉再接。第一、二、四、五段砍最後 promo_tail_secs 秒；第三、六段
// 由使用者指定從哪個時間點以後都不要。預設勾選：放進全集的幾乎都是單集輸出的成品。
const CUT_AT_SEGMENTS = new Set([3, 6]);
const segTrim = Object.fromEntries([1, 2, 3, 4, 5, 6].map((n) => [n, { on: true, text: "", ok: true, error: "" }]));
const segRows = {};

/** 依勾選狀態算出第 n 段的結束點，寫回 store，並更新該列的說明。 */
async function applySegTrim(n) {
  const clip = store[`s${n}`];
  const t = segTrim[n];
  const row = segRows[n];
  t.ok = true;
  t.error = "";
  if (clip) {
    const duration = clip.info.duration;
    let end = null;
    if (t.on && CUT_AT_SEGMENTS.has(n)) {
      if (!t.text.trim()) {
        t.ok = false;
        t.error = "請填入從哪個時間點以後不要";
      } else {
        try {
          end = await invoke("parse_timecode", { input: t.text });
        } catch (e) {
          t.ok = false;
          t.error = `時間格式不對：${e}`;
        }
        if (t.ok && !(end > 0 && end < duration)) {
          t.ok = false;
          t.error = `時間點要在 00:00:00 和檔案長度 ${fmtTime(duration)} 之間`;
        }
      }
    } else if (t.on) {
      end = duration - (settings?.promo_tail_secs ?? 45);
      if (!(end > 0)) {
        t.ok = false;
        t.error = `檔案只有 ${fmtTime(duration)}，比片尾要砍的 ${settings?.promo_tail_secs ?? 45} 秒還短`;
      }
    }
    clip.end = t.ok ? end : null;
  }
  if (row) {
    row.timeInput?.classList.toggle("invalid", !t.ok);
    row.trimInfo.textContent = !clip || !t.on
      ? ""
      : !t.ok
        ? `⚠ ${t.error}`
        : `使用到 ${fmtTime(clip.end)}，後面 ${fmtTime(clip.info.duration - clip.end)} 不要`;
    row.trimInfo.classList.toggle("warn", !t.ok);
  }
  full.refresh();
}

async function applyAllSegTrims() {
  for (const n of [1, 2, 3, 4, 5, 6]) await applySegTrim(n);
}

$("fullSegments").replaceChildren(
  ...SEGMENT_NAMES.map((name, i) => {
    const n = i + 1;
    const cutAt = CUT_AT_SEGMENTS.has(n);
    const row = document.createElement("div");
    row.className = "vrow seg-row";
    row.innerHTML = `<span class="vlabel">第${name}段</span>
      <button class="ghost">選擇檔案…</button>
      <button class="ghost small">移除</button>
      <label class="seg-trim" title="${cutAt ? "從指定時間點以後都不要（YT 與片尾 promo）" : "砍掉檔案最後的片尾 promo，秒數在設定裡調整"}">
        <input type="checkbox" class="seg-trim-on" checked /> 砍掉片尾
      </label>
      ${cutAt ? `<span class="seg-cut-at">從 <input class="time seg-trim-time" placeholder="00:00:00" /> 以後都不要
        <button class="ghost small seg-use-pos" title="用「播放時間軸」播到要切的地方暫停，再按這顆">用目前播放位置</button></span>` : ""}
      <span class="vinfo muted seg-info">尚未選擇</span>
      <span class="vinfo muted seg-trim-info"></span>`;
    const [pick, clear] = row.querySelectorAll("button");
    const info = row.querySelector(".seg-info");
    const onBox = row.querySelector(".seg-trim-on");
    const cutAtBox = row.querySelector(".seg-cut-at");
    const timeInput = row.querySelector(".seg-trim-time");
    segRows[n] = { timeInput, trimInfo: row.querySelector(".seg-trim-info") };

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
      await applySegTrim(n);
    });
    clear.addEventListener("click", async () => {
      info.textContent = "尚未選擇";
      await setClip(`s${n}`, null);
      await applySegTrim(n);
    });
    onBox.addEventListener("change", () => {
      segTrim[n].on = onBox.checked;
      cutAtBox?.classList.toggle("hidden", !onBox.checked);
      applySegTrim(n);
    });
    timeInput?.addEventListener("input", () => {
      segTrim[n].text = timeInput.value;
      applySegTrim(n);
    });
    row.querySelector(".seg-use-pos")?.addEventListener("click", async () => {
      const t = full.playerTimeIn(`s${n}`);
      if (t == null) {
        segRows[n].trimInfo.textContent = `⚠ 請先用「播放時間軸」或點時間軸，播到第${name}段要切的地方`;
        return;
      }
      timeInput.value = fmtTime(Math.floor(t));
      segTrim[n].text = timeInput.value;
      await applySegTrim(n);
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
    for (const n of [1, 2, 3, 4, 5, 6]) {
      const t = segTrim[n];
      if (store[`s${n}`] && t.on && !t.ok) return `第${SEGMENT_NAMES[n - 1]}段砍掉片尾：${t.error}`;
    }
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

// --- 硬體編碼偵測結果（設定頁） ---

function showEncoderReport(r) {
  const lines = r.attempts.map(
    (a) => `<div>${a.ok ? "✓" : "✗"} ${a.label}${a.ok ? "：可以使用" : `：<span class="muted">${escapeHtml(a.error || "失敗")}</span>`}</div>`,
  );
  const tried = r.attempts.map((a) => a.encoder);
  if (r.chosen === "x264") lines.push("<div>→ 都無法使用，改用 CPU 編碼</div>");
  else if (tried.length < 4) lines.push('<div class="muted">（找到可用的就不再往下測）</div>');
  $("encoderReport").innerHTML = `<div><strong>目前使用：${ENCODER_NAMES[r.chosen]}</strong></div>` + lines.join("");
}

function escapeHtml(s) {
  return s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
}

$("redetectEncoderBtn").addEventListener("click", async () => {
  const btn = $("redetectEncoderBtn");
  btn.disabled = true;
  $("encoderReport").textContent = "偵測中…";
  try {
    showEncoderReport(await invoke("detect_encoder", { force: true }));
  } catch (e) {
    $("encoderReport").textContent = `⚠ ${e}`;
  } finally {
    btn.disabled = false;
  }
});

// --- 與 app.js 的接點 ---

/** 設定改變時（promo 檔案、閃白長度、音量、輸出資料夾）同步到影片頁。 */
window.onSettingsChanged = async () => {
  if (!settings) return;
  refreshOutDir();
  await refreshPromos();
  await applyAllSegTrims();
  for (const b of blocks) if (b.type === "promo20" || b.type === "promo45") await loadPromoBlock(b);
  panels.forEach((p) => p.refresh());
};

document.querySelector('.tab[data-page="video"]').addEventListener("click", () => {
  refreshPartOptions();
  refreshTools();
});
document.querySelector('.tab[data-page="settings"]').addEventListener("click", refreshTools);

// app.js 可能已經先載好設定（設定讀取比這個檔案載入還快時），補跑一次
window.onSettingsChanged();
restoreBlocks();
refreshTools();

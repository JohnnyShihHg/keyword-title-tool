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

/** 精確到小數兩位：00:15:20.43 */
function fmtPrecise(sec) {
  const cs = Math.max(0, Math.round(sec * 100));
  const p = (n) => String(n).padStart(2, "0");
  const s = Math.floor(cs / 100);
  return `${p(Math.floor(s / 3600))}:${p(Math.floor((s / 60) % 60))}:${p(s % 60)}.${p(cs % 100)}`;
}

/**
 * 解析切點時間，全形數字與冒號也可；格式不對回傳 null。
 *   0456 → 4 分 56 秒、015620 → 1 小時 56 分 20 秒（純數字從右邊每兩位一組：秒、分、時）
 *   04:56、1:56:20、0456.5、04:56.43 也可以
 */
function parsePreciseTime(text) {
  const t = String(text ?? "")
    .replace(/[\uFF10-\uFF19\uFF1A\uFF0E]/g, (c) => String.fromCharCode(c.charCodeAt(0) - 0xfee0))
    .replace(/\s+/g, "");
  let parts;
  let frac = 0;
  if (/^\d+(\.\d+)?$/.test(t)) {
    const [digits, dec] = t.split(".");
    if (digits.length > 6) return null;
    const padded = digits.padStart(Math.ceil(digits.length / 2) * 2, "0");
    parts = padded.match(/\d\d/g).map(Number);
    frac = dec ? parseFloat(`0.${dec}`) : 0;
  } else if (/^\d+(:\d{1,2}){1,2}(\.\d+)?$/.test(t)) {
    const [main, dec] = t.split(".");
    parts = main.split(":").map(Number);
    frac = dec ? parseFloat(`0.${dec}`) : 0;
  } else {
    return null;
  }
  // 分、秒超過 59 多半是打錯（例如 0475），不要默默換算
  if (parts.slice(1).some((n) => n > 59) || (parts.length > 1 && parts[parts.length - 1] > 59)) return null;
  return parts.reduce((acc, n) => acc * 60 + n, 0) + frac;
}

// --- 完成提示：提示框＋提示音 ---
//
// 提示音用 Web Audio 即時合成（幾個正弦／三角波加上音量包絡），不用音效檔，沒有版權問題。
// 每個音：[相對開始秒數, 頻率 Hz, 長度秒, 波形]

const CHIMES = {
  chime: { name: "叮咚", notes: [[0, 1046.5, 0.5, "sine"], [0.16, 784, 0.7, "sine"]] },
  bell: { name: "清脆鈴聲", notes: [[0, 1318.5, 0.9, "sine"], [0, 2637, 0.5, "sine"]] },
  rise: { name: "上揚三音", notes: [[0, 523.3, 0.25, "triangle"], [0.11, 659.3, 0.25, "triangle"], [0.22, 784, 0.5, "triangle"]] },
  pop: { name: "柔和泡泡", notes: [[0, 587.3, 0.18, "sine"], [0.09, 880, 0.3, "sine"]] },
  done: { name: "完成小號", notes: [[0, 523.3, 0.14, "triangle"], [0.14, 523.3, 0.14, "triangle"], [0.28, 784, 0.6, "triangle"]] },
};

let audioCtx = null;

function playChime(id = settings?.notify_sound_id, volume = settings?.notify_volume ?? 60) {
  const chime = CHIMES[id] || CHIMES.chime;
  try {
    audioCtx ||= new AudioContext();
    const now = audioCtx.currentTime + 0.02;
    const peak = Math.max(0, Math.min(1, volume / 100)) * 0.35;
    for (const [at, freq, len, type] of chime.notes) {
      const osc = audioCtx.createOscillator();
      const gain = audioCtx.createGain();
      osc.type = type;
      osc.frequency.value = freq;
      gain.gain.setValueAtTime(0.0001, now + at);
      gain.gain.exponentialRampToValueAtTime(Math.max(peak, 0.0001), now + at + 0.012);
      gain.gain.exponentialRampToValueAtTime(0.0001, now + at + len);
      osc.connect(gain).connect(audioCtx.destination);
      osc.start(now + at);
      osc.stop(now + at + len + 0.05);
    }
  } catch (_) {
    /* 沒有音效裝置也不影響 */
  }
}

/**
 * 跳出完成提示框（設定關掉時不跳，回傳 false）。有 okText 就是詢問框，回傳使用者是否按了確定。
 * 提示音和提示框分開設定：只開提示音時照樣會響。
 */
function showNotice({ title, message, okText = "好", cancelText = null, input = null }) {
  if (settings?.notify_sound) playChime();
  if (!settings?.notify_popup) return Promise.resolve(false);
  const field = $("noticeInput");
  $("noticeField").classList.toggle("hidden", !input);
  if (input) {
    $("noticeInputLabel").textContent = input.label;
    field.value = input.value ?? "";
  }
  const box = $("notice");
  $("noticeTitle").textContent = title;
  $("noticeMessage").textContent = message;
  const ok = $("noticeOk");
  const cancel = $("noticeCancel");
  ok.textContent = okText;
  cancel.textContent = cancelText || "";
  cancel.classList.toggle("hidden", !cancelText);
  // 重播勾勾動畫
  const check = box.querySelector(".notice-check");
  check.replaceWith(check.cloneNode(true));
  box.classList.remove("hidden");
  if (input) {
    field.focus();
    field.select();
  } else ok.focus();
  return new Promise((resolve) => {
    const close = (answer) => {
      box.classList.add("hidden");
      ok.onclick = cancel.onclick = box.onclick = document.onkeydown = null;
      resolve(answer);
    };
    ok.onclick = () => close(input ? { value: field.value.trim() } : true);
    field.onkeydown = (e) => {
      if (e.key === "Enter") ok.click();
    };
    cancel.onclick = () => close(false);
    box.onclick = (e) => e.target === box && close(false);
    document.onkeydown = (e) => {
      if (e.key === "Escape") close(false);
    };
  });
}

{
  const select = $("notifySoundSelect");
  select.replaceChildren(...Object.entries(CHIMES).map(([id, c]) => new Option(c.name, id)));
  select.addEventListener("change", () => playChime(select.value, +settingsForm.elements.notify_volume.value));
  $("notifySoundTest").addEventListener("click", () =>
    playChime(select.value, +settingsForm.elements.notify_volume.value),
  );
}

function baseName(path) {
  return path.split(/[\\/]/).pop();
}

function usedDuration(c) {
  const start = Math.min(c.start ?? 0, c.info.duration);
  const end = Math.min(c.end ?? c.info.duration, c.info.duration);
  return Math.max(0, end - start);
}

/** 素材說明只留長度：輸出一律重新編碼成 1080p 29.97，格式不用看；格式衝突由快速模式那行提示。 */
function describe(info) {
  const audio = info.has_audio ? "" : " · 無聲音";
  return `${fmtTime(info.duration)}${audio}`;
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
 * @param buildSequence 回傳 [{ key, label, kind: main|yt|promo, join: flash|cut, gain_db?, segment? }]，
 *                      沒選檔案的項目會自動略過
 * @param defaultName 檔名欄空白時的預設檔名
 */
/**
 * @param hooks.setEnd(item, end) 切點微調器改了某段的結束點時呼叫（寫回該段的設定並重算）
 * @param hooks.resetEnd(item) 還原自動切點；hooks.isManual(item) 這段是不是手動切點
 */
function createPanel(prefix, buildSequence, defaultName, hooks = {}) {
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
          gain_db: item.gain_db || 0,
        };
      }),
      transition: settings?.transition_secs ?? 0.5,
      ...OUTPUT,
      encoder: "x264",
      output: "",
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
          ? `<span class="fast-no">素材格式不一致，需要重新編碼（較慢）：</span>${fastBlockers.map(escapeHtml).join("；")}`
          : '<span class="fast-yes">⚡ 快速模式：畫面直接複製，幾十秒內完成</span>';
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
        join.title = `${item.join === "cut" ? "直接接" : `閃白 ${settings.transition_secs} 秒`}（點一下微調這個接點）`;
        join.addEventListener("click", () => panel.openTrim(seq[i - 1].key));
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

  /** 這段原始畫面在成品上從第幾秒開始：閃白置中在切點上，所以是過場的正中間。 */
  function contentAt(p) {
    return p.transition_at != null ? p.transition_at + (settings?.transition_secs ?? 0.5) / 2 : p.visible_at;
  }

  /** 成品時間 → 第幾段、該段原始檔的第幾秒 */
  function locateOut(out) {
    let idx = 0;
    tl.placements.forEach((p, i) => {
      if (contentAt(p) <= out) idx = i;
    });
    const p = tl.placements[idx];
    const item = tl.seq[idx];
    if (!p || !item) return null;
    const { start, end } = clipRange(item);
    return { idx, seconds: Math.min(end - 0.04, start + Math.max(0, out - contentAt(p))) };
  }

  let outDragging = false;
  function showOutPos(out) {
    el("OutBar").classList.remove("hidden");
    const range = el("OutRange");
    range.max = String(Math.max(tl.total, 0.01));
    if (!outDragging) range.value = String(out);
    el("OutTime").textContent = `${fmtPrecise(out)} / ${fmtPrecise(tl.total)}`;
  }

  {
    const range = el("OutRange");
    range.addEventListener("input", () => {
      outDragging = true;
      el("OutTime").textContent = `${fmtPrecise(+range.value)} / ${fmtPrecise(tl.total)}`;
    });
    range.addEventListener("change", () => {
      outDragging = false;
      const at = locateOut(+range.value);
      if (at) tlGoto(at.idx, at.seconds, !el("Player").paused);
    });
  }

  function tlStop() {
    el("OutBar").classList.add("hidden");
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

    // 預覽的閃白和輸出一樣置中在切點：前一段最後半個過場淡到白，下一段開頭半個過場從白淡出
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
    const outAt = place ? contentAt(place) + Math.max(0, t - start) : 0;
    el("PlayerLabel").textContent =
      `播放時間軸（尚未輸出，閃白與音量為近似）：${item.label}。播放器自己的進度條是原始檔長度，請看上面的「成品」進度條`;
    showOutPos(outAt);

    // 播放中提早一點換段（偵測有延遲）；暫停時停在切點前最後一格要能看，不換段
    if (t >= end - (player.paused ? 0 : 0.03) || player.ended) {
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

  /** 讓播放器停在 key 那段原始檔的第幾秒（時間軸播放模式）。 */
  panel.seekTo = (key, seconds) => {
    const idx = tl.seq.findIndex((item) => item.key === key);
    if (idx >= 0) tlGoto(idx, seconds, false);
  };

  /** 播放器目前停在 key 那個檔案的第幾秒（原始檔時間）；播放器不在那個檔案就回傳 null。 */
  panel.playerTimeIn = (key) => {
    const player = el("Player");
    const clip = store[key];
    if (!clip || player.classList.contains("hidden") || player.dataset.path !== clip.path) return null;
    return player.currentTime;
  };

  // --- 逐格／逐秒：播放器的拖拉條一格跨好幾秒，抓不準切點 ---
  function currentFps() {
    const player = el("Player");
    const clip = Object.values(store).find((c) => c.path === player.dataset.path);
    return clip?.info.fps || 29.97;
  }

  function step(kind) {
    const player = el("Player");
    if (player.classList.contains("hidden") || !(player.readyState >= 1)) return;
    player.pause();
    const delta = kind.endsWith("s") ? 1 : 1 / currentFps();
    const sign = kind.startsWith("-") ? -1 : 1;
    let t = player.currentTime + sign * delta;
    // 時間軸播放時往回不跨出這段的開頭（往後跨過結尾會自動接下一段）
    const item = tl.idx >= 0 ? tl.seq[tl.idx] : null;
    if (item && store[item.key]) t = Math.max(clipRange(item).start, t);
    player.currentTime = Math.min(Math.max(0, t), player.duration || t);
  }

  function showPrecise() {
    const player = el("Player");
    const fps = currentFps();
    el("Precise").textContent = `${fmtPrecise(player.currentTime)}（第 ${Math.round(player.currentTime * fps)} 格）`;
  }

  el("StepBar").querySelectorAll("[data-step]").forEach((b) =>
    b.addEventListener("click", () => step(b.dataset.step)),
  );
  {
    const player = el("Player");
    ["timeupdate", "seeked", "loadedmetadata"].forEach((ev) => player.addEventListener(ev, showPrecise));
    // 原生的 ← → 一次跳好幾秒，改成逐格；Shift 逐秒
    player.addEventListener("keydown", (e) => {
      if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
      e.preventDefault();
      step(`${e.key === "ArrowLeft" ? "-" : "+"}1${e.shiftKey ? "s" : "f"}`);
    }, true);
    new MutationObserver(() => el("StepBar").classList.toggle("hidden", player.classList.contains("hidden"))).observe(
      player,
      { attributes: true, attributeFilter: ["class"] },
    );
  }

  // --- 切點微調器 ---
  //
  // 一般的時間軸播放到切點就換下一段，只能檢查不能調。這裡固定看「某段結尾 → 下一段開頭」
  // 這個接點：左邊是切點前最後一格、右邊是接上來的第一格，±1 格／秒直接移動切點。
  const trim = { key: null };

  function trimItem() {
    const idx = tl.seq.findIndex((it) => it.key === trim.key);
    return idx < 0 ? null : { idx, item: tl.seq[idx], next: tl.seq[idx + 1] || null };
  }

  /** 讓 video 停在 t 秒那一格（seek 完成才算數）。 */
  function showFrame(video, path, t) {
    return new Promise((resolve) => {
      const go = () => {
        video.addEventListener("seeked", () => resolve(), { once: true });
        video.currentTime = Math.max(0, t);
      };
      if (video.dataset.path !== path) {
        video.dataset.path = path;
        video.src = convertFileSrc(path);
        video.addEventListener("loadedmetadata", go, { once: true });
      } else go();
    });
  }

  async function renderTrim() {
    const t = trimItem();
    if (!t || !store[t.item.key]) return panel.closeTrim();
    const clip = store[t.item.key];
    const fps = clip.info.fps || 29.97;
    const { end } = clipRange(t.item);
    el("TrimTitle").textContent = `${t.item.label} → ${t.next ? t.next.label : "（片尾）"}`;
    el("TrimAt").textContent = fmtPrecise(end);
    el("TrimNote").textContent = `（最後一格約 ${fmtPrecise(end - 1 / fps)}，${t.next?.join === "flash" ? "接閃白" : "直接接"}）`;
    el("TrimEditor").querySelector('[data-trim="reset"]').classList.toggle("hidden", !hooks.isManual?.(t.item));
    const jobs = [showFrame(el("TrimOut"), clip.path, end - 0.5 / fps)];
    const nextClip = t.next && store[t.next.key];
    el("TrimIn").classList.toggle("hidden", !nextClip);
    if (nextClip) jobs.push(showFrame(el("TrimIn"), nextClip.path, clipRange(t.next).start + 0.001));
    await Promise.all(jobs);
  }

  panel.openTrim = (key) => {
    const item = tl.seq.find((it) => it.key === key);
    if (!item || !hooks.setEnd || item.kind === "promo") return;
    trim.key = key;
    el("TrimEditor").classList.remove("hidden");
    el("TrimEditor").focus({ preventScroll: true });
    renderTrim();
  };

  panel.closeTrim = () => {
    trim.key = null;
    el("TrimEditor").classList.add("hidden");
  };

  async function moveTrim(kind) {
    const t = trimItem();
    if (!t) return;
    const clip = store[t.item.key];
    const fps = clip.info.fps || 29.97;
    const { start, end } = clipRange(t.item);
    const delta = (kind.endsWith("s") ? 1 : 1 / fps) * (kind.startsWith("-") ? -1 : 1);
    const next = Math.min(clip.info.duration, Math.max(start + 1 / fps, end + delta));
    if (next === end) return;
    await hooks.setEnd(t.item, next);
    await renderTrim();
  }

  el("TrimEditor").addEventListener("click", async (ev) => {
    const act = ev.target.closest("[data-trim]")?.dataset.trim;
    if (!act) return;
    if (act === "close") return panel.closeTrim();
    if (act === "reset") {
      const t = trimItem();
      if (t) await hooks.resetEnd?.(t.item);
      return renderTrim();
    }
    if (act === "play") {
      const t = trimItem();
      if (!t) return;
      const { start, end } = clipRange(t.item);
      const playFrom = Math.max(start, end - 3);
      await tlGoto(t.idx, playFrom, true);
      // 播過接點 3 秒後自動停
      const stopAt = Date.now() + (end - playFrom + 3) * 1000;
      const timer = setInterval(() => {
        if (Date.now() >= stopAt || el("Player").paused) {
          el("Player").pause();
          clearInterval(timer);
        }
      }, 100);
      return;
    }
    moveTrim(act);
  });
  // 焦點在微調器上時 ← → 逐格、Shift 逐秒
  el("TrimEditor").addEventListener("keydown", (e) => {
    if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
    e.preventDefault();
    moveTrim(`${e.key === "ArrowLeft" ? "-" : "+"}1${e.shiftKey ? "s" : "f"}`);
  });

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
    // ffmpeg 只回報編碼到第幾秒；最後一格編完後還要寫結尾、把整個檔案重寫成可以
    // 邊下載邊播的格式（faststart），這段沒有進度可看。所以編碼最多顯示到 99%，
    // 之後改成「收尾中」，真的寫完才算 100%。
    const unlisten = await listen("render-progress", ({ payload: p }) => {
      const frac = p.total ? Math.min(1, p.seconds / p.total) : 0;
      if (frac >= 0.999) {
        progress.value = 0.99;
        status.textContent = `收尾中：正在整理檔案，請稍候（長片可能要幾十秒）…`;
        return;
      }
      progress.value = Math.min(frac, 0.99);
      const elapsed = (Date.now() - startedAt) / 1000;
      const remain = frac > 0.02 ? (elapsed / frac) * (1 - frac) : null;
      status.textContent =
        `輸出中（${encoderName}）${Math.min(99, Math.floor(frac * 100))}%` +
        (remain != null ? ` · 編碼剩約 ${fmtTime(remain)}（之後還要收尾）` : "");
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
      // 不等提示框關掉：按鈕要先在 finally 解鎖
      showNotice({
        title: "轉檔完成",
        message: `${prefix === "full" ? "全集" : "單集"} ${baseName(lastOutput)} 轉檔完成`,
        okText: "觀看成品",
        cancelText: "關閉",
      }).then((watch) => watch && el("PreviewBtn").click());
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

/** @type {{id: string, type: string, path?: string, url?: string, startText?: string, endText?: string, gainText?: string, status?: string}[]} */
let blocks = [];
let blockSeq = 0;
const newId = () => `b${Date.now().toString(36)}${blockSeq++}`;

function saveBlocks() {
  try {
    const plain = blocks.map(({ id, type, path, url, startText, endText, gainText }) => ({ id, type, path, url, startText, endText, gainText }));
    localStorage.setItem(BLOCKS_KEY, JSON.stringify(plain));
  } catch (_) {
    /* 存不了不影響使用 */
  }
}

function promoPath(type) {
  return type === "promo20" ? settings?.promo_20_path : type === "promo45" ? settings?.promo_45_path : "";
}

/**
 * 影片積木的音量增益（dB）。沒填過就跟設定頁的預設（剪映 +6）；YT、promo 已經加過，不再加。
 */
function blockGain(b) {
  if (b.type !== "video") return 0;
  const v = parseFloat(b.gainText);
  return Number.isFinite(v) ? Math.min(20, Math.max(-20, v)) : (settings?.audio_gain_db ?? 6);
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
    items.push({ key: b.id, label, kind: t.kind, join: flashPending ? "flash" : "cut", gain_db: blockGain(b) });
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
  // 和全集切點同一套：0456、04:56、00:04:55.49 都可以；空白＝從頭／到結尾
  const parse = (text) => {
    if (!String(text ?? "").trim()) return { ok: true, value: null };
    const value = parsePreciseTime(text);
    return value == null ? { ok: false, value: null, error: `時間格式不對：${text}` } : { ok: true, value };
  };
  const [s, e] = [parse(b.startText), parse(b.endText)];
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

// 積木排序用滑鼠事件自己做：Tauri 的原生拖放要開著才能從檔案總管拖影片進來，
// 開著時 WebView 收不到 HTML5 的 dragover/drop，原本的 draggable 排序就會失效。
let reorder = null;

function blockIndexAt(x, y) {
  const card = document.elementFromPoint(x, y)?.closest?.("#blockList .block");
  return card ? blocks.findIndex((b) => b.id === card.dataset.block) : -1;
}

document.addEventListener("pointermove", (ev) => {
  if (!reorder) return;
  if (!reorder.moving && Math.hypot(ev.clientX - reorder.x, ev.clientY - reorder.y) < 6) return;
  reorder.moving = true;
  reorder.card.classList.add("dragging");
  document.querySelectorAll("#blockList .drop-target").forEach((c) => c.classList.remove("drop-target"));
  const to = blockIndexAt(ev.clientX, ev.clientY);
  if (to >= 0 && to !== reorder.from) $("blockList").children[to]?.classList.add("drop-target");
});

document.addEventListener("pointerup", (ev) => {
  if (!reorder) return;
  const { from, moving, card } = reorder;
  reorder = null;
  card.classList.remove("dragging");
  document.querySelectorAll("#blockList .drop-target").forEach((c) => c.classList.remove("drop-target"));
  if (moving) {
    const to = blockIndexAt(ev.clientX, ev.clientY);
    if (to >= 0) moveBlock(from, to);
  }
});

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
      const t = BLOCK_TYPES[b.type];
      const head = `<span class="block-grip" title="按住拖拉調整順序">⋮⋮</span><strong class="block-title">${t.label}</strong>`;
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
        const gain =
          b.type === "video"
            ? `<label class="field" title="剪映的 +6 就是 +6dB，會自動加限幅器不爆音；0＝不調整。只加在這段，YT、promo 不加">音量 +
                 <input class="gain" data-field="gainText" type="number" min="-20" max="20" step="0.5" value="${blockGain(b)}"> dB</label>`
            : "";
        const source =
          b.type === "video"
            ? `<button class="ghost small" data-act="pick">${b.path ? "換檔案…" : "選擇檔案…"}</button>`
            : b.type === "yt"
              ? `<input class="grow yt-url" data-field="url" placeholder="貼上 YouTube 連結" value="${escapeHtml(b.url || "")}">
                 <button class="ghost small" data-act="download" ${b.downloading ? "disabled" : ""}>下載</button>`
              : "";
        card.innerHTML = `<div class="block-row">${head}${source}${tools}</div>
          <div class="block-row">${trim}${gain}<span class="block-status vinfo muted">${escapeHtml(b.status || (b.type === "video" ? "尚未選擇" : ""))}</span></div>`;
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
          if (input.classList.contains("gain")) single.refresh();
          if (input.classList.contains("time")) {
            clearTimeout(b.trimTimer);
            b.trimTimer = setTimeout(() => applyTrim(b), 500);
          }
        }),
      );
      card.addEventListener("pointerdown", (ev) => {
        // 在輸入框、按鈕上按下是操作它們，不是搬積木
        if (ev.button !== 0 || ev.target.closest("input, button, a, select, textarea")) return;
        reorder = { from: i, card, x: ev.clientX, y: ev.clientY, moving: false };
      });
      return card;
    }),
  );
  single.refresh();
}

const single = createPanel("single", blockSequence, () => $("partSelect").value, {
  setEnd: async (item, end) => {
    const b = blocks.find((x) => x.id === item.key);
    if (!b) return;
    b.endText = fmtPrecise(end);
    const input = document.querySelector(`[data-block="${b.id}"] [data-field="endText"]`);
    if (input) input.value = b.endText;
    saveBlocks();
    await applyTrim(b);
  },
});

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

function clearBlocks() {
  blocks.forEach((b) => delete store[b.id]);
  blocks = [];
  renderBlocks();
  saveBlocks();
  $("singleStatus").textContent = "";
}

$("clearBlocksBtn").addEventListener("click", async () => {
  if (!blocks.length) return;
  const ok = await dialog.ask("確定要移除單集的所有積木嗎？", { title: "全部移除", kind: "warning", okLabel: "移除", cancelLabel: "取消" });
  if (ok) clearBlocks();
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
    ...parts.map((p) => new Option(`第${SEGMENT_NAMES[p.number - 1]}段：${p.title_line}`, singleDefaultName(p.number))),
  );
  if (parts.some((p) => singleDefaultName(p.number) === current)) select.value = current;
}

/** 單集檔名＝文稿日期的月日－段號，例如 1021-3（和全集的 1021全集 同一套日期）。 */
function singleDefaultName(n) {
  const d = $("date").value.trim();
  return `${/^\d{8}$/.test(d) ? d.slice(4) : d}-${n}`;
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
  () => fullDefaultName(),
  {
    // 微調器移動切點：選「切在閃白」的段寫回時間欄；只砍片尾的段記成手動切點（不再自動砍 promo 長度）
    setEnd: async (item, end) => {
      const n = item.segment;
      if (!n) return;
      if (usesCutAt(n)) {
        const text = fmtPrecise(end);
        segTrim[n].text = text;
        segTrim[n].auto = { text, end };
        if (segRows[n].timeInput) segRows[n].timeInput.value = text;
      } else segTrim[n].manualEnd = end;
      segTrim[n].on = true;
      const box = segRows[n].el.querySelector(".seg-trim-on");
      if (box) box.checked = true;
      await applySegTrim(n);
    },
    resetEnd: async (item) => {
      const n = item.segment;
      if (!n) return;
      if (usesCutAt(n)) {
        const f = segFlash[n];
        if (f?.list[f.pick]) return useFlashCandidate(n, f.pick);
      } else segTrim[n].manualEnd = null;
      await applySegTrim(n);
    },
    isManual: (item) => {
      const n = item.segment;
      if (!n) return false;
      if (usesCutAt(n)) {
        const f = segFlash[n];
        const c = f?.list[f.pick];
        return !!c && Math.abs((store[item.key]?.end ?? 0) - (c.cut - 0.005)) > 0.001;
      }
      return segTrim[n].manualEnd != null;
    },
  },
);

/** 全集預設檔名：當天日期（月日）＋全集，例如 1018全集。 */
function fullDefaultName() {
  const d = $("date").value.trim();
  return `${/^\d{8}$/.test(d) ? d.slice(4) : d}全集`;
}

// 砍掉片尾：單集輸出的檔案尾巴接著 45 秒 promo（第三、六段前面還多接了 YT），
// 全集要把它砍掉再接。第一、二、四、五段砍掉 45 秒 promo 檔的實際長度（＋設定的微調）；第三、六段
// 由使用者指定從哪個時間點以後都不要。預設勾選：放進全集的幾乎都是單集輸出的成品。
// 每段都可以在「切在閃白／時間點」和「只砍片尾」之間切換：第三、六段沒接 YT（沒有閃白）時改選只砍片尾，
// 其他段有接 YT 時改選切在閃白。CUT_AT_SEGMENTS 只是預設值。
const CUT_AT_SEGMENTS = new Set([3, 6]);
const defaultSegMode = (n) => (CUT_AT_SEGMENTS.has(n) ? "cut" : "tail");
const segTrim = Object.fromEntries([1, 2, 3, 4, 5, 6].map((n) => [n, { on: true, text: "", ok: true, error: "", mode: defaultSegMode(n) }]));
const segRows = {};

/** 第 n 段是不是用「從某時間點以後不要」（閃白或手動輸入），而不是只砍片尾。 */
function usesCutAt(n) {
  return segTrim[n].mode === "cut";
}

/** 片尾要砍的秒數＝45 秒 promo 檔的實際長度＋微調；還沒指定 promo 時回傳 null。 */
function promoTailSecs() {
  const promo = store.promo45;
  if (!promo) return null;
  return Math.max(0, promo.info.duration + (settings?.promo_tail_adjust_secs ?? 0));
}

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
    if (t.on && usesCutAt(n)) {
      if (!t.text.trim()) {
        t.ok = false;
        t.error = "請填入從哪個時間點以後不要";
      } else {
        // 自動偵測的切點用精確值；畫面上的文字只到小數兩位
        end = t.auto && t.auto.text === t.text ? t.auto.end : parsePreciseTime(t.text);
        if (end == null) {
          t.ok = false;
          t.error = "時間格式不對，例如 0456（4 分 56 秒）、015620（1 小時 56 分 20 秒）或 04:56.43";
        }
        if (t.ok && !(end > 0 && end < duration)) {
          t.ok = false;
          t.error = `時間點要在 00:00:00 和檔案長度 ${fmtTime(duration)} 之間`;
        }
      }
    } else if (t.on && t.manualEnd != null) {
      // 用切點微調器調過：照手動的切點
      end = Math.min(t.manualEnd, duration);
    } else if (t.on) {
      const tail = promoTailSecs();
      if (tail == null) {
        t.ok = false;
        t.error = "請先在設定指定 45 秒 promo 檔案，才知道片尾要砍多長";
      } else {
        end = duration - tail;
        if (!(end > 0)) {
          t.ok = false;
          t.error = `檔案只有 ${fmtPrecise(duration)}，比片尾要砍的 ${tail.toFixed(2)} 秒還短`;
        }
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
        : usesCutAt(n)
          ? `使用到 ${fmtPrecise(clip.end)}，後面 ${fmtPrecise(clip.info.duration - clip.end)} 不要`
          : `${t.manualEnd != null ? "手動切點（微調過），" : ""}砍掉片尾 ${(clip.info.duration - clip.end).toFixed(2)} 秒，使用到 ${fmtPrecise(clip.end)}`;
    row.trimInfo.classList.toggle("warn", !t.ok);
  }
  full.refresh();
}

async function applyAllSegTrims() {
  for (const n of [1, 2, 3, 4, 5, 6]) await applySegTrim(n);
}

// 選「切在閃白」的段（預設第三、六段）自動抓閃白：完整單片是「帶子 ⚡ YT ｜ promo」，閃白之後都不要。
// 帶子本身三到五分鐘、不會有閃白；YT 長短每次不同（可能十幾分鐘），所以從開頭
// 2 分半之後往後找，第一個閃白就是帶子接 YT 的那個，找到就停。
const FLASH_SKIP_SECS = 150; // 帶子最短約 3 分鐘，提早 30 秒開始找，剛好 3 分鐘的也不會漏
const segFlash = {};

function renderFlashUi(n) {
  const box = segRows[n]?.flashBox;
  if (!box) return;
  const f = segFlash[n];
  const clip = store[`s${n}`];
  if (!clip || !segTrim[n].on || !usesCutAt(n) || !f) return (box.innerHTML = "");
  const btn = (act, label, title = "") => `<button class="ghost small" data-flash="${act}" title="${title}">${label}</button>`;
  // 往後找不到時，「上一個」退回剛剛那個
  const prev = f.pick > 0 || (f.state === "none" && f.list.length) ? btn("prev", "◀ 上一個") : "";
  if (f.state === "running") {
    box.innerHTML = `<span class="muted">偵測閃白中…（從 ${fmtTime(f.from)} 往後找）</span>`;
    return;
  }
  if (f.state === "error") {
    box.innerHTML = `<span class="warn">⚠ ${escapeHtml(f.error)}</span> ${btn("retry", "重新偵測")}`;
    return;
  }
  if (f.state === "none") {
    const fromStart = f.from > 0 ? btn("start", "從頭找", "帶子比 2 分半短時用") : "";
    box.innerHTML = `<span class="warn">⚠ ${fmtTime(f.from)} 之後找不到${f.list.length ? "下一個" : ""}閃白，請手動輸入時間</span> ${prev}${fromStart}`;
    return;
  }
  const c = f.list[f.pick];
  box.innerHTML = `<span class="flash-pill">偵測到閃白 ${fmtPrecise(c.peak)}${f.list.length > 1 ? `（第 ${f.pick + 1} 個）` : ""}，切在前一格乾淨畫面</span>
    ${btn("show", "微調切點", "看切點前最後一格和接上來的第一格，可以逐格移動切點")}
    ${prev}${btn("next", "下一個 ▶", "抓錯了：從這個閃白之後繼續往後找")}`;
}

function useFlashCandidate(n, i) {
  const f = segFlash[n];
  const c = f?.list[i];
  if (!c) return;
  f.pick = i;
  f.state = "found";
  // 切點落在乾淨那格和第一格變亮的畫面之間，乾淨那格是最後一格
  const end = c.cut - 0.005;
  const text = fmtPrecise(end);
  segTrim[n].text = text;
  segTrim[n].auto = { text, end };
  if (segRows[n].timeInput) segRows[n].timeInput.value = text;
  renderFlashUi(n);
  return applySegTrim(n);
}

/**
 * 從 from 秒往後找第一個閃白。沒給 from：重新偵測，從開頭 2 分半之後找（片子比這還
 * 還短就從頭）。next：接著目前這個往後找，找到的排在後面，可以用「上一個」退回。
 */
/** 放入影片後自動偵測的那一輪：選「切在閃白」的段（有放的）都抓到閃白時，問要不要開始轉檔。 */
let flashAnnounce = false;

async function announceFlashDone() {
  if (!flashAnnounce) return;
  const segs = [1, 2, 3, 4, 5, 6].filter((n) => store[`s${n}`] && segTrim[n].on && usesCutAt(n));
  if (!segs.length || segs.some((n) => segFlash[n]?.state !== "found")) return;
  flashAnnounce = false;
  const where = segs.map((n) => `第${SEGMENT_NAMES[n - 1]}段 ${fmtPrecise(segTrim[n].auto?.end ?? 0)}`).join("、");
  const go = await showNotice({
    title: "閃白判斷完成",
    message: `${where}，請問開始轉檔嗎？`,
    okText: "開始轉檔",
    cancelText: "稍後",
    input: { label: "輸出檔名", value: $("fullName").value.trim() || fullDefaultName() },
  });
  if (!go) return;
  $("fullName").value = go.value || fullDefaultName();
  $("fullRenderBtn").click();
}

async function detectSegFlash(n, { from, next = false } = {}) {
  const clip = store[`s${n}`];
  if (!clip || !usesCutAt(n) || !segTrim[n].on) return;
  const prev = segFlash[n];
  const list = next && prev ? prev.list.slice(0, prev.pick + 1) : [];
  if (from == null) from = clip.info.duration > FLASH_SKIP_SECS + 30 ? FLASH_SKIP_SECS : 0;
  const f = (segFlash[n] = { state: "running", list, pick: list.length - 1, from, error: "" });
  renderFlashUi(n);
  try {
    const hit = await invoke("detect_flash", { path: clip.path, from });
    if (store[`s${n}`] !== clip || segFlash[n] !== f) return; // 偵測途中換了檔案
    if (hit) {
      f.list.push(hit);
      await useFlashCandidate(n, f.list.length - 1);
      if (!next) announceFlashDone();
      return;
    }
    f.state = "none";
  } catch (e) {
    f.state = "error";
    f.error = String(e);
  }
  renderFlashUi(n);
}

/** 第 n 段放入影片（選擇檔案和拖曳共用），接著套用砍片尾；選「切在閃白」的段自動抓閃白。 */
async function loadSegment(n, path) {
  const row = segRows[n];
  row.info.textContent = "讀取中…";
  segTrim[n].text = "";
  segTrim[n].auto = null;
  segTrim[n].manualEnd = null;
  delete segFlash[n];
  if (row.timeInput) row.timeInput.value = "";
  try {
    const c = await setClip(`s${n}`, path);
    row.info.textContent = `${baseName(path)} · ${describe(c.info)}`;
  } catch (e) {
    row.info.textContent = `⚠ ${e}`;
  }
  await applySegTrim(n);
  renderFlashUi(n);
  if (usesCutAt(n)) flashAnnounce = true;
  detectSegFlash(n);
}

$("fullSegments").replaceChildren(
  ...SEGMENT_NAMES.map((name, i) => {
    const n = i + 1;
    const cutAt = defaultSegMode(n) === "cut";
    const row = document.createElement("div");
    row.className = "vrow seg-row";
    row.innerHTML = `<span class="vlabel">第${name}段</span>
      <button class="ghost">選擇檔案…</button>
      <button class="ghost small">移除</button>
      <label class="seg-trim" title="切在閃白：從閃白（或指定時間點）以後都不要（YT 與片尾 promo）；只砍片尾：砍掉檔案最後的片尾 promo，秒數在設定裡調整">
        <input type="checkbox" class="seg-trim-on" checked /> 砍掉片尾
      </label>
      <span class="seg-mode" role="radiogroup" title="有接 YT（有閃白）選「切在閃白」，沒有就選「只砍片尾」">
        <button type="button" class="seg-mode-btn${cutAt ? " active" : ""}" data-mode="cut">切在閃白／時間點</button>
        <button type="button" class="seg-mode-btn${cutAt ? "" : " active"}" data-mode="tail">只砍片尾</button></span>
      <span class="seg-cut-at${cutAt ? "" : " hidden"}">從 <input class="time seg-trim-time" placeholder="例 0456" title="自動抓閃白，也可手動輸入：0456＝4 分 56 秒、015620＝1 小時 56 分 20 秒；要更準可加小數，如 0456.43" /> 以後都不要
        <button class="ghost small seg-use-pos" title="用「播放時間軸」播到要切的地方暫停，再按這顆">用目前播放位置</button></span>
      <span class="vinfo muted seg-info">尚未選擇</span>
      <span class="vinfo muted seg-trim-info"></span>
      <span class="seg-flash"></span>`;
    const [pick, clear] = row.querySelectorAll("button");
    const info = row.querySelector(".seg-info");
    const onBox = row.querySelector(".seg-trim-on");
    const cutAtBox = row.querySelector(".seg-cut-at");
    const timeInput = row.querySelector(".seg-trim-time");
    const flashBox = row.querySelector(".seg-flash");
    const modeBox = row.querySelector(".seg-mode");
    segRows[n] = { el: row, info, timeInput, flashBox, trimInfo: row.querySelector(".seg-trim-info") };

    pick.addEventListener("click", async () => {
      const path = await pickVideo();
      if (path) loadSegment(n, path);
    });
    clear.addEventListener("click", async () => {
      info.textContent = "尚未選擇";
      delete segFlash[n];
      await setClip(`s${n}`, null);
      await applySegTrim(n);
      renderFlashUi(n);
    });
    onBox.addEventListener("change", () => {
      segTrim[n].on = onBox.checked;
      cutAtBox?.classList.toggle("hidden", !usesCutAt(n) || !onBox.checked);
      modeBox?.classList.toggle("hidden", !onBox.checked);
      applySegTrim(n);
      if (usesCutAt(n) && onBox.checked && !segFlash[n] && !segTrim[n].text) detectSegFlash(n);
      renderFlashUi(n);
    });
    modeBox?.addEventListener("click", (ev) => {
      const mode = ev.target.dataset?.mode;
      if (!mode || mode === segTrim[n].mode) return;
      segTrim[n].mode = mode;
      modeBox.querySelectorAll(".seg-mode-btn").forEach((b) => b.classList.toggle("active", b.dataset.mode === mode));
      cutAtBox.classList.toggle("hidden", !usesCutAt(n) || !segTrim[n].on);
      applySegTrim(n);
      if (usesCutAt(n) && segTrim[n].on && !segFlash[n] && !segTrim[n].text) detectSegFlash(n);
      renderFlashUi(n);
    });
    timeInput?.addEventListener("input", () => {
      segTrim[n].text = timeInput.value;
      applySegTrim(n);
    });
    flashBox?.addEventListener("click", (ev) => {
      const act = ev.target.dataset?.flash;
      const f = segFlash[n];
      if (act === "retry") detectSegFlash(n);
      if (act === "start") detectSegFlash(n, { from: 0 });
      if (act === "prev" && f) useFlashCandidate(n, f.state === "none" ? f.pick : f.pick - 1);
      if (act === "next" && f) detectSegFlash(n, { from: f.list[f.pick].peak + 1, next: true });
      if (act === "show" && f) full.openTrim(`s${n}`);
    });
    row.querySelector(".seg-use-pos")?.addEventListener("click", async () => {
      const t = full.playerTimeIn(`s${n}`);
      if (t == null) {
        segRows[n].trimInfo.textContent = `⚠ 請先用「播放時間軸」或點時間軸，播到第${name}段要切的地方`;
        return;
      }
      timeInput.value = fmtPrecise(t);
      segTrim[n].text = timeInput.value;
      await applySegTrim(n);
    });
    return row;
  }),
);

/** 全集六段全部清掉（含切點、閃白偵測結果），勾選回到預設。 */
async function clearFullSegments() {
  for (const n of [1, 2, 3, 4, 5, 6]) {
    delete store[`s${n}`];
    delete segFlash[n];
    segTrim[n] = { on: true, text: "", ok: true, error: "", auto: null, manualEnd: null, mode: defaultSegMode(n) };
    const row = segRows[n];
    row.info.textContent = "尚未選擇";
    row.trimInfo.textContent = "";
    if (row.timeInput) row.timeInput.value = "";
    const box = row.el.querySelector(".seg-trim-on");
    if (box) box.checked = true;
    row.el.querySelector(".seg-cut-at").classList.toggle("hidden", !usesCutAt(n));
    row.el.querySelector(".seg-mode").classList.remove("hidden");
    row.el.querySelectorAll(".seg-mode-btn").forEach((b) => b.classList.toggle("active", b.dataset.mode === segTrim[n].mode));
    renderFlashUi(n);
  }
  flashAnnounce = false;
  $("fullStatus").textContent = "";
  $("chapterInfo").textContent = "時間點＝閃白結束、該段完全出現的那一秒";
  full.refresh();
}

// 影片頁的「清空全部」：只清目前這邊（單集或全集）
window.onClearVideo = async () => {
  const isFull = !!document.querySelector('.mode-btn[data-mode="full"].active');
  const ok = await dialog.ask(isFull ? "確定要清空全集的六段影片嗎？" : "確定要移除單集的所有積木嗎？", {
    title: "清空全部",
    kind: "warning",
    okLabel: "清空",
    cancelLabel: "取消",
  });
  if (!ok) return;
  for (const p of ["single", "full"]) {
    const player = $(`${p}Player`);
    player.pause();
    player.removeAttribute("src");
    delete player.dataset.path;
    player.load();
    player.classList.add("hidden");
  }
  if (isFull) await clearFullSegments();
  else clearBlocks();
};

// --- 單集／全集切換鈕（取代折疊區塊的摘要列） ---
{
  const accs = [...document.querySelectorAll("#page-video details.acc")];
  const accOf = (mode) => accs.find((d) => d.querySelector(mode === "full" ? "#fullSegments" : "#blockList"));
  const sync = () => {
    const open = accs.find((d) => d.open) || accs[0];
    if (!open.open) open.open = true;
    const mode = open.querySelector("#fullSegments") ? "full" : "single";
    document.querySelectorAll(".mode-btn").forEach((b) => b.classList.toggle("active", b.dataset.mode === mode));
    $("modeHint").textContent = open.querySelector(".acc-hint")?.textContent ?? "";
  };
  document.querySelectorAll(".mode-btn").forEach((b) =>
    b.addEventListener("click", () => {
      accOf(b.dataset.mode).open = true;
      sync();
    }),
  );
  accs.forEach((d) => d.addEventListener("toggle", sync));
  sync();
}

// --- 從檔案總管拖影片進來 ---
//
// 放進目前展開的那一邊：單集放一支，全集一次六支、依檔名「4 碼日期-段號」排進對應段落
// （例：1017-1、(new)1018-5、1017-3new）。

const VIDEO_EXT_RE = new RegExp(`\\.(${VIDEO_EXTENSIONS.join("|")})$`, "i");

/** 從檔名判斷第幾段；判斷不出來回傳 null。 */
function segmentFromName(path) {
  const name = baseName(path).replace(/\.[^.]+$/, "");
  const m = [...name.matchAll(/(?:^|\D)(?:\d{4})?\d{4}[-－_](\d)(?!\d)/g)].pop();
  const n = m ? Number(m[1]) : NaN;
  return n >= 1 && n <= 6 ? n : null;
}

function openAccordion() {
  const acc = [...document.querySelectorAll("#page-video details.acc")];
  return acc.find((d) => d.open && d.querySelector("#fullSegments"))
    ? "full"
    : acc.find((d) => d.open && d.querySelector("#blockList"))
      ? "single"
      : null;
}

/**
 * 拖曳放入的動畫：每支檔名從放開的位置飛進它的欄位，一支接一支由上往下，落地時欄位亮一下。
 * 純視覺效果，檔案在背後照常載入；系統設定「減少動態效果」時不播。
 */
function flyInto(items, from) {
  if (matchMedia("(prefers-reduced-motion: reduce)").matches) return;
  items.forEach(({ name, target }, i) => {
    if (!target) return;
    const chip = document.createElement("div");
    chip.className = "fly-chip";
    chip.textContent = name;
    chip.style.left = "0px";
    chip.style.top = "0px";
    document.body.appendChild(chip);
    const r = target.getBoundingClientRect();
    const w = chip.offsetWidth;
    const h = chip.offsetHeight;
    const start = `translate(${from.x - w / 2}px, ${from.y - h / 2}px)`;
    const end = `translate(${r.left + 90}px, ${r.top + r.height / 2 - h / 2}px)`;
    const anim = chip.animate(
      [
        { transform: `${start} scale(.8)`, opacity: 0 },
        { transform: `${start} scale(1.05)`, opacity: 1, offset: 0.18 },
        { transform: `${end} scale(.55)`, opacity: 0.15 },
      ],
      { duration: 560, delay: i * 90, easing: "cubic-bezier(.55, 0, .2, 1)", fill: "both" },
    );
    anim.onfinish = () => {
      chip.remove();
      target.classList.remove("landed");
      void target.offsetWidth; // 重播落地閃一下
      target.classList.add("landed");
    };
  });
}

async function dropIntoSingle(paths, from) {
  const status = $("singleStatus");
  let b = blocks.find((x) => x.type === "video" && !store[x.id]);
  if (!b) b = addBlock("video", blocks.findIndex((x) => x.type !== "video") >= 0 ? 0 : blocks.length);
  const card = () => document.querySelector(`[data-block="${b.id}"]`);
  flyInto([{ name: baseName(paths[0]), target: card() }], from);
  await loadFileBlock(b, paths[0]);
  status.textContent = paths.length > 1 ? `⚠ 單集一次只放一支，已放入 ${baseName(paths[0])}，其他 ${paths.length - 1} 支沒有放` : "";
}

async function dropIntoFull(paths, from) {
  const status = $("fullStatus");
  const bySeg = new Map();
  const unknown = [];
  const clash = new Set();
  for (const p of paths) {
    const n = segmentFromName(p);
    if (n == null) unknown.push(baseName(p));
    else if (bySeg.has(n)) clash.add(n);
    else bySeg.set(n, p);
  }
  clash.forEach((n) => bySeg.delete(n));
  const placed = [...bySeg.keys()].sort((a, b) => a - b);
  const msgs = [];
  if (placed.length) msgs.push(`已放入：${placed.map((n) => `第${SEGMENT_NAMES[n - 1]}段 ${baseName(bySeg.get(n))}`).join("、")}`);
  if (unknown.length) msgs.push(`⚠ 看不出第幾段（檔名要有「日期-段號」，如 1017-3）：${unknown.join("、")}`);
  if (clash.size) msgs.push(`⚠ 好幾支都是同一段，沒有放入：${[...clash].map((n) => `第${SEGMENT_NAMES[n - 1]}段`).join("、")}`);
  status.textContent = msgs.join("　");
  flyInto(placed.map((n) => ({ name: baseName(bySeg.get(n)), target: segRows[n].el })), from);
  await Promise.all(placed.map((n) => loadSegment(n, bySeg.get(n))));
}

{
  const page = $("page-video");
  window.__TAURI__.webview.getCurrentWebview().onDragDropEvent(async ({ payload: p }) => {
    if (page.classList.contains("hidden")) return;
    const target = openAccordion();
    if (p.type === "enter" || p.type === "over") {
      page.classList.add("drop-hover");
      return;
    }
    page.classList.remove("drop-hover");
    if (p.type !== "drop") return;
    const paths = (p.paths || []).filter((x) => VIDEO_EXT_RE.test(x));
    if (!paths.length) return;
    // Tauri 給的是實體像素，換成 CSS 像素
    const dpr = window.devicePixelRatio || 1;
    const from = p.position ? { x: p.position.x / dpr, y: p.position.y / dpr } : { x: innerWidth / 2, y: innerHeight / 2 };
    if (target === "single") await dropIntoSingle(paths, from);
    else if (target === "full") await dropIntoFull(paths, from);
    else {
      // 兩邊都收合時：一支當單集、多支當全集
      const want = paths.length > 1 ? "#fullSegments" : "#blockList";
      const acc = [...document.querySelectorAll("#page-video details.acc")].find((d) => d.querySelector(want));
      if (acc) acc.open = true;
      await (paths.length > 1 ? dropIntoFull(paths, from) : dropIntoSingle(paths, from));
    }
  });
}

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
  // 沒自己改過音量的影片積木跟著設定頁的預設走
  renderBlocks();
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

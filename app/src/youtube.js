// 自動切段頁「上傳 YouTube（私人）」：標題、說明、後臺關鍵字從文稿帶入，上傳在 Rust 端（youtube.rs）。
// 和 autocut.js／video.js 共用全域的 invoke、$、settings、listen、dialog、dateInput、convertFileSrc、
// acStatus、acCurrentTitles、acPill、acLabelCells、persistSettings、showNotice。

let ytMeta = []; // [{ number, title, description, keywords, warnings }]
let ytUploads = {}; // 段號 → { state, progress, video_id, error, warning, path }
let ytDone = {}; // 檔案路徑 → 影片 ID（上傳過的）
let ytRowEls = {}; // 段號 → 這列的元素
let ytMetaTimer = null;
let ytLoggedIn = false;
let ytThumbAuto = {}; // 段號 → 共用資料夾當天的 N.jpg（有才列）
let ytThumbDir = ""; // 共用資料夾當天的資料夾（選縮圖時從這裡開）
let ytPlaylists = []; // 頻道的播放清單 [{ id, title }]
let ytRemote = {}; // 影片 ID → { privacy, publish_at }（YouTube 上的實際狀態）；privacy 是 null＝找不到
let ytRemoteAt = {}; // 影片 ID → 上次查的時間
let ytRemoteBusy = false;

function ytKey(kind) {
  return `${kind}:${dateInput.value.trim() || "today"}`;
}

function ytLoad(kind) {
  try {
    return JSON.parse(localStorage.getItem(ytKey(kind)) || "{}");
  } catch {
    return {};
  }
}

function ytSave(kind, value) {
  try {
    localStorage.setItem(ytKey(kind), JSON.stringify(value));
  } catch {}
}

function ytMessage(text) {
  $("ytMessage").textContent = text;
  $("ytMessage").title = text;
}

// ───────── 帳號 ─────────

async function ytShowAccount() {
  const a = await invoke("yt_account");
  ytLoggedIn = a.logged_in;
  $("ytAccount").textContent = a.logged_in ? a.channel : "還沒登入";
  $("ytAccount").closest(".ac-account").classList.toggle("on", a.logged_in);
  $("ytLoginBtn").textContent = a.logged_in ? "換帳號" : "登入 YouTube";
  $("ytLogoutBtn").classList.toggle("hidden", !a.logged_in);
  $("ytPlaylistDefault").classList.toggle("hidden", !a.logged_in);
  if (a.logged_in) ytLoadPlaylists();
}

$("ytLoginBtn").addEventListener("click", async () => {
  // 不停用按鈕：Google 擋下或關掉分頁時不會回來，再按一次就取消上一次、重開登入頁
  ytMessage("已開瀏覽器，請在 Google 頁面登入並允許…（沒反應就再按一次）");
  try {
    const channel = await invoke("yt_login", { settings });
    ytMessage(`登入完成：${channel}`);
  } catch (e) {
    if (String(e) === "CANCELED") return; // 被新的一次取代
    ytMessage(String(e));
  }
  ytShowAccount();
});

$("ytLogoutBtn").addEventListener("click", async () => {
  await invoke("yt_logout").catch((e) => ytMessage(String(e)));
  ytMessage("");
  ytShowAccount();
});

// ───────── 每段要用的檔案、縮圖、播放清單 ─────────

/** 要上傳的檔：自己選的 ＞ 串接成品（上傳\MMDD-N.mp4）＞ 正片 */
function ytFileFor(n) {
  const picked = ytLoad("ytFiles")[n];
  if (picked) return picked;
  const s = acStatus?.segments?.find((x) => x.n === n);
  if (s?.chain?.output && s.chain.state === "done") return s.chain.output;
  if (!s?.chain && s?.output && s.state === "done") return s.output;
  return "";
}

/** 縮圖：自己選的 ＞ 共用資料夾當天的 N.jpg */
function ytThumbFor(n) {
  return ytLoad("ytThumbs")[n] || ytThumbAuto[n] || "";
}

function ytIds(v) {
  if (Array.isArray(v)) return v.filter(Boolean);
  return typeof v === "string" ? v.split(",").filter(Boolean) : [];
}

/** 必選播放清單的名稱（設定頁，一行一個；{日期} 換成文稿日期） */
function ytRequiredNames() {
  const date = dateInput.value.trim();
  return (settings?.yt_required_playlists || "")
    .split("\n")
    .map((s) => s.trim())
    .filter(Boolean)
    .map((s) => s.replaceAll("{日期}", date));
}

/** 用名稱找頻道上的播放清單（忽略空白） */
function ytFindPlaylist(name) {
  const norm = (s) => s.replace(/\s/g, "");
  return ytPlaylists.find((p) => norm(p.title) === norm(name));
}

/** 必選清單：已經建好的 ID、還沒建的名稱 */
function ytRequired() {
  const ids = [];
  const missing = [];
  for (const name of ytRequiredNames()) {
    const p = ytFindPlaylist(name);
    if (p) ids.push(p.id);
    else missing.push(name);
  }
  return { ids, missing };
}

/** 預設播放清單＝必選的＋上面那個選單另外加的（存成逗號分隔的 ID） */
function ytDefaultPlaylists() {
  return [...new Set([...ytRequired().ids, ...ytIds(settings?.yt_playlist_id || "")])];
}

/** 這段的播放清單：必選的一定有，再加這列自己選過的（沒選過就用預設） */
function ytPlaylistsFor(n) {
  const saved = ytLoad("ytPlaylists")[n];
  return [...new Set([...ytRequired().ids, ...(saved === undefined ? ytDefaultPlaylists() : ytIds(saved))])];
}

/** 共用資料夾（`\\數媒yt上傳\yt上傳\1026關鍵`）裡的 1.jpg～6.jpg */
async function ytFindThumbs() {
  ytThumbAuto = {};
  ytThumbDir = "";
  const date = dateInput.value.trim();
  if (!/^\d{8}$/.test(date)) return;
  const txt = await invoke("sync_path", { settings, date }).catch(() => null);
  if (!txt) return;
  const dir = txt.replace(/[\\/][^\\/]*$/, "");
  ytThumbDir = dir;
  for (const m of ytMeta) {
    for (const ext of ["jpg", "png"]) {
      const p = `${dir}\\${m.number}.${ext}`;
      if (await invoke("path_exists", { path: p }).catch(() => false)) {
        ytThumbAuto[m.number] = p;
        break;
      }
    }
  }
}

async function ytRefreshMeta() {
  try {
    ytMeta = await invoke("yt_meta", { text: await acCurrentTitles() });
  } catch {
    ytMeta = [];
  }
  ytBuild();
  ytRefreshPickers(); // 換日期時必選的「關鍵時刻{日期}全集」也跟著換
  await ytFindThumbs();
  ytUpdate();
}

// ───────── 播放清單複選（可搜尋） ─────────

/** 按鈕顯示選了哪些；點開是搜尋框＋勾選清單（浮在畫面上，不會被表格裁掉） */
function ytPlaylistPicker(getIds, setIds, emptyText) {
  const wrap = document.createElement("div");
  wrap.className = "yt-pl-picker";
  const btn = document.createElement("button");
  btn.type = "button";
  btn.className = "ghost small yt-pl-btn";
  const pop = document.createElement("div");
  pop.className = "yt-pl-pop hidden";
  pop.innerHTML = `<input class="yt-pl-search" placeholder="搜尋播放清單" spellcheck="false" />
    <div class="yt-pl-list"></div>
    <div class="yt-pl-foot"><span class="yt-pl-n muted"></span><button type="button" class="ghost small yt-pl-clear">只留必選</button><button type="button" class="small yt-pl-done">完成</button></div>`;
  wrap.append(btn, pop);
  const search = pop.querySelector(".yt-pl-search");
  const list = pop.querySelector(".yt-pl-list");

  const summary = () => {
    const ids = getIds();
    const names = ids.map((id) => ytPlaylists.find((p) => p.id === id)?.title || "（找不到的清單）");
    btn.textContent = !ids.length ? emptyText : ids.length === 1 ? names[0] : `${names[0]} 等 ${ids.length} 個`;
    btn.title = names.length ? names.join("\n") : emptyText;
    pop.querySelector(".yt-pl-n").textContent = `已選 ${ids.length} 個`;
  };
  const render = () => {
    const ids = getIds();
    const q = search.value.trim().toLowerCase();
    // 已選的排前面，方便看
    const required = ytRequired().ids;
    const items = ytPlaylists.filter((p) => !q || p.title.toLowerCase().includes(q)).sort((a, b) => ids.includes(b.id) - ids.includes(a.id));
    list.replaceChildren(
      ...items.map((p) => {
        const label = document.createElement("label");
        label.className = "yt-pl-item";
        const box = document.createElement("input");
        box.type = "checkbox";
        box.checked = ids.includes(p.id);
        // 必選的勾著、不能取消（改設定頁的「必選播放清單」）
        if (required.includes(p.id)) {
          box.disabled = true;
          label.classList.add("required");
          label.title = "必選（設定頁可改）";
        }
        box.onchange = () => {
          const cur = getIds().filter((x) => x !== p.id);
          if (box.checked) cur.push(p.id);
          setIds(cur);
          summary();
        };
        label.append(box, document.createTextNode(p.title));
        return label;
      }),
    );
    if (!items.length) list.textContent = ytPlaylists.length ? "找不到符合的清單" : "還沒讀到播放清單（請先登入）";
  };
  const place = () => {
    const r = btn.getBoundingClientRect();
    const w = 320;
    pop.style.left = `${Math.max(8, Math.min(r.left, window.innerWidth - w - 8))}px`;
    const below = window.innerHeight - r.bottom;
    if (below < 300 && r.top > below) {
      pop.style.top = "";
      pop.style.bottom = `${window.innerHeight - r.top + 4}px`;
    } else {
      pop.style.bottom = "";
      pop.style.top = `${r.bottom + 4}px`;
    }
  };

  btn.onclick = () => {
    const open = pop.classList.contains("hidden");
    ytClosePickers();
    if (!open) return;
    search.value = "";
    render();
    place();
    pop.classList.remove("hidden");
    search.focus();
  };
  search.oninput = render;
  search.onkeydown = (e) => e.key === "Escape" && pop.classList.add("hidden");
  pop.querySelector(".yt-pl-clear").onclick = () => {
    setIds([]);
    summary();
    render();
  };
  pop.querySelector(".yt-pl-done").onclick = () => pop.classList.add("hidden");
  wrap.refresh = summary;
  summary();
  return wrap;
}

function ytClosePickers() {
  document.querySelectorAll(".yt-pl-pop").forEach((p) => p.classList.add("hidden"));
}
document.addEventListener("mousedown", (e) => {
  if (!e.target.closest(".yt-pl-picker")) ytClosePickers();
});
$("page-autocut").addEventListener("scroll", ytClosePickers, { passive: true });

/** 登入後讀頻道的播放清單（完整，分頁讀到最後） */
async function ytLoadPlaylists() {
  try {
    ytPlaylists = await invoke("yt_playlists", { settings });
  } catch (e) {
    ytPlaylists = [];
    ytMessage(`讀不到播放清單：${e}`);
  }
  ytPlaylists.sort((a, b) => a.title.localeCompare(b.title, "zh-Hant"));
  ytRefreshPickers();
  ytUpdate();
}

function ytRefreshPickers() {
  $("ytPlaylistDefault").firstElementChild?.refresh();
  for (const el of Object.values(ytRowEls)) el.picker?.refresh();
}

// 上面那個是預設：還沒自己選過的列跟著預設
$("ytPlaylistDefault").replaceChildren(
  ytPlaylistPicker(
    () => ytDefaultPlaylists(),
    (ids) => {
      settings.yt_playlist_id = ids.join(",");
      persistSettings();
      for (const el of Object.values(ytRowEls)) el.picker?.refresh();
    },
    "預設：不加播放清單",
  ),
);

// ───────── 表格 ─────────

/** 有標題的段各一列；只在段落組成或日期改變時重建，選單開到一半不會被洗掉 */
function ytBuild() {
  const tbody = $("ytRows");
  const numbers = ytMeta.map((m) => m.number).join(",");
  if (tbody.dataset.numbers !== numbers || tbody.dataset.date !== dateInput.value.trim()) {
    tbody.dataset.numbers = numbers;
    tbody.dataset.date = dateInput.value.trim();
    ytRowEls = {};
    tbody.replaceChildren(
      ...ytMeta.map((m) => {
        const n = m.number;
        const tr = document.createElement("tr");
        tr.innerHTML = `<td>第 ${n} 段</td><td class="yt-main"><div class="yt-title"></div><div class="yt-count"></div></td>
          <td class="yt-thumb"></td><td class="yt-pl"></td><td class="yt-file"></td><td class="yt-status"></td><td class="ac-actions"></td>`;
        const el = {
          tr,
          title: tr.querySelector(".yt-title"),
          count: tr.querySelector(".yt-count"),
          thumb: tr.querySelector(".yt-thumb"),
          file: tr.querySelector(".yt-file"),
          status: tr.querySelector(".yt-status"),
          actions: tr.querySelector(".ac-actions"),
        };
        el.picker = ytPlaylistPicker(
          () => ytPlaylistsFor(n),
          (ids) => {
            const all = ytLoad("ytPlaylists");
            all[n] = ids;
            ytSave("ytPlaylists", all);
          },
          "不加清單",
        );
        tr.querySelector(".yt-pl").append(el.picker);
        // 檔案、縮圖都是點了就選（縮圖從共用資料夾開）
        el.fileBtn = Object.assign(document.createElement("button"), { type: "button", className: "yt-file-btn", onclick: () => ytPick(n) });
        el.file.append(el.fileBtn);
        ytRowEls[n] = el;
        return tr;
      }),
    );
    if (!ytMeta.length) {
      const tr = document.createElement("tr");
      tr.innerHTML = `<td colspan="7" class="muted">文稿還沒有段落標題</td>`;
      tbody.append(tr);
    }
    acLabelCells(tbody);
  }
  for (const m of ytMeta) {
    const el = ytRowEls[m.number];
    if (!el) continue;
    el.m = m;
    el.title.textContent = m.title;
    el.title.title = `${m.title}\n\n${m.description}${m.warnings.length ? `\n\n⚠ ${m.warnings.join("\n⚠ ")}` : ""}`;
    ytCount(el);
    el.picker.refresh();
  }
  ytUpdate();
}

/** 實際送出的標記：預設標記 → 文稿的後臺關鍵字（Rust 端去重複、超過 500 字的從後面截掉） */
function ytTagsFor(m) {
  return [settings?.yt_default_tags || "", (m?.keywords || []).join(",")].filter((s) => s.trim()).join(",");
}

/** 只有標記超過 500 字時才提醒（跟 Studio 一樣算：含逗號，有空白的標記加引號） */
function ytCount(el) {
  const tags = [...new Set(ytTagsFor(el.m).split(/[,，、\n]/).map((t) => t.trim()).filter(Boolean))];
  const total = tags.reduce((a, t) => a + t.length + (t.includes(" ") ? 2 : 0), 0) + Math.max(0, tags.length - 1);
  el.count.textContent = total > 500 ? `標記 ${total}/500，後面的後臺關鍵字會截掉` : "";
}

/** 縮圖框：有圖顯示小圖，沒有顯示「未取得」；都可以點來自己選 */
function ytThumbCell(el, n) {
  const thumb = ytThumbFor(n);
  if (el.thumbPath === thumb && el.thumb.firstChild) return; // 沒變不重畫（避免閃）
  el.thumbPath = thumb;
  const btn = document.createElement("button");
  btn.type = "button";
  btn.className = "yt-thumb-btn";
  btn.title = thumb ? `${thumb}\n點一下換別張` : "共用資料夾沒有這段的縮圖，點一下自己選";
  btn.onclick = () => ytPickThumb(n);
  const empty = () => btn.replaceChildren(Object.assign(document.createElement("span"), { className: "yt-thumb-empty", textContent: "未取得縮圖\n點一下選擇" }));
  if (thumb) {
    const img = document.createElement("img");
    img.alt = thumb.split(/[\\/]/).pop();
    img.src = convertFileSrc(thumb);
    img.onerror = empty;
    btn.append(img);
  } else empty();
  el.thumb.replaceChildren(btn);
}

/** 檔案、縮圖、狀態、按鈕（監看狀態、上傳進度變了就更新） */
async function ytUpdate() {
  const files = Object.fromEntries(ytMeta.map((m) => [m.number, ytFileFor(m.number)]));
  const paths = Object.values(files).filter(Boolean);
  if (paths.length) ytDone = { ...ytDone, ...(await invoke("yt_uploaded", { paths }).catch(() => ({}))) };
  for (const m of ytMeta) {
    const el = ytRowEls[m.number];
    if (!el) continue;
    const n = m.number;
    const path = files[n];
    el.fileBtn.textContent = path ? path.split(/[\\/]/).pop() : "還沒有成品，點一下選檔";
    el.fileBtn.classList.toggle("empty", !path);
    el.fileBtn.title = path ? `${path}\n點一下換別的檔` : "點一下選要上傳的影片";
    ytThumbCell(el, n);

    const up = ytUploads[n];
    // 哪支影片：文稿這段的 YT 連結（大家共用，以它為準）＞ 剛傳完的 ＞ 本機上傳紀錄
    const docId = ytLinkId(ytDocLink(n));
    let id = docId || (up?.state === "done" ? up.video_id : ytDone[path]);
    el.videoId = id;
    // 再跟 YouTube 對一次：被刪掉（或連結打錯）就當沒傳過
    const remote = id ? ytRemote[id] : undefined;
    const deleted = remote && !remote.privacy;
    if (deleted) id = null;
    el.tr.classList.toggle("yt-uploaded", !!id && up?.state !== "uploading");
    let label = "";
    let cls = "";
    if (up?.state === "uploading") [label, cls] = [`上傳中 ${Math.round(up.progress * 100)}%`, "ok"];
    else if (up?.state === "failed") [label, cls] = ["失敗", "bad"];
    else if (up?.state === "canceled") label = "已取消";
    else if (deleted) [label, cls] = [docId ? "文稿連結的影片找不到" : "YouTube 上已刪除", "bad"];
    else if (id) [label, cls] = [remote || !docId ? `已上傳（${ytPrivacyLabel(remote)}）` : "已上傳", "done"];
    else if (ytWaitingFor(m)) [label, cls] = [`等${ytWaitingFor(m).join("、")}`, "wait"];
    else label = path ? "未上傳" : "";
    el.status.replaceChildren();
    if (label) {
      const pill = document.createElement("span");
      acPill(pill, label, cls, up?.state === "uploading" ? up.progress : null);
      if (up?.error) pill.title = up.error;
      el.status.append(pill);
    }
    if (up?.warning) el.status.append(Object.assign(document.createElement("div"), { className: "yt-warn", textContent: up.warning }));

    el.actions.replaceChildren();
    const button = (text, cls, onclick) => el.actions.append(Object.assign(document.createElement("button"), { type: "button", className: cls, textContent: text, onclick }));
    if (up?.state === "uploading") {
      button("取消", "ghost small danger", () => invoke("yt_cancel", { n }));
      continue;
    }
    if (id) button("開 Studio", "ghost small", () => invoke("yt_open", { videoId: id }).catch((e) => ytMessage(String(e))));
    if (path) button(id ? "再傳一次" : "上傳", id ? "ghost small" : "small", () => ytUpload(m, path, !!id));
  }
  ytFetchStates(Object.values(ytRowEls).map((el) => el.videoId).filter(Boolean));
  ytAutoUpload();
}

// ───────── 文稿的 YT 連結（檔頭「串N」）：讀出影片 ID、上傳完回寫短網址 ─────────

/** 文稿頁第 n 段的 YT 連結欄（app.js 的 segments） */
function ytDocLink(n) {
  return segments[n - 1]?.link.value.trim() || "";
}

/** youtu.be/ID、youtube.com/watch?v=ID、/shorts/ID、/live/ID → ID；不是 YouTube 網址回傳 "" */
function ytLinkId(url) {
  const m =
    url.match(/youtu\.be\/([\w-]{11})/) ||
    url.match(/youtube\.com\/(?:watch\?(?:.*&)?v=|shorts\/|live\/|embed\/)([\w-]{11})/) ||
    url.match(/studio\.youtube\.com\/video\/([\w-]{11})/);
  return m ? m[1] : "";
}

/** 上傳完把短網址填進文稿這段的 YT 連結；原本有別的連結就先問 */
async function ytWriteLink(n, videoId) {
  const input = segments[n - 1]?.link;
  if (!input) return;
  const url = `https://youtu.be/${videoId}`;
  const old = input.value.trim();
  if (ytLinkId(old) === videoId) return;
  if (old) {
    const ok = await dialog.ask(`第 ${n} 段的 YT 連結原本是\n${old}\n\n要換成剛上傳的\n${url}\n嗎？`, {
      title: "更新文稿的 YT 連結",
      kind: "warning",
      okLabel: "換成新的",
      cancelLabel: "保留原本的",
    });
    if (!ok) return;
  }
  input.value = url;
  changed(); // 跟手動貼上一樣：存草稿、重新排版，有開同步就寫回共用資料夾
  ytMessage(`第 ${n} 段的連結已寫進文稿：${url}`);
}

// ───────── YouTube 上的實際狀態（公開／私人／不公開／排程／已刪除） ─────────

function ytPrivacyLabel(remote) {
  if (!remote) return "私人"; // 還沒查到：剛傳完一定是私人
  if (remote.publish_at && remote.privacy === "private") return "排程公開";
  return { private: "私人", unlisted: "不公開", public: "公開" }[remote.privacy] || remote.privacy;
}

/** 傳過的影片每 2 分鐘最多查一次（1 個配額單位查 50 支） */
async function ytFetchStates(ids) {
  if (!ytLoggedIn || ytRemoteBusy) return;
  const now = Date.now();
  const due = [...new Set(ids)].filter((id) => !(now - (ytRemoteAt[id] || 0) < 120000));
  if (!due.length) return;
  ytRemoteBusy = true;
  due.forEach((id) => (ytRemoteAt[id] = now));
  try {
    ytRemote = { ...ytRemote, ...(await invoke("yt_video_states", { settings, ids: due })) };
    ytRemoteBusy = false;
    ytUpdate();
  } catch {
    ytRemoteBusy = false; // 查不到（沒網路、配額）就先照本機紀錄顯示
  }
}

async function ytPick(n) {
  const current = ytFileFor(n);
  const defaultPath = current ? current.replace(/[\\/][^\\/]*$/, "") : undefined;
  const path = await dialog.open({ multiple: false, defaultPath, filters: [{ name: "影片", extensions: ["mp4", "mov", "mkv"] }] });
  if (!path) return;
  const all = ytLoad("ytFiles");
  all[n] = path;
  ytSave("ytFiles", all);
  ytUpdate();
}

async function ytPickThumb(n) {
  const current = ytThumbFor(n);
  const defaultPath = current ? current.replace(/[\\/][^\\/]*$/, "") : ytThumbDir || undefined;
  const path = await dialog.open({ multiple: false, defaultPath, filters: [{ name: "圖片", extensions: ["jpg", "jpeg", "png"] }] });
  if (!path) return;
  const all = ytLoad("ytThumbs");
  all[n] = path;
  ytSave("ytThumbs", all);
  ytUpdate();
}

// ───────── 上傳 ─────────

async function ytUpload(m, path, again) {
  if (again) {
    const ok = await dialog.ask(`第 ${m.number} 段已經上傳過，要再傳一支新的私人影片嗎？`, {
      title: "再傳一次",
      kind: "warning",
      okLabel: "再傳",
      cancelLabel: "取消",
    });
    if (!ok) return;
  }
  const footer = (settings.yt_description_footer || "").trim();
  const req = {
    n: m.number,
    path,
    title: m.title,
    description: footer ? `${m.description}\n\n${footer}` : m.description,
    tags: ytTagsFor(m),
    thumbnail: ytThumbFor(m.number) || null,
    playlist_ids: ytPlaylistsFor(m.number),
  };
  try {
    await invoke("yt_upload", { settings, req });
    ytMessage("");
  } catch (e) {
    ytMessage(String(e));
  }
}

// 自動上傳：成品做好、登入了、還沒傳過，就自動傳（每個檔這次開程式只自動試一次）
const ytAutoTried = new Set();

/** 自動上傳用的檔：只認自動切段做出來的成品（不傳自己選的檔），有串接就等串接完成 */
function ytAutoPath(n) {
  const s = acStatus?.segments?.find((x) => x.n === n);
  return s?.chain ? (s.chain.state === "done" ? s.chain.output : "") : s?.state === "done" && !s.stale ? s.output : "";
}

/** 設定頁勾的「自動上傳前要備齊」裡，這段還缺哪些 */
function ytMissing(m) {
  const miss = [];
  if (settings.yt_require_thumbnail && !ytThumbFor(m.number)) miss.push("縮圖");
  if (settings.yt_require_keywords && !m.keywords.length) miss.push("後臺關鍵字");
  if (settings.yt_require_playlist) for (const name of ytRequired().missing) miss.push(`播放清單「${name}」`);
  return miss;
}

/** 這段在等資料（成品好了、還沒傳、資料沒齊）：回傳缺的東西，不是在等就回傳 null */
function ytWaitingFor(m) {
  if (!settings?.yt_auto_upload) return null;
  const path = ytAutoPath(m.number);
  if (!path || ytLinkId(ytDocLink(m.number)) || ytDone[path] || ytUploads[m.number]) return null;
  const miss = ytMissing(m);
  return miss.length ? miss : null;
}

async function ytAutoUpload() {
  if (!settings?.yt_auto_upload || !ytLoggedIn) return;
  for (const m of ytMeta) {
    const path = ytAutoPath(m.number);
    // 文稿這段已經有 YT 連結＝已經傳過（可能是別台電腦或 Studio 手動傳的），不自動再傳
    if (!path || ytLinkId(ytDocLink(m.number)) || ytDone[path] || ytAutoTried.has(path) || ytUploads[m.number]?.state === "uploading") continue;
    if (ytMissing(m).length) continue; // 資料還沒齊，先等（狀態欄顯示「等…」）
    ytAutoTried.add(path);
    const done = await invoke("yt_uploaded", { paths: [path] }).catch(() => ({}));
    if (done[path]) {
      ytDone[path] = done[path];
      continue;
    }
    ytMessage(`第 ${m.number} 段成品做好了，自動上傳中`);
    ytUpload(m, path, false);
  }
}

// 有段在等資料時，每分鐘看一次：當天的播放清單建了沒、共用資料夾的文稿有沒有人改過（沒在編輯就載入，
// 跟點進文稿時一樣）、縮圖到了沒
setInterval(async () => {
  const waiting = ytMeta.map((m) => ytWaitingFor(m)).filter(Boolean);
  if (!waiting.length) return;
  if (ytLoggedIn && waiting.some((miss) => miss.some((x) => x.startsWith("播放清單")))) await ytLoadPlaylists();
  if (syncLoads() && sync.path && !sync.busy && !sync.conflict && !sync.dirty && !syncPaused()) {
    try {
      const mtime = await invoke("sync_stat", { path: sync.path });
      if ((mtime ?? null) !== sync.mtime) await syncPull(); // 載入後 onDocRefreshed 會重讀標題、關鍵字
    } catch {}
  }
  await ytFindThumbs();
  ytUpdate();
}, 60000);

listen("yt-upload", (e) => {
  const u = e.payload;
  ytUploads[u.n] = u;
  if (u.state === "done") {
    ytDone[u.path] = u.video_id;
    showNotice({ title: `第 ${u.n} 段已上傳 YouTube（私人）`, message: `https://youtu.be/${u.video_id}` });
    ytWriteLink(u.n, u.video_id);
  }
  if (u.state === "failed") ytMessage(`第 ${u.n} 段上傳失敗：${u.error}`);
  ytUpdate();
});

listen("autocut-status", () => ytUpdate());

// 文稿改了（打字、換日期、匯入、同步讀進來）就重讀標題；跟 autocut.js 的同一個鉤子串起來
const ytPrevDocRefreshed = window.onDocRefreshed;
window.onDocRefreshed = () => {
  ytPrevDocRefreshed?.();
  clearTimeout(ytMetaTimer);
  ytMetaTimer = setTimeout(ytRefreshMeta, 500);
};
const ytPrevSettingsChanged = window.onSettingsChanged;
window.onSettingsChanged = () => {
  ytPrevSettingsChanged?.();
  if (settings) {
    ytBuild();
    ytRefreshPickers();
  }
};

(async function ytInit() {
  while (!settings) await new Promise((r) => setTimeout(r, 100));
  ytRefreshPickers();
  ytShowAccount();
  ytRefreshMeta();
})();

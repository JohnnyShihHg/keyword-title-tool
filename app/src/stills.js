// 自動切段頁「替換畫面」：要遮的畫面（靜圖）放在靜圖資料夾的「第N段」子資料夾＝只給那段比對；
// 直接放在資料夾裡＝每段都比（舊用法）。檔案操作都在 Rust 端（autocut.rs 的 stills_*），監看中會自己發現資料夾變了。
// 貼 LINE 的遮蓋訊息：抓出資料夾、檔名前綴、編號範圍去找圖，再用主題對文稿標題猜是第幾段。
// 和 autocut.js、bars.js 共用全域的 invoke、$、settings、listen、dialog、convertFileSrc、acMessage、acBarTitles。

const AC_STILL_SEGS = [1, 2, 3, 4, 5, 6, 0]; // 0＝每段都比
let acStills = {}; // 段號 → [路徑]
let acStillEls = {}; // 段號 → { details, count, grid }
let acStillCands = []; // 貼訊息找到、還沒加入的圖

function acStillsDir() {
  return settings?.autocut_stills_dir || "";
}

function acStillSegName(n) {
  return n ? `第 ${n} 段` : "每段都比";
}

function acStillThumb(path, onRemove) {
  const item = document.createElement("div");
  item.className = "ac-still";
  item.title = path;
  const img = document.createElement("img");
  img.alt = path.split(/[\\/]/).pop();
  img.src = convertFileSrc(path);
  img.onerror = () => item.classList.add("broken");
  const name = Object.assign(document.createElement("span"), { className: "ac-still-name", textContent: img.alt });
  const del = Object.assign(document.createElement("button"), { type: "button", className: "ac-still-del", textContent: "×", title: "拿掉（搬到「_已移除」資料夾）" });
  del.onclick = onRemove;
  item.append(img, name, del);
  return item;
}

function acBuildStills() {
  const root = $("acStills");
  const open = new Set(Object.entries(acStillEls).filter(([, el]) => el.details.open).map(([n]) => Number(n)));
  acStillEls = {};
  root.replaceChildren(
    ...AC_STILL_SEGS.map((n) => {
      const details = document.createElement("details");
      details.className = "ac-bar-seg ac-still-seg";
      details.dataset.seg = n;
      details.open = open.has(n);
      details.innerHTML = `<summary><span class="ac-bar-name">${acStillSegName(n)}</span><span class="ac-bar-count"></span>${
        n ? "" : '<span class="muted ac-still-hint">直接放在靜圖資料夾裡的</span>'
      }<span class="spacer"></span><button type="button" class="ghost small ac-still-add">＋ 加圖片</button></summary><div class="ac-stills-grid"></div>`;
      const el = { details, count: details.querySelector(".ac-bar-count"), grid: details.querySelector(".ac-stills-grid") };
      details.querySelector(".ac-still-add").onclick = async (e) => {
        e.preventDefault(); // 不要觸發收合
        const picked = await dialog.open({ multiple: true, filters: [{ name: "圖片", extensions: ["jpg", "jpeg", "png"] }] });
        if (picked?.length) await acAddStills(n, [].concat(picked));
      };
      acStillEls[n] = el;
      return details;
    }),
  );
  acFillStills();
}

function acFillStills() {
  let total = 0;
  for (const n of AC_STILL_SEGS) {
    const el = acStillEls[n];
    if (!el) continue;
    const list = acStills[n] || [];
    total += list.length;
    el.count.textContent = list.length ? `${list.length} 張` : "";
    el.grid.replaceChildren(...list.map((p) => acStillThumb(p, () => acRemoveStill(p))));
    if (!list.length) el.grid.append(Object.assign(document.createElement("div"), { className: "muted ac-bar-empty", textContent: "沒有圖片（可以直接拖圖片進來）" }));
  }
  const bars = Object.values(typeof acBars === "object" ? acBars : {}).reduce((a, v) => a + (v?.length || 0), 0);
  $("acReplaceNote").textContent = `靜圖 ${total} 張、BAR ${bars} 條`;
}

async function acLoadStills() {
  const dir = acStillsDir();
  if (!dir) {
    acStills = {};
    acFillStills();
    return;
  }
  try {
    acStills = await invoke("stills_list", { dir });
  } catch (e) {
    acStills = {};
    acMessage(String(e));
  }
  acFillStills();
}

async function acAddStills(n, paths) {
  const dir = acStillsDir();
  if (!dir) return acMessage("請先在上面選靜圖資料夾");
  try {
    const added = await invoke("stills_add", { dir, n, paths });
    acMessage(`${added.length} 張加到${acStillSegName(n)}`);
    if (acStillEls[n]) acStillEls[n].details.open = true;
  } catch (e) {
    acMessage(String(e));
  }
  await acLoadStills();
}

async function acRemoveStill(path) {
  try {
    await invoke("stills_remove", { dir: acStillsDir(), path });
  } catch (e) {
    acMessage(String(e));
  }
  await acLoadStills();
}

// ───────── 貼 LINE 的遮蓋訊息 ─────────
// 22-敬平-iphone duo頻傳災情 翻攝X、微博，上述兩位疑創作者請幫遮
// 可用提供的截圖畫面替換
// 22-敬平-iphone duo頻傳災情01-03
// \\10.3.20.33\yt上傳\1027關鍵\替換畫面

/** 開頭編號：22＝前半（第 1～3 段）、23＝後半（第 4～6 段）；後面再接 -N＝那半的第 N 段（22-3＝第 3 段、23-2＝第 5 段）。
 * → { segs: 可能的段落, exact: 確定是哪段（沒寫小段＝null） }；看不懂回傳 null */
function acStillCode(line) {
  const m = line.match(/^\s*(2[23])(?:\s*[-－]\s*([1-3]))?\s*[-－]/);
  if (!m) return null;
  const base = m[1] === "22" ? 0 : 3;
  const exact = m[2] ? base + Number(m[2]) : null;
  return { segs: exact ? [exact] : [base + 1, base + 2, base + 3], exact, code: m[2] ? `${m[1]}-${m[2]}` : m[1] };
}

/** → { dir, prefix, range: [a, b] | null, topics: [主題…], code: acStillCode 的結果 } */
function acParseStillMsg(text) {
  const lines = text.split(/\r?\n/).map((l) => l.trim()).filter(Boolean);
  const dir = lines.find((l) => /^(\\\\|[A-Za-z]:[\\/])/.test(l)) || "";
  let prefix = "";
  let range = null;
  for (const l of lines) {
    if (l === dir) continue;
    // 「…01-03」「…01~03」或只有一張「…01」；前面要有字（排除開頭就是編號的「22-3」）
    const m = l.match(/^(.*\D)(\d{1,3})\s*[-~～–－]\s*(\d{1,3})$/) || l.match(/^(.*\D)(\d{2,3})$/);
    if (m && m[1].replace(/[\d\s\-－]/g, "").length >= 2) {
      prefix = m[1].trim();
      const a = Number(m[2]);
      range = [a, m[3] != null ? Number(m[3]) : a];
    }
  }
  // 主題：去掉開頭的編號和人名（世聰-）；第一行還要去掉後面的說明（「翻攝X、微博，…」）。
  // 第一行（「姆巴佩」）和檔名那行（「法國姆巴佩之前比賽得分」）寫法常常不一樣，兩個都拿來對標題
  const strip = (l) => l.replace(/^[\d\s\-－]+/, "").replace(/^[^\-－\s]{1,6}[\-－]/, "");
  const first = lines.find((l) => /^\d/.test(l) && l !== prefix && !l.startsWith(prefix || "\u0000")) || "";
  const topics = [prefix && strip(prefix).trim(), first && strip(first).split(/\s+(?=翻攝|現場|畫面)|[，,、]/)[0].trim()].filter(Boolean);
  return { dir, prefix, range, topics: [...new Set(topics)], code: acStillCode(prefix) || acStillCode(first) };
}

function acStillNorm(s) {
  return [...s.toLowerCase()].filter((c) => /[\p{L}\p{N}]/u.test(c));
}

/** 主題的二字詞在各段標題出現幾個 → 由多到少 [{ n, hits, words }] */
function acSegmentScores(topics) {
  const grams = new Set();
  for (const topic of topics) {
    const t = acStillNorm(topic);
    t.slice(1).forEach((c, i) => grams.add(t[i] + c));
  }
  return Object.entries(typeof acBarTitles === "object" ? acBarTitles : {})
    .map(([n, lines]) => {
      const all = acStillNorm(lines.join(" ")).join("");
      const words = [...grams].filter((g) => all.includes(g));
      return { n: Number(n), hits: words.length, words };
    })
    .sort((a, b) => b.hits - a.hits);
}

/** 對到的二字詞接回主題裡的連續字（「ip、ph、ho…」→「iphoneduo」） */
function acMatchedWords(topics, grams) {
  const out = new Set();
  for (const topic of topics) {
    const t = acStillNorm(topic);
    let run = "";
    for (let i = 0; i < t.length - 1; i++) {
      if (grams.includes(t[i] + t[i + 1])) run = run ? run + t[i + 1] : t[i] + t[i + 1];
      else if (run) out.add(run), (run = "");
    }
    if (run) out.add(run);
  }
  return [...out].slice(0, 4);
}

/** 明顯是某一段才回傳（至少對到 2 個二字詞、而且是第二名的兩倍以上），不然 null 讓人自己選。
 * `segs`：只在這幾段裡比（編號說是前半／後半） */
function acGuessSegment(topics, segs = null) {
  const [best, second] = acSegmentScores(topics).filter((s) => !segs || segs.includes(s.n));
  if (!best || best.hits < 2 || (second && best.hits < second.hits * 2)) return null;
  return best;
}

let acStillParseTimer = null;
let acStillParseSeq = 0;

async function acParseStills() {
  const seq = ++acStillParseSeq;
  const p = acParseStillMsg($("acStillsMsg").value);
  // 編號寫到小段（22-3）就直接用；只有 22／23 就在那半的三段裡用標題比；沒有編號才六段一起比
  const code = p.code;
  const half = code && !code.exact ? `${code.code}＝第 ${code.segs[0]}～${code.segs[2]} 段` : "";
  const guess = code?.exact ? null : acGuessSegment(p.topics, code?.segs);
  let n = code?.exact || guess?.n || null;
  let why = "";
  if (code?.exact) {
    why = `編號 ${code.code} → 第 ${n} 段`;
    const other = acGuessSegment(p.topics);
    if (other && other.n !== n) why += `（但標題比較像第 ${other.n} 段，請確認）`;
  } else if (guess) why = `${half ? `${half}，` : ""}標題對到「${acMatchedWords(p.topics, guess.words).join("、")}」→ 第 ${n} 段`;
  else if (p.topics.length || half) {
    const [a, b] = acSegmentScores(p.topics).filter((s) => !code || code.segs.includes(s.n));
    const scores = a?.hits ? `第 ${a.n} 段對到 ${a.hits} 個詞${b?.hits ? `、第 ${b.n} 段 ${b.hits} 個` : ""}` : "標題對不到，文稿還沒進來？";
    why = `沒有命中段落：${half ? `${half}，` : ""}判斷不出是哪一段（${scores}），請自己選`;
  } else if ($("acStillsMsg").value.trim()) why = "沒有命中段落：訊息開頭沒有 22／23 編號、也看不出主題，請自己選";
  $("acStillsSeg").value = n ? String(n) : "";
  acFillCands();
  $("acStillsWhy").textContent = why;
  // 沒命中、或編號和標題對不起來 → 紅字，不然會以為貼進去就好了
  $("acStillsWhy").className = !n && why ? "bad" : why.includes("請確認") ? "bad" : "muted";

  let found = [];
  let msg = "";
  if (!p.dir) msg = p.prefix ? "訊息裡沒有資料夾路徑，請按「自己選圖片」" : "";
  else if (!p.prefix) msg = "訊息裡找不到檔名（像「22-敬平-…01-03」那一行），請按「自己選圖片」";
  else {
    try {
      found = await invoke("stills_find", { dir: p.dir, prefix: p.prefix, range: p.range });
      const want = p.range ? p.range[1] - p.range[0] + 1 : null;
      msg = found.length
        ? `在 ${p.dir} 找到 ${found.length} 張${want && want !== found.length ? `（訊息說 ${want} 張）` : ""}`
        : `${p.dir} 裡找不到「${p.prefix}」開頭的圖`;
    } catch (e) {
      msg = String(e);
    }
  }
  if (seq !== acStillParseSeq) return;
  // 自己選、拖進來的保留，訊息找到的換新
  const manual = acStillCands.filter((c) => c.manual);
  acStillCands = [...manual, ...found.filter((path) => !manual.some((c) => c.path === path)).map((path) => ({ path }))];
  $("acStillsFoundMsg").textContent = msg;
  $("acStillsFoundMsg").className = msg && !found.length ? "bad" : "muted";
  acFillCands();
}

function acFillCands() {
  $("acStillsCands").replaceChildren(
    ...acStillCands.map((c) =>
      acStillThumb(c.path, () => {
        acStillCands = acStillCands.filter((x) => x !== c);
        acFillCands();
      }),
    ),
  );
  $("acStillsAddBtn").disabled = !acStillCands.length || $("acStillsSeg").value === "";
  $("acStillsAddBtn").textContent = acStillCands.length ? `加入 ${acStillCands.length} 張` : "加入";
  $("acStillsSeg").classList.toggle("bad", !!acStillCands.length && $("acStillsSeg").value === "");
}

function acAddCands(paths) {
  for (const path of paths) if (!acStillCands.some((c) => c.path === path)) acStillCands.push({ path, manual: true });
  acFillCands();
}

/** 清空框裡的內容（加入後留著框，可以接著貼下一則） */
function acResetPaste() {
  acStillParseSeq++; // 還在找圖的結果不要再填回來
  clearTimeout(acStillParseTimer);
  $("acStillsMsg").value = "";
  $("acStillsWhy").textContent = "";
  $("acStillsFoundMsg").textContent = "";
  $("acStillsFoundMsg").className = "muted";
  $("acStillsSeg").value = "";
  acStillCands = [];
  acFillCands();
}

function acClosePaste() {
  $("acStillsPaste").classList.add("hidden");
  acResetPaste();
}

// 預設不選：猜不出來就要人自己選，不會默默放錯段
$("acStillsSeg").replaceChildren(new Option("請選段落", ""), ...AC_STILL_SEGS.map((n) => new Option(acStillSegName(n), String(n))));
$("acStillsSeg").value = "";
$("acStillsSeg").addEventListener("change", acFillCands);
$("acStillsPasteBtn").addEventListener("click", () => {
  if (!acStillsDir()) return acMessage("請先在上面選靜圖資料夾");
  $("acReplace").open = true;
  $("acStillsPaste").classList.remove("hidden");
  $("acStillsMsg").focus();
});
$("acStillsMsg").addEventListener("input", () => {
  clearTimeout(acStillParseTimer);
  acStillParseTimer = setTimeout(acParseStills, 400);
});
$("acStillsPickBtn").addEventListener("click", async () => {
  const picked = await dialog.open({ multiple: true, filters: [{ name: "圖片", extensions: ["jpg", "jpeg", "png"] }] });
  if (picked?.length) acAddCands([].concat(picked));
});
$("acStillsCancelBtn").addEventListener("click", acClosePaste);
$("acStillsAddBtn").addEventListener("click", async () => {
  if ($("acStillsSeg").value === "") return acMessage("請先選要放到第幾段");
  const n = Number($("acStillsSeg").value);
  const paths = acStillCands.map((c) => c.path);
  if (!paths.length) return;
  await acAddStills(n, paths);
  // 框不關，讓人接著貼下一則；要關按「關閉」
  acResetPaste();
  $("acStillsFoundMsg").textContent = `已加入 ${paths.length} 張到${acStillSegName(n)}，可以接著貼下一則`;
  $("acStillsFoundMsg").className = "good";
  $("acStillsMsg").focus();
});
$("acStillsRefresh").addEventListener("click", acLoadStills);
$("acReplace").addEventListener("toggle", () => {
  if ($("acReplace").open) acLoadStills();
});

// 圖片拖進「替換畫面」：拖到貼訊息的框＝加進候選；拖到某一段＝直接加進那段
{
  const IMG_RE = /\.(jpe?g|png)$/i;
  let hover = null;
  const targetAt = (pos) => {
    if ($("page-autocut").classList.contains("hidden") || !$("acReplace").open || !pos) return null;
    const dpr = window.devicePixelRatio || 1;
    const el = document.elementFromPoint(pos.x / dpr, pos.y / dpr);
    return el?.closest("#acStillsPaste:not(.hidden), .ac-still-seg") || null;
  };
  window.__TAURI__.webview.getCurrentWebview().onDragDropEvent(async ({ payload: p }) => {
    hover?.classList.remove("drop-target");
    hover = null;
    if (p.type === "leave") return;
    const target = targetAt(p.position);
    if (!target) return;
    if (p.type === "enter" || p.type === "over") {
      hover = target;
      target.classList.add("drop-target");
      return;
    }
    if (p.type !== "drop") return;
    const paths = (p.paths || []).filter((x) => IMG_RE.test(x));
    if (!paths.length) return;
    if (target.id === "acStillsPaste") acAddCands(paths);
    else await acAddStills(Number(target.dataset.seg), paths);
  });
}

const acStillsPrevSettingsChanged = window.onSettingsChanged;
window.onSettingsChanged = () => {
  acStillsPrevSettingsChanged?.();
  acLoadStills();
};

// 監看中靜圖資料夾變了（別台電腦放進來的），狀態更新時順便重讀；開著才讀、最多 15 秒一次（資料夾可能在網路上）
let acStillsReadAt = 0;
listen("autocut-status", () => {
  if (!$("acReplace").open || Date.now() - acStillsReadAt < 15000) return;
  acStillsReadAt = Date.now();
  acLoadStills();
});

(async function acStillsInit() {
  while (!settings) await new Promise((r) => setTimeout(r, 100));
  acBuildStills();
  acLoadStills();
})();

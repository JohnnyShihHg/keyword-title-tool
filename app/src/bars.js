// 自動切段頁「壓 BAR」：舊標題出現在標題條的時段，整張蓋上新的 BAR 圖（1920×1080 透明 PNG）。
// 每段一個可收合的區塊，裡面可以疊加好幾條。時段在 Rust 端用標題條 OCR 自動找、對準到格（autocut.rs 的 bar_ranges）；
// 填了手動時段就用手動的。設定依日期存在 localStorage，改了就送給監看中的 Rust（autocut_set_bars）。
// 和 autocut.js 共用全域的 invoke、$、settings、listen、dialog、dateInput、convertFileSrc、acStatus、acCurrentTitles。

let acBars = {}; // 段號 → [{ old, image, manual }]（manual 是畫面上打的字：「29:55-31:18, 32:00-32:10」）
let acBarsDate = null;
let acBarTitles = {}; // 段號 → 文稿這段的標題列表（舊標題的下拉選項）
let acBarEls = {}; // 段號 → { details, count, list, items: [{ chips, manualInput, ... }] }

function acBarsKey() {
  return `acBars:${dateInput.value.trim() || "today"}`;
}

function acLoadBars() {
  acBarsDate = dateInput.value.trim();
  try {
    acBars = JSON.parse(localStorage.getItem(acBarsKey()) || "{}") || {};
  } catch {
    acBars = {};
  }
  acBuildBars();
}

function acSaveBars() {
  try {
    localStorage.setItem(acBarsKey(), JSON.stringify(acBars));
  } catch {}
}

/** 「1:02:03.5」「29:55」「95」→ 秒；看不懂回傳 null */
function acParseTime(s) {
  const parts = s.trim().split(":");
  if (!parts.length || parts.length > 3 || parts.some((p) => !/^\d+(\.\d+)?$/.test(p))) return null;
  return parts.reduce((a, p) => a * 60 + parseFloat(p), 0);
}

function acFmtTime(t) {
  const s = Math.max(0, t);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = (s % 60).toFixed(1).padStart(4, "0");
  return h ? `${h}:${String(m).padStart(2, "0")}:${sec}` : `${m}:${sec}`;
}

/** 手動時段「29:55-31:18, 32:00-32:10」→ [[秒, 秒], …]；有一段看不懂就回傳 null */
function acParseRanges(text) {
  const out = [];
  for (const part of text.split(/[,，、\n]/).map((s) => s.trim()).filter(Boolean)) {
    const [a, b] = part.split(/\s*[-~～–]\s*/);
    const s = acParseTime(a || "");
    const e = acParseTime(b || "");
    if (s == null || e == null || e <= s) return null;
    out.push([s, e]);
  }
  return out;
}

/** 有圖、而且有舊標題或手動時段的才算數（跟 Rust 端同一個規則，狀態裡 bars[i] 的順序靠這個對上） */
function acBarComplete(b) {
  return !!b.image && (!!b.old.trim() || !!(acParseRanges(b.manual || "") || []).length);
}

function acBarPayload(n) {
  return (acBars[n] || []).filter(acBarComplete).map((b) => ({ old: b.old.trim(), image: b.image, manual: acParseRanges(b.manual || "") || [] }));
}

function acPushBars(n) {
  acSaveBars();
  invoke("autocut_set_bars", { n, bars: acBarPayload(n) }).catch((e) => acMessage(String(e)));
  acUpdateBarChips();
}

/** 開始監看時一起送的設定 */
function acBarsConfig() {
  return Object.fromEntries([1, 2, 3, 4, 5, 6].map((n) => [n, acBarPayload(n)]).filter(([, b]) => b.length));
}

function acBuildBars() {
  const root = $("acBars");
  const open = new Set(Object.entries(acBarEls).filter(([, el]) => el.details.open).map(([n]) => Number(n)));
  acBarEls = {};
  root.replaceChildren(
    ...[1, 2, 3, 4, 5, 6].map((n) => {
      const details = document.createElement("details");
      details.className = "ac-bar-seg";
      details.open = open.has(n);
      details.innerHTML = `<summary><span class="ac-bar-name">第 ${n} 段</span><span class="ac-bar-count"></span><span class="spacer"></span>
        <button type="button" class="ghost small ac-bar-add">＋ 新增 BAR</button></summary><div class="ac-bar-list"></div>`;
      const el = { details, count: details.querySelector(".ac-bar-count"), list: details.querySelector(".ac-bar-list"), items: [] };
      details.querySelector(".ac-bar-add").onclick = (e) => {
        e.preventDefault(); // 不要觸發收合
        (acBars[n] ||= []).push({ old: "", image: "", manual: "" });
        acSaveBars();
        details.open = true;
        acBuildBarItems(n);
      };
      acBarEls[n] = el;
      acBuildBarItems(n);
      return details;
    }),
  );
}

function acBuildBarItems(n) {
  const el = acBarEls[n];
  const bars = acBars[n] || [];
  el.count.textContent = bars.length ? `${bars.length} 條` : "";
  el.items = [];
  el.list.replaceChildren(
    ...bars.map((b, i) => {
      const item = document.createElement("div");
      item.className = "ac-bar-item";
      item.innerHTML = `<button type="button" class="ac-bar-thumb"></button>
        <div class="ac-bar-fields">
          <label>要換掉的舊標題<input class="ac-bar-old" list="acBarTitles${n}" spellcheck="false" placeholder="從文稿選，或自己打（標題條上原本的字）" /></label>
          <label>時段<input class="ac-bar-manual" spellcheck="false" placeholder="留空＝自動找；手動：29:55-31:18, 32:00-32:10（成品段內時間）" /></label>
          <div class="ac-bar-chips muted"></div>
        </div>
        <button type="button" class="ghost small danger ac-bar-del">刪除</button>`;
      const q = (s) => item.querySelector(s);
      const thumb = q(".ac-bar-thumb");
      const drawThumb = () => {
        thumb.replaceChildren();
        thumb.title = b.image ? `${b.image}\n點一下換別張` : "點一下選 BAR 圖（1920×1080 透明 PNG）";
        if (b.image) {
          const img = document.createElement("img");
          img.src = convertFileSrc(b.image);
          img.alt = b.image.split(/[\\/]/).pop();
          img.onerror = () => thumb.replaceChildren(img.alt);
          thumb.append(img);
        } else thumb.append(Object.assign(document.createElement("span"), { textContent: "選 BAR 圖" }));
      };
      drawThumb();
      thumb.onclick = async () => {
        const path = await dialog.open({ multiple: false, defaultPath: b.image ? b.image.replace(/[\\/][^\\/]*$/, "") : undefined, filters: [{ name: "PNG", extensions: ["png"] }] });
        if (!path) return;
        b.image = path;
        drawThumb();
        acPushBars(n);
      };
      q(".ac-bar-old").value = b.old;
      q(".ac-bar-old").onchange = () => {
        b.old = q(".ac-bar-old").value;
        acPushBars(n);
      };
      const manual = q(".ac-bar-manual");
      manual.value = b.manual || "";
      manual.onchange = () => {
        const ok = acParseRanges(manual.value) != null;
        manual.classList.toggle("bad", !ok);
        manual.title = ok ? "" : "看不懂：每段寫「開始-結束」，用逗號分開，例如 29:55-31:18, 32:00-32:10";
        if (!ok) return;
        b.manual = manual.value;
        acPushBars(n);
      };
      q(".ac-bar-del").onclick = () => {
        bars.splice(i, 1);
        if (!bars.length) delete acBars[n];
        acBuildBarItems(n);
        acPushBars(n);
      };
      el.items.push({ bar: b, chips: q(".ac-bar-chips") });
      return item;
    }),
  );
  if (!bars.length) el.list.append(Object.assign(document.createElement("div"), { className: "muted ac-bar-empty", textContent: "還沒有 BAR。按「＋ 新增 BAR」加一條；同一段可以加好幾條。" }));
  acUpdateBarChips();
}

/** 每條 BAR 實際要蓋的時段（監看狀態送來的；完整的 BAR 依序對到 bars[j]） */
function acUpdateBarChips() {
  for (const [n, el] of Object.entries(acBarEls)) {
    const found = acStatus?.segments?.find((s) => s.n === Number(n))?.bars || [];
    let j = 0;
    for (const it of el.items) {
      const b = it.bar;
      let text;
      if (!b.image) text = "還沒選 BAR 圖";
      else if (!acBarComplete(b)) text = "填舊標題（或手動時段）";
      else {
        const ranges = found[j++];
        if (!acStatus?.running) text = "開始監看後會列出要蓋的時段";
        else if (!ranges) text = "這段還沒錄完，完整後會列出要蓋的時段";
        else if (!ranges.length) text = (b.manual || "").trim() ? "手動時段不在這段裡" : "找不到這個舊標題（OCR 對不到），可以改用手動時段";
        else text = `${(b.manual || "").trim() ? "手動" : "自動找到"} ${ranges.length} 段：${ranges.map(([a, z]) => `${acFmtTime(a)}–${acFmtTime(z)}`).join("、")}`;
      }
      it.chips.textContent = text;
    }
  }
}

/** 段落進度表的「壓 BAR」按鈕：展開那一段、捲過去 */
window.acOpenBars = (n) => {
  const el = acBarEls[n];
  if (!el) return;
  el.details.open = true;
  el.details.scrollIntoView({ behavior: "smooth", block: "center" });
};
window.acBarCount = (n) => (acBars[n] || []).filter(acBarComplete).length;

/** 舊標題的下拉選項：文稿這段的標題列表 */
async function acRefreshBarTitles() {
  let meta = [];
  try {
    meta = await invoke("yt_meta", { text: await acCurrentTitles() });
  } catch {}
  acBarTitles = Object.fromEntries(meta.map((m) => [m.number, m.description.split("\n").filter((l) => l.trim() && !l.trim().startsWith("#"))]));
  let lists = $("acBarTitleLists");
  if (!lists) {
    lists = document.createElement("div");
    lists.id = "acBarTitleLists";
    lists.hidden = true;
    document.body.append(lists);
  }
  lists.replaceChildren(
    ...[1, 2, 3, 4, 5, 6].map((n) => {
      const dl = document.createElement("datalist");
      dl.id = `acBarTitles${n}`;
      dl.append(...(acBarTitles[n] || []).map((t) => new Option(t)));
      return dl;
    }),
  );
}

listen("autocut-status", () => acUpdateBarChips());

// 換日期＝換一組 BAR；文稿改了就更新舊標題的下拉選項
let acBarTitleTimer = null;
const acBarsPrevDocRefreshed = window.onDocRefreshed;
window.onDocRefreshed = () => {
  acBarsPrevDocRefreshed?.();
  if (dateInput.value.trim() !== acBarsDate) acLoadBars();
  clearTimeout(acBarTitleTimer);
  acBarTitleTimer = setTimeout(acRefreshBarTitles, 600);
};

(async function acBarsInit() {
  while (!settings) await new Promise((r) => setTimeout(r, 100));
  acLoadBars();
  acRefreshBarTitles();
})();

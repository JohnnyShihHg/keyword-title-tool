// 自動切段頁「手動遮罩」：自動判斷漏掉的地方，指定時段一定遮（蓋過自動判斷的）。
// 時段是成品的段內時間，可以直接貼同事回報的剪輯時間碼「00;16;01;20-00;16;10;25」（;＝29.97 掉格、:＝不掉格，最後一節是格數）
// 或一般的「16:01.6-16:10.8」。靜圖可以選好幾張：Rust 端照版型切開，每截用同版型最像的；不選就從這段能用的挑（plan::manual_plan）。
// 設定依日期存在 localStorage，改了就送給監看中的 Rust（autocut_set_manual_masks）。
// 和 bars.js 共用 acParseTime、acFmtTime；和 autocut.js 共用 invoke、$、dialog、dateInput、convertFileSrc、acStatus、acMessage。

let acMasks = {}; // 段號 → [{ ranges: "16:01.6-16:10.8", stills: [路徑…] }]（舊設定是單張的 still）
let acMasksDate = null;
let acMaskEls = {}; // 段號 → { details, count, list, items: [{ spec, chips }] }

function acMasksKey() {
  return `acMasks:${dateInput.value.trim() || "today"}`;
}

function acLoadMasks() {
  acMasksDate = dateInput.value.trim();
  try {
    acMasks = JSON.parse(localStorage.getItem(acMasksKey()) || "{}") || {};
  } catch {
    acMasks = {};
  }
  acBuildMasks();
}

function acSaveMasks() {
  try {
    localStorage.setItem(acMasksKey(), JSON.stringify(acMasks));
  } catch {}
}

/** 剪輯時間碼「00;16;01;20」「00:16:01:20」→ 秒（29.97fps；用 ; 的是掉格時間碼）；其他格式交給 acParseTime */
function acParseMaskTime(s) {
  const t = s.trim();
  const m = t.match(/^(\d+)[:;](\d+)[:;](\d+)([:;.,])(\d+)$/);
  if (m && (t.includes(";") || m[4] === ":")) {
    const [h, mi, se, ff] = [m[1], m[2], m[3], m[5]].map(Number);
    if (mi > 59 || se > 59 || ff > 29) return null;
    const minutes = h * 60 + mi;
    let frames = (minutes * 60 + se) * 30 + ff;
    if (t.includes(";")) frames -= 2 * (minutes - Math.floor(minutes / 10));
    return frames / (30000 / 1001);
  }
  return acParseTime(t);
}

/** 「16:01.6-16:10.8, 00;17;00;00-00;17;05;00」→ [[秒, 秒], …]；看不懂回傳 null */
function acParseMaskRanges(text) {
  const out = [];
  for (const part of text.split(/[,，、\n]/).map((s) => s.trim()).filter(Boolean)) {
    const [a, b, extra] = part.split(/\s*[-~～–]\s*/);
    const s = acParseMaskTime(a || "");
    const e = acParseMaskTime(b || "");
    if (extra != null || s == null || e == null || e <= s) return null;
    out.push([s, e]);
  }
  return out;
}

function acMaskComplete(m) {
  return !!(acParseMaskRanges(m.ranges || "") || []).length;
}

/** 選的靜圖（舊設定只有一張 still，讀進來時併進 stills） */
function acMaskStills(m) {
  if (m.still) {
    m.stills = [...new Set([...(m.stills || []), m.still])];
    delete m.still;
  }
  return (m.stills ||= []);
}

function acMaskPayload(n) {
  return (acMasks[n] || []).filter(acMaskComplete).map((m) => ({ ranges: acParseMaskRanges(m.ranges), stills: acMaskStills(m) }));
}

function acPushMasks(n) {
  acSaveMasks();
  invoke("autocut_set_manual_masks", { n, masks: acMaskPayload(n) }).catch((e) => acMessage(String(e)));
  acUpdateMaskChips();
}

/** 開始監看時一起送的設定 */
function acMasksConfig() {
  return Object.fromEntries([1, 2, 3, 4, 5, 6].map((n) => [n, acMaskPayload(n)]).filter(([, m]) => m.length));
}

function acBuildMasks() {
  const root = $("acMasks");
  if (!root) return;
  const open = new Set(Object.entries(acMaskEls).filter(([, el]) => el.details.open).map(([n]) => Number(n)));
  acMaskEls = {};
  root.replaceChildren(
    ...[1, 2, 3, 4, 5, 6].map((n) => {
      const details = document.createElement("details");
      details.className = "ac-bar-seg";
      details.open = open.has(n) || !!(acMasks[n] || []).length;
      details.innerHTML = `<summary><span class="ac-bar-name">第 ${n} 段</span><span class="ac-bar-count"></span><span class="spacer"></span>
        <button type="button" class="ghost small ac-bar-add">＋ 新增遮罩</button></summary><div class="ac-bar-list"></div>`;
      const el = { details, count: details.querySelector(".ac-bar-count"), list: details.querySelector(".ac-bar-list"), items: [] };
      details.querySelector(".ac-bar-add").onclick = (e) => {
        e.preventDefault(); // 不要觸發收合
        (acMasks[n] ||= []).push({ ranges: "", stills: [] });
        acSaveMasks();
        details.open = true;
        acBuildMaskItems(n);
        el.list.lastElementChild?.querySelector(".ac-mask-ranges")?.focus();
      };
      acMaskEls[n] = el;
      acBuildMaskItems(n);
      return details;
    }),
  );
}

function acBuildMaskItems(n) {
  const el = acMaskEls[n];
  const specs = acMasks[n] || [];
  el.count.textContent = specs.length ? `${specs.length} 個` : "";
  el.items = [];
  el.list.replaceChildren(
    ...specs.map((m, i) => {
      const item = document.createElement("div");
      item.className = "ac-bar-item";
      item.innerHTML = `<div class="ac-bar-fields">
          <label>時段（成品段內時間）<input class="ac-mask-ranges" spellcheck="false" placeholder="可以直接貼：00;16;01;20-00;16;10;25；或 16:01.6-16:10.8，好幾段用逗號分開" /></label>
          <div class="ac-mask-label">靜圖（可以選好幾張：滿版的地方用滿版圖、雙框的地方用雙框圖蓋小框，各自挑最像的；不選＝自動挑）</div>
          <div class="ac-mask-stills"></div>
          <div class="ac-bar-chips muted"></div>
        </div>
        <button type="button" class="ghost small danger ac-bar-del">刪除</button>`;
      const q = (s) => item.querySelector(s);
      const list = acMaskStills(m);
      const box = q(".ac-mask-stills");
      const drawStills = () => {
        box.replaceChildren(
          ...list.map((path, j) => {
            const t = document.createElement("button");
            t.type = "button";
            t.className = "ac-bar-thumb ac-mask-thumb";
            t.title = `${path}\n點一下移除這張`;
            const img = document.createElement("img");
            img.src = convertFileSrc(path);
            img.alt = path.split(/[\\/]/).pop();
            img.onerror = () => t.replaceChildren(img.alt);
            t.append(img);
            t.onclick = () => {
              list.splice(j, 1);
              drawStills();
              acPushMasks(n);
            };
            return t;
          }),
        );
        const add = document.createElement("button");
        add.type = "button";
        add.className = "ac-bar-thumb ac-mask-thumb";
        add.title = "選靜圖（要是靜圖資料夾裡的圖，可以一次選好幾張）";
        add.append(Object.assign(document.createElement("span"), { textContent: list.length ? "＋ 再加" : "＋ 選靜圖（不選＝自動挑）" }));
        add.onclick = async () => {
          const dir = list.length ? list[list.length - 1].replace(/[\\/][^\\/]*$/, "") : $("acStillsDir")?.value?.trim() || undefined;
          const picked = await dialog.open({ multiple: true, defaultPath: dir, filters: [{ name: "圖片", extensions: ["jpg", "jpeg", "png"] }] });
          const paths = (Array.isArray(picked) ? picked : picked ? [picked] : []).filter((p) => !list.includes(p));
          if (!paths.length) return;
          list.push(...paths);
          drawStills();
          acPushMasks(n);
        };
        box.append(add);
      };
      drawStills();
      const ranges = q(".ac-mask-ranges");
      ranges.value = m.ranges || "";
      const commit = () => {
        const ok = acParseMaskRanges(ranges.value) != null;
        ranges.classList.toggle("bad", !ok);
        ranges.title = ok ? "" : "看不懂：每段寫「開始-結束」，例如 00;16;01;20-00;16;10;25 或 16:01.6-16:10.8，好幾段用逗號分開";
        if (!ok) return;
        m.ranges = ranges.value;
        acPushMasks(n);
      };
      ranges.onchange = commit;
      // 貼上後馬上套用（不用再點別的地方）
      ranges.addEventListener("paste", () => setTimeout(commit, 0));
      q(".ac-bar-del").onclick = () => {
        specs.splice(i, 1);
        if (!specs.length) delete acMasks[n];
        acBuildMaskItems(n);
        acPushMasks(n);
      };
      el.items.push({ spec: m, chips: q(".ac-bar-chips") });
      return item;
    }),
  );
  if (!specs.length) el.list.append(Object.assign(document.createElement("div"), { className: "muted ac-bar-empty", textContent: "沒有手動遮罩。同事回報哪裡沒遮到，按「＋ 新增遮罩」把時間貼進來。" }));
  acUpdateMaskChips();
}

/** 每個手動遮罩實際要遮的時段（監看狀態送來的；填好的依序對到 manual[j]） */
function acUpdateMaskChips() {
  for (const [n, el] of Object.entries(acMaskEls)) {
    const found = acStatus?.segments?.find((s) => s.n === Number(n))?.manual || [];
    let j = 0;
    for (const it of el.items) {
      const m = it.spec;
      let text;
      const parsed = acParseMaskRanges(m.ranges || "");
      if (!parsed || !parsed.length) text = "填要遮的時段";
      else {
        const got = found[j++];
        const want = parsed.map(([a, z]) => `${acFmtTime(a)}–${acFmtTime(z)}`).join("、");
        if (!acStatus?.running) text = `會遮 ${want}（開始監看後套用）`;
        else if (!got) text = `會遮 ${want}（這段還沒錄完，完整後套用）`;
        else if (!got.length) text = "時段不在這段裡（超過這段長度？）";
        else text = `遮 ${got.map(([a, z, name]) => `${acFmtTime(a)}–${acFmtTime(z)}（${name.split(/[\\/]/).pop()}）`).join("、")}`;
      }
      it.chips.textContent = text;
    }
  }
}

window.acMaskCount = (n) => (acMasks[n] || []).filter(acMaskComplete).length;

listen("autocut-status", () => acUpdateMaskChips());

// 換日期＝換一組手動遮罩
const acMasksPrevDocRefreshed = window.onDocRefreshed;
window.onDocRefreshed = () => {
  acMasksPrevDocRefreshed?.();
  if (dateInput.value.trim() !== acMasksDate) acLoadMasks();
};

(async function acMasksInit() {
  while (!settings) await new Promise((r) => setTimeout(r, 100));
  acLoadMasks();
})();

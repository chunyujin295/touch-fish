/* TouchFish 书库前端：无框架、无构建、零依赖。
   所有内容渲染都由本地服务端直出，这里只负责书库管理与入口跳转。 */

const $ = (s, el = document) => el.querySelector(s);
const $$ = (s, el = document) => [...el.querySelectorAll(s)];

const state = {
  books: [],
  sites: [],
  packs: [],
  settings: {},
  current: null,
  filter: "",
};

const PALETTE = [
  ["#ff8a4c", "#e0451f"],
  ["#5b8def", "#2f4fd0"],
  ["#12b76a", "#0a7d48"],
  ["#a78bfa", "#6d3fe0"],
  ["#f59e0b", "#b45309"],
  ["#22d3ee", "#0e7490"],
  ["#f472b6", "#be185d"],
  ["#94a3b8", "#475467"],
];

function hueOf(str) {
  let h = 0;
  for (let i = 0; i < str.length; i++) h = (h * 31 + str.charCodeAt(i)) & 0xffff;
  return PALETTE[h % PALETTE.length];
}

function esc(s) {
  return String(s == null ? "" : s).replace(/[&<>"']/g, (c) => ({
    "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
  }[c]));
}

function toast(msg, isErr) {
  const t = $("#toast");
  t.textContent = msg;
  t.className = "toast show" + (isErr ? " err" : "");
  clearTimeout(t._timer);
  t._timer = setTimeout(() => (t.className = "toast"), 2600);
}

async function api(url, opts) {
  const r = await fetch(url, opts);
  if (!r.ok) {
    const text = await r.text().catch(() => "");
    throw new Error(text || r.statusText);
  }
  return r.json();
}

/* ================= 加载 ================= */

async function boot() {
  try {
    const [books, sites, packs, settings] = await Promise.all([
      api("/api/books"),
      api("/api/sites"),
      api("/api/packs"),
      api("/api/settings"),
    ]);
    state.books = books;
    state.sites = sites;
    state.packs = packs;
    state.settings = settings;
    renderShelf();
    renderSites();
    renderSettings();
    $("#dataDir").textContent = settings.data_dir || "—";
  } catch (e) {
    toast("加载失败：" + e.message, true);
  }
}

/* ================= 书库 ================= */

function wordsLabel(n) {
  if (n >= 10000) return (n / 10000).toFixed(1) + " 万字";
  return n + " 字";
}

function renderShelf() {
  const q = state.filter.trim().toLowerCase();
  const list = state.books.filter(
    (b) => !q || b.title.toLowerCase().includes(q) || b.author.toLowerCase().includes(q)
  );

  $("#empty").classList.toggle("hidden", state.books.length > 0);
  $("#bookGrid").classList.toggle("hidden", state.books.length === 0);

  const totalWords = state.books.reduce((a, b) => a + b.words, 0);
  const reading = state.books.filter((b) => b.progress != null).length;
  $("#stats").innerHTML = state.books.length
    ? `<div class="st"><b>${state.books.length}</b><span>本书</span></div>
       <div class="st"><b>${wordsLabel(totalWords)}</b><span>累计字数</span></div>
       <div class="st"><b>${reading}</b><span>在读 / 已读</span></div>`
    : "";

  $("#bookGrid").innerHTML = list
    .map((b) => {
      const [h1, h2] = hueOf(b.id);
      const pct = b.progress != null ? Math.round(((b.progress + 1) / Math.max(1, b.chapters)) * 100) : 0;
      const cover = b.cover
        ? `<img src="${esc(b.cover)}" alt="">`
        : `<div class="t">${esc(b.title)}</div><div class="a">${esc(b.author)}</div>`;
      return `<div class="book" data-id="${esc(b.id)}">
        <div class="cover" style="--h1:${h1};--h2:${h2}">
          <span class="fmt">${b.format.toUpperCase()}</span>${cover}
        </div>
        <div class="bk-t">${esc(b.title)}</div>
        <div class="bk-a">${esc(b.author)} · ${b.chapters} 章</div>
        <div class="bk-p"><i style="width:${pct}%"></i></div>
        <div class="bk-pd"><span>${pct > 0 ? "已读 " + pct + "%" : "未开始"}</span><b>${wordsLabel(b.words)}</b></div>
      </div>`;
    })
    .join("");

  if (list.length === 0 && state.books.length > 0) {
    $("#bookGrid").innerHTML = `<p style="color:#8b95a5;font-size:13px;padding:20px 0">没有匹配「${esc(state.filter)}」的书</p>`;
  }
}

/* ================= 抽屉 ================= */

async function openBook(id) {
  let detail;
  try {
    detail = await api("/api/books/" + id);
  } catch (e) {
    return toast("打开失败：" + e.message, true);
  }
  const book = state.books.find((b) => b.id === id) || detail;
  state.current = detail;

  const [h1, h2] = hueOf(id);
  const cover = detail.cover
    ? `<img src="${esc(detail.cover)}" alt="">`
    : `<div style="color:#fff;font-size:11px;font-weight:700;padding:14px 8px;line-height:1.4">${esc(detail.title)}</div>`;
  $("#dwCover").style.background = `linear-gradient(150deg,${h1},${h2})`;
  $("#dwCover").innerHTML = cover;
  $("#dwTitle").textContent = detail.title;
  $("#dwAuthor").textContent = detail.author || "佚名";
  const p = detail.progress;
  $("#dwStat").textContent = `${detail.format.toUpperCase()} · ${detail.chapters.length} 章 · ${wordsLabel(book.words || 0)}${p != null ? " · 读到第 " + (p + 1) + " 章" : ""}`;

  $("#dwSite").innerHTML = state.sites
    .map((s) => `<option value="${s.id}">${esc(s.name)} · ${s.layout}</option>`)
    .join("");
  $("#dwPack").innerHTML = state.packs
    .map((p2) => `<option value="${p2.id}">${esc(p2.name)}</option>`)
    .join("");
  $("#dwSite").value = state.settings.default_site || "csdn";
  $("#dwPack").value = state.settings.default_pack || "cs";

  $("#dwChCount").textContent = detail.chapters.length + " 章";
  $("#chList").innerHTML = detail.chapters
    .map(
      (c, i) =>
        `<a href="#" data-ch="${i}" class="${i === p ? "cur" : ""}"><span class="n">${String(i + 1).padStart(2, "0")}</span><span class="tx">${esc(c.title)}</span></a>`
    )
    .join("");

  $("#dwRead").textContent = p != null ? "继续阅读 · 第 " + (p + 1) + " 章" : "开始阅读";
  $("#scrim").classList.remove("hidden");
  requestAnimationFrame(() => $("#drawer").classList.add("open"));
}

function closeDrawer() {
  $("#drawer").classList.remove("open");
  $("#scrim").classList.add("hidden");
}

function readUrl(id, idx) {
  const site = $("#dwSite").value || state.settings.default_site || "csdn";
  const pack = $("#dwPack").value || state.settings.default_pack || "cs";
  return `/r/${id}/${idx}?site=${encodeURIComponent(site)}&pack=${encodeURIComponent(pack)}`;
}

/* ================= 导入 ================= */

async function importFiles(files) {
  const list = [...files].filter((f) => /\.(epub|mobi|azw|prc|txt)$/i.test(f.name));
  if (!list.length) return toast("只支持 .epub / .mobi / .txt", true);
  let ok = 0;
  for (const f of list) {
    const fd = new FormData();
    fd.append("file", f, f.name);
    try {
      const res = await api("/api/import", { method: "POST", body: fd });
      ok++;
      if (res.duplicated) toast(`《${res.book.title}》已在书库`);
    } catch (e) {
      toast(`${f.name} 导入失败：${e.message}`, true);
    }
  }
  if (ok) {
    toast(`已导入 ${ok} 本`);
    state.books = await api("/api/books");
    renderShelf();
  }
}

/* ================= 皮肤 / 设置 ================= */

function renderSites() {
  $("#siteCards").innerHTML = state.sites
    .map(
      (s) => `<div class="scard">
      <div class="sc-head"><i class="sc-dot" style="background:${esc(s.accent)}"></i><b>${esc(s.name)}</b>
        <span class="lay">${esc(s.layout)}</span></div>
      <p>${esc(s.desc || s.host)}</p>
      <div class="sc-foot"><span>${esc(s.host)}</span>
        <a href="#" data-try="${esc(s.id)}">用这本书试读 →</a></div>
    </div>`
    )
    .join("");

  $("#packCards").innerHTML = state.packs
    .map(
      (p) => `<div class="pk"><i style="background:${esc(p.accent)}"></i>${esc(p.name)}
      <span>${esc((p.terms || []).slice(0, 2).join(" / "))}</span></div>`
    )
    .join("");
}

function renderSettings() {
  const s = state.settings;
  $("#setSite").innerHTML = state.sites.map((x) => `<option value="${x.id}">${esc(x.name)}</option>`).join("");
  $("#setPack").innerHTML = state.packs.map((x) => `<option value="${x.id}">${esc(x.name)}</option>`).join("");
  $("#setSite").value = s.default_site || "csdn";
  $("#setPack").value = s.default_pack || "cs";
  $("#setBoss").value = s.boss_url || "";
  $("#setBlur").checked = s.pause_on_blur !== false;
  $("#setHotkey").value = s.hotkey || "ctrl+alt+f";
  $("#setTray").checked = s.tray !== false;
  $("#setKeys").checked = s.reader_keys !== false;
  $("#setPort").value = s.port || 7317;
}

async function saveSettings() {
  const body = {
    default_site: $("#setSite").value,
    default_pack: $("#setPack").value,
    boss_url: $("#setBoss").value,
    pause_on_blur: $("#setBlur").checked,
    hotkey: $("#setHotkey").value.trim() || "ctrl+alt+f",
    tray: $("#setTray").checked,
    reader_keys: $("#setKeys").checked,
    port: Number($("#setPort").value) || 7317,
  };
  try {
    const next = await api("/api/settings", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    });
    state.settings = next;
    $("#saveHint").textContent = "已保存";
    setTimeout(() => ($("#saveHint").textContent = ""), 2200);
  } catch (e) {
    toast("保存失败：" + e.message, true);
  }
}

/* ================= 视图切换 ================= */

function showView(name) {
  $$(".nv").forEach((b) => b.classList.toggle("on", b.dataset.view === name));
  $$(".view").forEach((v) => v.classList.toggle("hidden", v.id !== "view-" + name));
  const titles = {
    shelf: ["书库", "把本地电子书渲染成以假乱真的社区网页"],
    sites: ["皮肤与主题包", "版式决定像不像，语境决定真不真"],
    settings: ["设置", "默认皮肤、主题包与 Boss Key 落点"],
  };
  $("#viewTitle").textContent = titles[name][0];
  $("#viewSub").textContent = titles[name][1];
}

/* ================= 事件 ================= */

document.addEventListener("DOMContentLoaded", () => {
  boot();

  $$(".nv").forEach((b) => b.addEventListener("click", () => showView(b.dataset.view)));

  $("#bookGrid").addEventListener("click", (e) => {
    const card = e.target.closest(".book");
    if (card) openBook(card.dataset.id);
  });

  $("#scrim").addEventListener("click", closeDrawer);
  $("#drawerClose").addEventListener("click", closeDrawer);

  $("#chList").addEventListener("click", (e) => {
    e.preventDefault();
    const a = e.target.closest("a[data-ch]");
    if (a && state.current) location.href = readUrl(state.current.id, a.dataset.ch);
  });

  $("#dwRead").addEventListener("click", () => {
    if (!state.current) return;
    const idx = state.current.progress != null ? state.current.progress : 0;
    location.href = readUrl(state.current.id, idx);
  });

  $("#dwDelete").addEventListener("click", async () => {
    if (!state.current) return;
    if (!confirm(`从书库移除《${state.current.title}》？文件不会被删除，只是不再显示。`)) return;
    await api("/api/books/" + state.current.id, { method: "DELETE" });
    state.books = await api("/api/books");
    renderShelf();
    closeDrawer();
    toast("已移除");
  });

  $("#siteCards").addEventListener("click", (e) => {
    const a = e.target.closest("[data-try]");
    if (!a) return;
    e.preventDefault();
    const book = state.books[0];
    if (!book) return toast("先导入一本书", true);
    location.href = `/r/${book.id}/0?site=${a.dataset.try}&pack=${state.settings.default_pack || "cs"}`;
  });

  $("#importBtn").addEventListener("click", () => $("#fileInput").click());
  $("#fileInput").addEventListener("change", (e) => importFiles(e.target.files));

  $("#searchInput").addEventListener("input", (e) => {
    state.filter = e.target.value;
    renderShelf();
  });

  $("#saveSettings").addEventListener("click", saveSettings);

  // 拖拽导入
  let dragDepth = 0;
  window.addEventListener("dragenter", (e) => {
    e.preventDefault();
    dragDepth++;
    $("#dropmask").classList.remove("hidden");
  });
  window.addEventListener("dragover", (e) => e.preventDefault());
  window.addEventListener("dragleave", (e) => {
    e.preventDefault();
    if (--dragDepth <= 0) {
      dragDepth = 0;
      $("#dropmask").classList.add("hidden");
    }
  });
  window.addEventListener("drop", (e) => {
    e.preventDefault();
    dragDepth = 0;
    $("#dropmask").classList.add("hidden");
    if (e.dataTransfer.files.length) importFiles(e.dataTransfer.files);
  });

  document.addEventListener("keydown", (e) => {
    const typing =
      e.target &&
      (e.target.tagName === "INPUT" ||
        e.target.tagName === "TEXTAREA" ||
        e.target.isContentEditable);
    if (e.key === "Escape") {
      if ($("#drawer").classList.contains("open")) return closeDrawer();
      if (typing) {
        e.target.blur();
        $("#searchInput").value = "";
        $("#searchInput").dispatchEvent(new Event("input"));
      }
      return;
    }
    // 斜杠聚焦搜索，和大多数阅读器一致；正在输入时不抢键
    if (e.key === "/" && !typing && !e.ctrlKey && !e.metaKey) {
      e.preventDefault();
      showView("shelf");
      $("#searchInput").focus();
    }
    // Ctrl/Cmd + O 导入
    if (e.key.toLowerCase() === "o" && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      $("#fileInput").click();
    }
  });
});

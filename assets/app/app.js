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

/* ================= 外观 ================= */

const THEME_KEY = "tf-theme";
const THEME_LABEL = { auto: "跟随系统", light: "浅色", dark: "深色" };
const darkQuery = window.matchMedia ? window.matchMedia("(prefers-color-scheme: dark)") : null;

function normalizeTheme(v) {
  return v === "light" || v === "dark" ? v : "auto";
}

// auto 模式下真正用哪套配色，只有系统说了算
function resolvedTheme(mode) {
  if (mode === "light" || mode === "dark") return mode;
  return darkQuery && darkQuery.matches ? "dark" : "light";
}

function paintTheme(mode) {
  const m = normalizeTheme(mode);
  const dark = resolvedTheme(m) === "dark";
  const root = document.documentElement;
  root.dataset.theme = dark ? "dark" : "light";
  // 单独记一份"用户选的模式"，顶栏图标要靠它区分 跟随系统 / 浅色 / 深色
  root.dataset.themeMode = m;
  // 缓存只为了让下次启动首帧不白闪，设置本身以服务端为准
  try {
    localStorage.setItem(THEME_KEY, m);
  } catch (e) {}
  const btn = $("#themeBtn");
  if (btn) {
    const tip = `外观：${THEME_LABEL[m]}（当前${dark ? "深色" : "浅色"}）· 点击切换`;
    btn.title = tip;
    btn.setAttribute("aria-label", tip);
  }
  const sel = $("#setTheme");
  if (sel) sel.value = m;
  return m;
}

// 顶栏按钮：跟随系统 → 固定成与当前相反的配色 → 另一种 → 回到跟随系统
async function cycleTheme() {
  const cur = normalizeTheme(state.settings.appearance || document.documentElement.dataset.themeMode);
  const next =
    cur === "auto" ? (resolvedTheme("auto") === "dark" ? "light" : "dark") : cur === "light" ? "dark" : "auto";
  state.settings.appearance = paintTheme(next);
  try {
    state.settings = await api("/api/settings", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ appearance: next }),
    });
    paintTheme(state.settings.appearance);
  } catch (e) {
    toast("外观已切换，但没能保存：" + e.message, true);
  }
}

/* ================= 自定义图标 ================= */

const DEFAULT_LOGO = "/assets/app/logo.png";

// 空字符串 = 用自带图标；有值就用服务端给的 /brand/logo.xxx?v=...
function applyLogo(url) {
  const src = url || DEFAULT_LOGO;
  state.settings.logo = url || "";
  $("#brandLogo").src = src;
  $("#logoPreview").src = src;
  $("#favicon").href = src;
  // 用的就是自带图标时，"恢复默认"没有意义
  $("#logoReset").disabled = !url;
}

async function uploadLogo(file) {
  if (!file) return;
  if (file.size > 2 * 1024 * 1024) return toast("图标不要超过 2 MB", true);
  const fd = new FormData();
  fd.append("file", file, file.name);
  try {
    const res = await api("/api/logo", { method: "POST", body: fd });
    applyLogo(res.logo);
    toast("图标已更新");
  } catch (e) {
    toast("上传失败：" + e.message, true);
  }
}

async function resetLogo() {
  try {
    const res = await api("/api/logo", { method: "DELETE" });
    applyLogo(res.logo);
    toast("已恢复默认图标");
  } catch (e) {
    toast("恢复失败：" + e.message, true);
  }
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
    // 服务端才是准的：缓存里可能是上一次会话的旧选择
    paintTheme(settings.appearance);
    applyLogo(settings.logo);
    renderShelf();
    renderSites();
    renderSettings();
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
    $("#bookGrid").innerHTML = `<p style="color:var(--ink3);font-size:13px;padding:20px 0">没有匹配「${esc(state.filter)}」的书</p>`;
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
  // 桌面模式给一个"在浏览器中打开"；纯网页模式给一个真正的退出入口
  const web = s.mode === "web";
  $("#rowOpenBrowser").classList.toggle("hidden", web);
  $("#rowQuit").classList.toggle("hidden", !web);
  // 热键和托盘是桌面窗口的能力，网页模式下没有窗口可唤，露出来只会让人白改
  $("#rowHotkey").classList.toggle("hidden", web);
  $("#rowTray").classList.toggle("hidden", web);
  $("#setSite").innerHTML = state.sites.map((x) => `<option value="${x.id}">${esc(x.name)}</option>`).join("");
  $("#setPack").innerHTML = state.packs.map((x) => `<option value="${x.id}">${esc(x.name)}</option>`).join("");
  $("#setTheme").value = normalizeTheme(s.appearance);
  $("#setSite").value = s.default_site || "csdn";
  $("#setPack").value = s.default_pack || "cs";
  $("#setBoss").value = s.boss_url || "";
  $("#setBlur").checked = s.pause_on_blur !== false;
  $("#setHotkey").value = s.hotkey || "ctrl+alt+f";
  $("#setTray").checked = s.tray !== false;
  $("#setKeys").checked = s.reader_keys !== false;
  $("#setPort").value = s.port || 7317;

  // 数据目录：环境变量指定时不给改，否则没法保证下次启动还认这个位置
  $("#setDataDir").value = s.data_dir || "";
  const dirLocked = !!s.data_dir_env;
  $("#setDataDir").disabled = dirLocked;
  $("#migrateData").disabled = dirLocked;
  $("#dataDirHint").textContent = dirLocked
    ? "当前由环境变量 TOUCHFISH_DATA 指定，界面里不能修改"
    : "书籍、封面、进度、自定义图标都在这里。填一个空目录即可把现有数据复制过去并立即切换，旧目录里的数据随后清理";
}

const HOTKEY_MODS = new Set([
  "ctrl", "control", "alt", "option", "opt", "shift", "win", "super", "meta", "cmd", "command",
]);

// 和后端 parse_hotkey 保持同一套口径：至少一个修饰键 + 一个主键。
// 只做形状校验，真正能不能注册上（有没有被别的软件占用）只有系统知道。
function hotkeyLooksOk(spec) {
  const parts = spec.split(/[+\-]/).map((p) => p.trim().toLowerCase()).filter(Boolean);
  if (parts.length < 2) return false;
  return parts.some((p) => HOTKEY_MODS.has(p)) && parts.some((p) => !HOTKEY_MODS.has(p));
}

async function saveSettings() {
  const hotkey = $("#setHotkey").value.trim().toLowerCase() || "ctrl+alt+f";
  if (!hotkeyLooksOk(hotkey)) {
    return toast("热键写法不对：需要「修饰键 + 主键」，例如 ctrl+alt+f", true);
  }
  const hotkeyChanged = hotkey !== (state.settings.hotkey || "ctrl+alt+f").toLowerCase();
  const body = {
    appearance: $("#setTheme").value,
    default_site: $("#setSite").value,
    default_pack: $("#setPack").value,
    boss_url: $("#setBoss").value,
    pause_on_blur: $("#setBlur").checked,
    hotkey,
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
    paintTheme(next.appearance);
    // 热键是运行中重新注册的，不用重启；被别的软件占用时后端会保留旧键
    $("#saveHint").textContent = hotkeyChanged ? "已保存，热键已生效" : "已保存";
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

  $("#themeBtn").addEventListener("click", cycleTheme);

  // 桌面窗口里点一下就把同一个书库交给系统浏览器，不必回到命令行。
  // 后端会先把窗口收起来（和托盘「打开网页页面」一致），所以提示里要说清楚人去哪了。
  $("#openBrowser").addEventListener("click", async () => {
    try {
      const res = await api("/api/open-browser", { method: "POST" });
      toast("已收起到托盘，并在浏览器中打开 " + res.url);
    } catch (e) {
      toast("打开失败：" + e.message, true);
    }
  });

  $("#quitApp").addEventListener("click", async () => {
    if (!confirm("退出后本地服务会停止，这个页面也将无法访问。确定退出？")) return;
    try {
      await api("/api/shutdown", { method: "POST" });
    } catch (e) {
      // 服务端会先回包再退出，偶尔回包还没读完连接就断了，这里不当作失败
    }
    document.body.innerHTML = '<div class="bye">TouchFish 已退出，可以关闭这个标签页。</div>';
  });
  // 选了"跟随系统"时，系统在太阳落山后切到深色，这里要跟着变
  if (darkQuery) {
    const onSchemeChange = () => {
      if ((document.documentElement.dataset.themeMode || "auto") === "auto") paintTheme("auto");
    };
    if (darkQuery.addEventListener) darkQuery.addEventListener("change", onSchemeChange);
    else if (darkQuery.addListener) darkQuery.addListener(onSchemeChange);
  }

  // 阅读页按 D 会把选择写进 localStorage，书库这个标签页跟着切
  window.addEventListener("storage", (e) => {
    if (e.key !== "tf-theme") return;
    const mode = normalizeTheme(e.newValue);
    state.settings.appearance = mode;
    paintTheme(mode);
  });

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

  $("#logoPick").addEventListener("click", () => $("#logoInput").click());
  $("#logoInput").addEventListener("change", (e) => {
    uploadLogo(e.target.files[0]);
    // 清空 value，否则连着选同一个文件不会再触发 change
    e.target.value = "";
  });
  $("#logoReset").addEventListener("click", resetLogo);

  $("#searchInput").addEventListener("input", (e) => {
    state.filter = e.target.value;
    renderShelf();
  });

  $("#saveSettings").addEventListener("click", saveSettings);

  // 热键改坏了也能一键回到默认
  $("#hotkeyReset").addEventListener("click", () => {
    $("#setHotkey").value = "ctrl+alt+f";
  });

  // 外观是全局的，改完立刻生效并落盘：阅读页的 localStorage 监听会跟着切
  $("#setTheme").addEventListener("change", async (e) => {
    const mode = normalizeTheme(e.target.value);
    state.settings.appearance = paintTheme(mode);
    try {
      state.settings = await api("/api/settings", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ appearance: mode }),
      });
      paintTheme(state.settings.appearance);
    } catch (err) {
      toast("外观没能保存：" + err.message, true);
    }
  });

  $("#migrateData").addEventListener("click", async () => {
    const target = $("#setDataDir").value.trim().replace(/^"+|"+$/g, "");
    if (!target) return toast("先填一个目录，例如 D:\\TouchFish", true);
    if (target === state.settings.data_dir) return toast("和当前目录一样", true);
    if (
      !confirm(
        `把数据迁移到：\n${target}\n\n会先复制现有书库，成功后清理旧目录里的数据并立即生效。继续？`
      )
    )
      return;
    const btn = $("#migrateData");
    btn.disabled = true;
    btn.textContent = "迁移中…";
    try {
      const res = await api("/api/data-dir", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ path: target }),
      });
      state.settings.data_dir = res.data_dir;
      $("#setDataDir").value = res.data_dir;
      toast("已迁移到 " + res.data_dir);
    } catch (e) {
      toast("迁移失败：" + e.message, true);
    } finally {
      btn.textContent = "迁移";
      btn.disabled = !!state.settings.data_dir_env;
    }
  });

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

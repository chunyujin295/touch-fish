// 阅读页主题选择的回归测试。
//
// 背景：曾经的实现是 localStorage 优先，一次按键留下的旧值会永久压过设置，
// 表现就是"设置里明明选了深色，阅读页还是整页白底"。这里把主题脚本抽出来，
// 套一个最小 DOM 桩，逐场景验证"服务端为准、缓存只兜底"。
//
//   node scripts/test_reader_theme.js
//
// 直接读 assets/reader/layout-*.html，不需要起服务。

const fs = require("fs");
const path = require("path");
const vm = require("vm");

const ROOT = path.join(__dirname, "..");
const LAYOUTS = ["blog", "forum", "acad"].map((n) =>
  path.join(ROOT, "assets", "reader", `layout-${n}.html`)
);

function extractThemeScript(file) {
  const html = fs.readFileSync(file, "utf8");
  const m = html.match(/<script>([\s\S]*?)<\/script>/);
  if (!m) throw new Error(`${file} 里没有内联脚本`);
  return m[1];
}

/**
 * @param {string} js 主题脚本
 * @param {{serverMode?:string, cache?:string|null, prefersDark?:boolean}} opt
 */
function run(js, opt = {}) {
  const { serverMode, cache = null, prefersDark = false } = opt;
  const dataset = {};
  if (serverMode !== undefined) dataset.themeMode = serverMode;

  const store = {};
  if (cache !== null) store["tf-theme"] = cache;

  const listeners = {};
  const mediaListeners = [];

  const document = { documentElement: { dataset } };
  const localStorage = {
    getItem: (k) => (k in store ? store[k] : null),
    setItem: (k, v) => {
      store[k] = String(v);
    },
  };
  const window = {
    matchMedia: (q) => ({
      matches: prefersDark,
      media: q,
      addEventListener: (_t, fn) => mediaListeners.push(fn),
    }),
    addEventListener: (t, fn) => {
      (listeners[t] = listeners[t] || []).push(fn);
    },
  };

  const sandbox = { document, window, localStorage, console };
  vm.createContext(sandbox);
  vm.runInContext(js, sandbox);

  return {
    theme: () => dataset.theme,
    mode: () => dataset.themeMode,
    cache: () => (store["tf-theme"] === undefined ? null : store["tf-theme"]),
    fire: (t, e) => (listeners[t] || []).forEach((fn) => fn(e)),
    hasListener: (t) => !!(listeners[t] || []).length,
  };
}

const cases = [
  {
    name: "服务端深色 + 缓存浅色 → 深色（旧值不能压过设置）",
    opt: { serverMode: "dark", cache: "light" },
    want: "dark",
  },
  {
    name: "服务端浅色 + 缓存深色 → 浅色",
    opt: { serverMode: "light", cache: "dark" },
    want: "light",
  },
  {
    name: "服务端 auto + 系统深色 → 深色",
    opt: { serverMode: "auto", prefersDark: true },
    want: "dark",
  },
  {
    name: "服务端 auto + 系统浅色 → 浅色",
    opt: { serverMode: "auto", prefersDark: false },
    want: "light",
  },
  {
    name: "没有服务端模式（静态页）+ 缓存深色 → 兜底深色",
    opt: { cache: "dark" },
    want: "dark",
  },
  {
    name: "缓存里是垃圾值 → 不采纳",
    opt: { serverMode: "light", cache: "banana" },
    want: "light",
  },
];

let fail = 0;

for (const file of LAYOUTS) {
  const js = extractThemeScript(file);
  const label = path.basename(file);

  for (const c of cases) {
    const r = run(js, c.opt);
    if (r.theme() !== c.want) {
      fail++;
      console.error(`✗ ${label} · ${c.name}：期望 ${c.want}，实际 ${r.theme()}`);
    } else {
      console.log(`✓ ${label} · ${c.name}`);
    }
  }

  // 加载后缓存要被对齐成服务端的值，否则下次 BFCache / 别的标签页还会翻上来
  const aligned = run(js, { serverMode: "dark", cache: "light" });
  if (aligned.cache() !== "dark") {
    fail++;
    console.error(`✗ ${label} · 缓存对齐：期望 dark，实际 ${aligned.cache()}`);
  } else {
    console.log(`✓ ${label} · 缓存对齐到服务端值`);
  }

  // 正常加载（非 BFCache）时 pageshow 不能用缓存覆盖服务端
  const ps = run(js, { serverMode: "dark", cache: "light" });
  ps.fire("pageshow", { persisted: false });
  if (ps.theme() !== "dark") {
    fail++;
    console.error(`✗ ${label} · pageshow(非缓存) 不该覆盖服务端：实际 ${ps.theme()}`);
  } else {
    console.log(`✓ ${label} · pageshow 正常加载不覆盖服务端`);
  }

  // 跨标签切换：storage 事件要把模式同步过去
  const st = run(js, { serverMode: "light" });
  st.fire("storage", { key: "tf-theme", newValue: "dark" });
  if (st.theme() !== "dark") {
    fail++;
    console.error(`✗ ${label} · storage 同步：期望 dark，实际 ${st.theme()}`);
  } else {
    console.log(`✓ ${label} · storage 事件跨标签同步`);
  }
}

if (fail) {
  console.error(`\n${fail} 项失败`);
  process.exit(1);
}
console.log("\n全部通过");

// 阅读页快捷键的行为测试。
//
// 没有浏览器自动化可用（headless 在这台机器上彻底失效），
// 所以把页面里那段脚本抽出来，套一个最小 DOM 桩，直接按键验证去了哪里。
//
//   node scripts/test_reader_keys.js <页面 HTML 文件>
//
// 页面可以用：curl -s "http://127.0.0.1:7317/r/<id>/<ch>?site=csdn&pack=cs" > page.html

const fs = require("fs");
const vm = require("vm");

const file = process.argv[2];
if (!file) {
  console.error("用法: node scripts/test_reader_keys.js <页面 HTML>");
  process.exit(2);
}
const html = fs.readFileSync(file, "utf8");
const blocks = [...html.matchAll(/<script>([\s\S]*?)<\/script>/g)].map((m) => m[1]);
const js = blocks.find((b) => b.includes("boss="));
if (!js) {
  console.error("页面里没找到键盘脚本");
  process.exit(1);
}

// 从页面里取书 id 与章节号，期望值不能写死
const meta = js.match(/book="([^"]+)",ch=(\d+)/);
if (!meta) {
  console.error("脚本里没找到 book / ch");
  process.exit(1);
}
const BOOK = meta[1];
const CH = meta[2];

/**
 * @param anchors 选择器 → href（null 表示页面上没有这个链接）
 * @param key 要按的键
 */
function press(anchors, key, opts = {}) {
  const state = { loc: null, replaced: null, listener: null, stored: {} };
  const prose = { style: {} };
  const els = Object.assign({ ".prose": prose }, anchors);
  const doc = {
    addEventListener: (t, f) => {
      if (t === "keydown") state.listener = f;
    },
    querySelector: (sel) => (sel in els ? els[sel] : null),
    getElementById: () => null,
    createElement: () => ({
      style: {},
      set innerHTML(_v) {},
      appendChild() {},
    }),
    body: { appendChild() {} },
  };
  const location = {
    get href() {
      return state.loc;
    },
    set href(v) {
      state.loc = v;
    },
    replace: (v) => {
      state.replaced = v;
    },
  };
  const localStorage = {
    getItem: (k) => (k === "tf.fs" ? opts.fs || null : null),
    setItem: (k, v) => {
      state.stored[k] = v;
    },
  };
  const sandbox = vm.createContext({ document: doc, location, localStorage, window: {} });
  vm.runInContext(js, sandbox);
  if (!state.listener) throw new Error("脚本没有注册 keydown");
  state.listener({
    key,
    ctrlKey: !!opts.ctrl,
    altKey: !!opts.alt,
    metaKey: !!opts.meta,
    preventDefault() {},
    target: { tagName: opts.tag || "BODY" },
  });
  return { state, prose };
}

let pass = 0;
let fail = 0;
function check(name, got, want) {
  const ok = got === want;
  if (ok) pass++;
  else fail++;
  console.log(`${ok ? "  ok  " : "  FAIL"} ${name}${ok ? "" : `  期望 ${want}，实际 ${got}`}`);
}

// 1. 页内还有下一页：→ 应该走分页条
let r = press(
  {
    '[data-nav="page-next"]': {
      getAttribute: (n) => (n === "href" ? "/r/b/3?p=3" : null),
    },
    '[data-nav="next"]': { getAttribute: () => "/r/b/4" },
  },
  "ArrowRight",
);
check("→ 优先翻页", r.state.loc, "/r/b/3?p=3");

// 2. 已经是本章最后一页：→ 应该跨到下一章
r = press(
  {
    '[data-nav="page-next"]': null,
    '[data-nav="next"]': { getAttribute: () => "/r/b/4" },
  },
  "ArrowRight",
);
check("→ 页尾跨章", r.state.loc, "/r/b/4");

// 3. 全书最后一页：→ 什么都不做
r = press({ '[data-nav="page-next"]': null, '[data-nav="next"]': null }, "ArrowRight");
check("→ 无路可走时不动", r.state.loc, null);

// 4. ← 同理
r = press({ '[data-nav="page-prev"]': { getAttribute: () => "/r/b/3?p=1" } }, "ArrowLeft");
check("← 翻回上一页", r.state.loc, "/r/b/3?p=1");

// 5. S 换下一个皮肤（csdn 之后是 juejin）
r = press({}, "s");
check("S 换皮肤", r.state.loc, `/r/${BOOK}/${CH}?site=juejin&pack=cs`);

// 6. Shift+S 反向（csdn 往前一个应该绕到最后一位 iphy）
r = press({}, "S");
check("Shift+S 反向换皮肤", r.state.loc, `/r/${BOOK}/${CH}?site=iphy&pack=cs`);

// 7. P 换主题包（cs 之后是 crypto）
r = press({}, "p");
check("P 换主题包", r.state.loc, `/r/${BOOK}/${CH}?site=csdn&pack=crypto`);

// 8. Esc 永远生效，且不管焦点在哪
r = press({}, "Escape", { tag: "INPUT" });
check("Esc 在输入框里也生效", r.state.replaced, "about:blank");

// 9. 带修饰键的组合不抢（Ctrl+O 之类）
r = press({}, "s", { ctrl: true });
check("Ctrl+组合键不抢", r.state.loc, null);

// 10. 字号 +
r = press({}, "+");
check("+ 放大正文字号", r.prose.style.fontSize, "17px");
check("+ 记住字号", r.state.stored["tf.fs"], 17);

// 11. 下限：已经 13px 时再减还是 13
r = press({}, "-", { fs: "13" });
check("- 不小于 13px", r.prose.style.fontSize, "13px");

console.log(`\n通过 ${pass} 项，失败 ${fail} 项`);
process.exit(fail ? 1 : 0);

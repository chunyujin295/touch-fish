//! 渲染层：把 Book + 站点皮肤 + 主题包 → 一张完整的伪装网页。
//!
//! 关键约束：
//! 1. 服务端直出 HTML（不靠前端 JS 拼内容），页面在浏览器看来就是一张真实网页；
//! 2. 所有噪声由 seed 决定，同一本书同一章刷新多少次都一样；
//! 3. 皮肤只决定"长什么样"，主题包只决定"聊什么"，两者改动互不影响。

use std::collections::HashMap;

use serde_json::Value;

use super::asset_text;
use crate::model::{Block, Book, Inline};
use crate::noise::{seed_of, Rng};

pub fn page(
    book: &Book,
    idx: usize,
    site_id: &str,
    pack_id: &str,
    boss_url: &str,
    page_no: usize,
    reader_keys: bool,
) -> String {
    let sites = load("data/sites.json");
    let packs = load("data/packs.json");

    let site = find(&sites, site_id).or_else(|| sites.first()).cloned().unwrap_or_else(|| serde_json::json!({}));
    let pack = find(&packs, pack_id).or_else(|| packs.first()).cloned().unwrap_or_else(|| serde_json::json!({}));

    let layout = {
        let l = s(&site, "layout");
        if l.is_empty() { "blog".to_string() } else { l.to_string() }
    };
    let tpl = {
        let t = asset_text(&format!("reader/layout-{layout}.html"));
        if t.is_empty() {
            "<html><body style=\"font:16px/1.8 system-ui;padding:40px\">排版模板缺失</body></html>".to_string()
        } else {
            t
        }
    };

    // ---- CSS：基础 + 版式 + 站点 ----
    let mut css = String::new();
    css.push_str(&asset_text("reader/base.css"));
    css.push_str(&asset_text(&format!("reader/{layout}.css")));
    let sid = s(&site, "id").to_string();
    css.push_str(&asset_text("sites/all.css"));

    // ---- 噪声种子：书 + 章 + 皮肤 + 主题包 ----
    let seed = seed_of(&[
        &book.id,
        &idx.to_string(),
        &sid,
        s(&pack, "id"),
        &book.chapters.len().to_string(),
    ]);
    let mut rng = Rng::new(seed);

    let chapter = book.chapters.get(idx);
    let chapter_title = chapter.map(|c| c.title.clone()).unwrap_or_default();
    let blocks = chapter.map(|c| c.blocks.clone()).unwrap_or_default();

    // ---- 正文 + 目录 ----
    // 分页伪装：CSDN / 博客园的长文本来就是一页一页翻的，sites.json 里 paging 决定每页多少块
    let per = site.get("paging").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let ranges = paginate(&blocks, per);
    let total_pages = ranges.len().max(1);
    let cur = page_no.clamp(1, total_pages);
    let (b0, b1) = ranges.get(cur - 1).copied().unwrap_or((0, blocks.len()));
    let page_blocks = &blocks[b0..b1];

    let mut toc: Vec<(String, String)> = Vec::new();
    let body = body_html(page_blocks, &chapter_title, &mut toc);

    // ---- 元信息 ----
    let author = pick_author(&pack, &mut rng, &book.author);
    // 阅读量必须跟篇幅挂钩：一段两百字的短帖配八千阅读，比任何排版破绽都显眼
    let words: usize = blocks
        .iter()
        .map(|b| match b {
            Block::P { x, .. } | Block::H { x, .. } | Block::Q { x } => x.chars().count(),
            Block::List { items, .. } => items.iter().map(|i| i.chars().count()).sum(),
            Block::Code { x, .. } => x.chars().count() / 3,
            _ => 0,
        })
        .sum();
    let read = ((words * rng.range(12, 34)) / 10).clamp(160, 480_000);
    let like = (read / rng.range(18, 60)).max(3);
    // 收藏一定少于点赞，不能出现两个数字一模一样这种破绽
    let star = (like / rng.range(3, 7)).max(1);
    let art_ts = book.added_at + idx as i64 * rng.range(1800, 40000) as i64;
    let time = fmt_ts(art_ts.max(1_760_000_000));

    let tags: Vec<String> = pack
        .get("terms")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    // ---- 侧栏 / 广告 / 评论 ----
    let side = side_html(&pack, &mut rng);
    let ads = ads_html(&pack);
    let comments = comments_html(&pack, &layout, art_ts, &mut rng);

    // ---- 顶部导航 / 面包屑 / 页脚 ----
    let nav = nav_html(&site);
    let crumb = crumb_html(&pack, &site);
    let footer = s(&site, "footer").to_string();
    let host = s(&site, "host").to_string();
    let slug: String = format!("{:x}", rng.next_u64() % 0xffff_ffff);
    let url = s(&site, "url").replace("{slug}", &slug);

    // ---- 标题：按站点包装器处理 ----
    let title = wrap_title(&chapter_title, &pack, &mut rng);

    // ---- 阅读进度条（真实站点都有，顺便当作位置指示） ----
    let total = book.chapters.len().max(1);
    let progress = ((idx + 1) as f64 / total as f64 * 100.0).round();

    // ---- 摘要卡（学术版式用）：不复述正文，写成真实的"摘要体" ----
    let ccount = pack
        .get("comments")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    let abs = abstract_of(&chapter_title, &tags, ccount);

    let ccount_str = ccount.to_string();

    // Confluence 这类站点没有广告位，硬塞反而假
    let no_ads = b(&site, "no_ads");
    let (ad_top, ad_inline) = if no_ads {
        (String::new(), String::new())
    } else {
        ads
    };
    let accent = s(&site, "accent").to_string();

    // ---- 站点细节件：CSDN 的专栏收录条、博客园的 posted @ 行、知乎的赞同 pill ----
    // 模板写在 sites.json 里，渲染层只负责替换——加一个站点仍然不用改 Rust
    let term = tags.first().cloned().unwrap_or_default();
    let posts = rng.range(24, 168); // 专栏里收录了多少篇，跟评论数不是一回事
    // 阅读时长按正文字数算，不能拍脑袋——两行字的帖子标"阅读 8 分钟"更假
    let mins = ((words as f64 / 380.0).round() as usize).max(1);
    let fork = (star / 5).max(7); // star 和 fork 不能同数，比例大概是 5:1
    // GitHub 的仓库名必须是 ASCII——中文仓库名挂在 /{user}/{repo} 上一眼就是假的
    const REPOS: [&str; 6] = [
        "dist-sys-notes",
        "consensus-playground",
        "runtime-lab",
        "core-internals",
        "infra-notes",
        "sys-reading",
    ];
    let repo = REPOS[(rng.next_u64() % REPOS.len() as u64) as usize];
    let detail = |key: &str| -> String {
        let tpl = s(&site, key);
        if tpl.is_empty() {
            return String::new();
        }
        let mut out = tpl.to_string();
        for (k, v) in [
            ("{author}", esc(&author.0)),
            ("{time}", esc(&time)),
            ("{read}", read.to_string()),
            ("{like}", fmt_num(like)),
            ("{star}", fmt_num(star)),
            ("{ccount}", ccount.to_string()),
            ("{posts}", fmt_num(posts)),
            ("{mins}", mins.to_string()),
            ("{fork}", fmt_num(fork)),
            ("{repo}", repo.to_string()),
            ("{time_en}", fmt_ts_en(art_ts.max(1_760_000_000))),
            ("{term}", esc(&term)),
            ("{host}", esc(&host)),
        ] {
            out = out.replace(k, &v);
        }
        out
    };

    let mut m: HashMap<&str, String> = HashMap::new();
    m.insert("TABTITLE", format!("{} - {}", esc(&title), s(&site, "name")));
    m.insert("CSS", css);
    m.insert("ACCENT", accent.clone());
    m.insert("ACCENTHEX", accent.trim_start_matches('#').to_string());
    m.insert("INITIAL", initial(s(&site, "name")));
    m.insert("SITE", esc(s(&site, "name")));
    m.insert("SITETAG", esc(s(&site, "tag")));
    m.insert("NAV", nav);
    m.insert("SEARCH", esc(&format!("搜索 {}", s(&site, "name"))));
    m.insert("HDCLASS", if b(&site, "dark") { "dk".into() } else { String::new() });
    m.insert("NOTICE", esc(s(&site, "notice")));
    m.insert("CRUMB", crumb);
    m.insert("TITLE", esc(&title));
    m.insert("AUTHOR", esc(&author.0));
    m.insert("AUTHOR_INITIAL", initial(&author.0));
    m.insert("ROLE", esc(&author.1));
    m.insert("TIME", time.clone());
    m.insert("TIME_REL", rel_time(art_ts.max(1_760_000_000)));
    m.insert("READ", fmt_num(read));
    m.insert("LIKE", fmt_num(like));
    m.insert("STAR", fmt_num(star));
    m.insert("BODY", body);
    m.insert("DETAIL_TOP", detail("detail_top"));
    m.insert("DETAIL_BOTTOM", detail("detail_bottom"));
    m.insert("TAGS", tags_html(&tags));
    m.insert("AD_TOP", ad_top);
    m.insert("AD_INLINE", ad_inline);
    m.insert("SIDE", side);
    m.insert("COMMENTS", comments);
    m.insert("CCOUNT", ccount_str);
    m.insert("ABS_BG", esc(&abs.0));
    m.insert("ABS_ME", esc(&abs.1));
    m.insert("ABS_RS", esc(&abs.2));
    m.insert("KEYWORDS", tags.join(" / "));
    m.insert(
        "PAGER",
        format!(
            "{}{}",
            if total_pages > 1 {
                page_bar(&book.id, idx, &sid, s(&pack, "id"), cur, total_pages)
            } else {
                String::new()
            },
            pager(&book, idx, &sid, s(&pack, "id"))
        ),
    );
    m.insert("FOOTER", esc(&footer));
    m.insert("HOST", esc(&host));
    m.insert("URL", esc(&url));
    m.insert("PROGRESS", format!("{progress:.1}"));
    // 快捷键要能在皮肤 / 主题包之间循环，所以把两份 id 表带进页面
    let site_ids: Vec<String> = sites
        .iter()
        .filter_map(|v| v.get("id").and_then(|x| x.as_str()).map(|x| x.to_string()))
        .collect();
    let pack_ids: Vec<String> = packs
        .iter()
        .filter_map(|v| v.get("id").and_then(|x| x.as_str()).map(|x| x.to_string()))
        .collect();
    m.insert(
        "BOSS",
        keys_script(
            boss_url,
            &book.id,
            idx,
            &sid,
            s(&pack, "id"),
            &site_ids,
            &pack_ids,
            reader_keys,
        ),
    );
    m.insert("LIVE", live_script(read, like, star));
    m.insert("BODYCLASS", format!("ly-{layout} s-{sid}"));

    fill(&tpl, &m)
}

/* ================= 数据加载 ================= */

fn load(name: &str) -> Vec<Value> {
    serde_json::from_str::<Value>(&asset_text(name))
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
}

fn find<'a>(list: &'a [Value], id: &str) -> Option<&'a Value> {
    list.iter()
        .find(|v| v.get("id").and_then(|x| x.as_str()) == Some(id))
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}

fn b(v: &Value, k: &str) -> bool {
    v.get(k).and_then(|x| x.as_bool()).unwrap_or(false)
}

/* ================= 片段拼装 ================= */

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// 段落的行内片段 → HTML。
/// 只有 http(s) 的链接才渲染成 <a>：电子书里绝大多数内链是 `../Text/ch03.xhtml` 这种，
/// 在假站点上点过去是 404，比不做链接更露馅。
fn inline_html(runs: &[Inline]) -> String {
    let mut out = String::new();
    for r in runs {
        match r {
            Inline::Text(t) => out.push_str(&esc(t)),
            Inline::Strong(t) => out.push_str(&format!("<b>{}</b>", esc(t))),
            Inline::Em(t) => out.push_str(&format!("<i>{}</i>", esc(t))),
            Inline::Code(t) => out.push_str(&format!("<code>{}</code>", esc(t))),
            Inline::Link { href, text } => {
                let h = href.trim();
                if h.starts_with("http://") || h.starts_with("https://") {
                    out.push_str(&format!(
                        "<a href=\"{}\" target=\"_blank\" rel=\"noopener noreferrer\">{}</a>",
                        esc(h),
                        esc(text)
                    ));
                } else {
                    out.push_str(&esc(text));
                }
            }
        }
    }
    out
}

fn initial(name: &str) -> String {
    name.chars()
        .next()
        .map(|c| esc(&c.to_string()))
        .unwrap_or_default()
}

fn fmt_num(n: usize) -> String {
    if n >= 10_000 {
        format!("{:.1}w", n as f64 / 10_000.0)
    } else if n >= 1_000 {
        let v = n as f64 / 1000.0;
        if v.fract() == 0.0 {
            format!("{v:.0}k")
        } else {
            format!("{v:.1}k")
        }
    } else {
        n.to_string()
    }
}

fn nav_html(site: &Value) -> String {
    let on = site.get("nav_on").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
    site.get("nav")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .enumerate()
                .filter_map(|(i, x)| x.as_str().map(|t| (i, t)))
                .map(|(i, t)| {
                    let cls = if i == on { " class=\"on\"" } else { "" };
                    format!("<a href=\"#\"{cls}>{}</a>", esc(t))
                })
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

fn crumb_html(pack: &Value, site: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(a) = pack.get("crumb").and_then(|v| v.as_array()) {
        for x in a.iter().filter_map(|x| x.as_str()) {
            parts.push(format!("<span>{}</span>", esc(x)));
        }
    }
    if parts.is_empty() {
        parts.push(format!("<span>{}</span>", esc(s(site, "tag"))));
    }
    parts.join("<span class=\"sep\">›</span>")
}

/// 按字符（不是字节）截断，中文标题不能被切成乱码
fn clamp_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

fn wrap_title(raw: &str, pack: &Value, rng: &mut Rng) -> String {
    // 真实站点的帖子标题很少超过 40 字，太长的章节名先收一收再套包装器
    let t = clamp_chars(raw.trim(), 30);
    let t = t.as_str();
    if t.is_empty() {
        return "无标题".into();
    }
    // 标题包装器：同一个章节标题，在不同主题包里换个体裁
    if let Some(a) = pack.get("title_wrap").and_then(|v| v.as_array()) {
        if !a.is_empty() {
            let tpl = a[(rng.next_u64() % a.len() as u64) as usize]
                .as_str()
                .unwrap_or("{t}");
            return tpl.replace("{t}", t);
        }
    }
    t.to_string()
}

fn tags_html(tags: &[String]) -> String {
    tags.iter()
        .map(|t| format!("<a href=\"#\"># {}</a>", esc(t)))
        .collect::<Vec<_>>()
        .join("")
}

fn pick_author(pack: &Value, rng: &mut Rng, book_author: &str) -> (String, String) {
    if let Some(a) = pack.get("authors").and_then(|v| v.as_array()) {
        if !a.is_empty() {
            let it = &a[(rng.next_u64() % a.len() as u64) as usize];
            let name = it.get("name").and_then(|v| v.as_str()).unwrap_or("匿名");
            let role = it.get("role").and_then(|v| v.as_str()).unwrap_or("");
            return (name.to_string(), role.to_string());
        }
    }
    (
        if book_author.trim().is_empty() {
            "匿名用户".into()
        } else {
            book_author.to_string()
        },
        String::new(),
    )
}

fn side_html(pack: &Value, rng: &mut Rng) -> String {
    let Some(mods) = pack.get("sidebar").and_then(|v| v.as_array()) else {
        return String::new();
    };
    let mut out = String::new();
    for (i, m) in mods.iter().enumerate() {
        if i > 2 {
            break;
        }
        let h = esc(s(m, "h"));
        let rank = b(m, "rank");
        let mut inner = String::new();
        if let Some(tags) = m.get("tags").and_then(|v| v.as_array()) {
            inner.push_str("<div class=\"tagp\">");
            for t in tags.iter().filter_map(|x| x.as_str()) {
                inner.push_str(&format!("<a href=\"#\">{}</a>", esc(t)));
            }
            inner.push_str("</div>");
        } else if let Some(items) = m.get("items").and_then(|v| v.as_array()) {
            for (j, it) in items.iter().enumerate() {
                let (t, meta) = match it {
                    Value::Array(a) => (
                        a.first().and_then(|x| x.as_str()).unwrap_or(""),
                        a.get(1).and_then(|x| x.as_str()).unwrap_or(""),
                    ),
                    Value::String(sv) => (sv.as_str(), ""),
                    _ => ("", ""),
                };
                let mark = if rank {
                    format!(
                        "<span class=\"n{}\">{}</span>",
                        if j < 3 { " t" } else { "" },
                        j + 1
                    )
                } else {
                    String::new()
                };
                let em = if meta.is_empty() {
                    String::new()
                } else {
                    format!("<span class=\"em\">{}</span>", esc(meta))
                };
                inner.push_str(&format!(
                    "<a class=\"item\" href=\"#\">{mark}<span class=\"tx\">{}</span>{em}</a>",
                    esc(t)
                ));
            }
        }
        out.push_str(&format!(
            "<div class=\"mod\"><div class=\"mod-h\">{h}</div><div class=\"mod-b\">{inner}</div></div>"
        ));
        let _ = rng;
    }
    out
}

fn ads_html(pack: &Value) -> (String, String) {
    let empty = || serde_json::json!({"t":"","d":"","cta":""});
    let ads = pack
        .get("ads")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let a0 = ads.first().cloned().unwrap_or_else(empty);
    let a1 = ads.get(1).cloned().unwrap_or_else(empty);

    let top = format!(
        "<div class=\"ad hl\"><div class=\"adt\">{}</div><div class=\"add\">{}</div><div class=\"adc\">{} ›</div></div>",
        esc(s(&a0, "t")),
        esc(s(&a0, "d")),
        esc(s(&a0, "cta"))
    );
    if s(&a1, "t").is_empty() {
        return (top, String::new());
    }
    let inline = format!(
        "<div class=\"ad-inline\"><div class=\"ic\"></div><div class=\"ax\"><div class=\"adt\">{}</div><div class=\"add\">{}</div></div><div class=\"adc\">{} ›</div></div>",
        esc(s(&a1, "t")),
        esc(s(&a1, "d")),
        esc(s(&a1, "cta"))
    );
    (top, inline)
}

fn comments_html(pack: &Value, layout: &str, art_ts: i64, rng: &mut Rng) -> String {
    let list = pack
        .get("comments")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // 评论时间比正文晚几分钟到几小时
    let mut out = String::new();
    let mut ts = art_ts;
    for (i, c) in list.iter().enumerate() {
        ts += rng.range(180, 9000) as i64;
        let u = s(c, "u");
        let role = s(c, "role");
        let x = s(c, "x");
        let lk = c.get("lk").and_then(|v| v.as_u64()).unwrap_or(0);
        if layout == "forum" {
            let ro = if role.is_empty() {
                String::new()
            } else {
                format!("<span class=\"ro\">{}</span>", esc(role))
            };
            out.push_str(&format!(
                "<div class=\"fl\"><div class=\"lf\"><div class=\"av\">{}</div><div class=\"no\">#{}</div></div>\
                 <div class=\"rb\"><div class=\"rh\"><b>{}</b>{}<span>{}</span><span class=\"rp\">举报 · 回复</span></div>\
                 <div class=\"rx\">{}</div>\
                 <div class=\"rf\"><span>👍 {}</span><span>引用回复</span><span>评分</span></div></div></div>",
                initial(u),
                i + 1,
                esc(u),
                ro,
                fmt_ts(ts),
                esc(x),
                lk
            ));
        } else {
            let ro = if role.is_empty() {
                String::new()
            } else {
                format!("<span>{}</span>", esc(role))
            };
            out.push_str(&format!(
                "<div class=\"cmt\"><div class=\"av\">{}</div><div class=\"cb\">\
                 <div class=\"ch\"><b>{}</b>{}<span>{}</span><span class=\"lk\">赞 {}</span></div>\
                 <p>{}</p></div></div>",
                initial(u),
                esc(u),
                ro,
                fmt_ts(ts),
                lk,
                esc(x)
            ));
        }
    }
    out
}

fn pager(book: &Book, idx: usize, site: &str, pack: &str) -> String {
    let total = book.chapters.len();
    let link = |i: usize, label: &str, nav: &str| {
        format!(
            "<a class=\"pg\" data-nav=\"{nav}\" href=\"/r/{}/{i}?site={site}&pack={pack}\">{label}</a>",
            book.id
        )
    };
    let mut out = String::new();
    out.push_str("<div class=\"pager\">");
    if idx > 0 {
        out.push_str(&link(idx - 1, "‹ 上一篇", "prev"));
    } else {
        out.push_str("<span class=\"pg off\">‹ 上一篇</span>");
    }
    out.push_str("<span class=\"pg-info\">");
    out.push_str(&format!("{:02} / {:02}", idx + 1, total));
    out.push_str("</span>");
    if idx + 1 < total {
        out.push_str(&link(idx + 1, "下一篇 ›", "next"));
    } else {
        out.push_str("<span class=\"pg off\">下一篇 ›</span>");
    }
    out.push_str("</div>");
    out
}

/* ================= 正文 ================= */

/// 把整章切成每页 per 个块。per = 0 表示不分页。
/// 页尾如果正好压在标题上，就把标题推到下一页——真实分页也极少让标题孤零零留在页底。
fn paginate(blocks: &[Block], per: usize) -> Vec<(usize, usize)> {
    if per == 0 || blocks.len() <= per {
        return vec![(0, blocks.len())];
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < blocks.len() {
        let mut j = (i + per).min(blocks.len());
        while j < blocks.len() && j > i + 1 && matches!(blocks[j - 1], Block::H { .. }) {
            j -= 1;
        }
        out.push((i, j));
        i = j;
    }
    out
}

/// 文章分页条：CSDN 式的「1 2 3 … 下一页」。
/// 页数多的时候只显示当前页附近的窗口，不然一排 40 个数字不像话。
fn page_bar(book_id: &str, idx: usize, site: &str, pack: &str, cur: usize, total: usize) -> String {
    let link = |p: usize, label: &str, cls: &str| {
        format!(
            "<a class=\"{cls}\" href=\"/r/{book_id}/{idx}?site={site}&pack={pack}&p={p}\">{label}</a>"
        )
    };
    let link_nav = |p: usize, label: &str, nav: &str| {
        format!(
            "<a class=\"pn\" data-nav=\"{nav}\" href=\"/r/{book_id}/{idx}?site={site}&pack={pack}&p={p}\">{label}</a>"
        )
    };
    let mut out = String::from("<div class=\"pgbar\">");
    out.push_str(&format!(
        "<span class=\"lbl\">第 {cur} / {total} 页</span>"
    ));
    if cur > 1 {
        out.push_str(&link_nav(cur - 1, "‹ 上一页", "page-prev"));
    } else {
        out.push_str("<span class=\"pn off\">‹ 上一页</span>");
    }
    out.push_str("<span class=\"nums\">");
    let from = cur.saturating_sub(2).max(1);
    let to = (from + 4).min(total);
    if from > 1 {
        out.push_str(&link(1, "1", ""));
        if from > 2 {
            out.push_str("<i>…</i>");
        }
    }
    for p in from..=to {
        if p == cur {
            out.push_str(&format!("<b>{p}</b>"));
        } else {
            out.push_str(&link(p, &p.to_string(), ""));
        }
    }
    if to < total {
        if to + 1 < total {
            out.push_str("<i>…</i>");
        }
        out.push_str(&link(total, &total.to_string(), ""));
    }
    out.push_str("</span>");
    if cur < total {
        out.push_str(&link_nav(cur + 1, "下一页 ›", "page-next"));
    } else {
        out.push_str("<span class=\"pn off\">下一页 ›</span>");
    }
    out.push_str("</div>");
    out
}

fn body_html(blocks: &[Block], chapter_title: &str, toc: &mut Vec<(String, String)>) -> String {
    let mut out = String::new();
    let mut hcount = 0usize;
    let mut skipped_lead = false;

    for blk in blocks {
        match blk {
            Block::P { x, inl } => {
                if inl.is_empty() {
                    out.push_str(&format!("<p>{}</p>", esc(x)));
                } else {
                    out.push_str(&format!("<p>{}</p>", inline_html(inl)));
                }
            }
            Block::H { level, x } => {
                // 章首那个与章节名重复的标题不再重复渲染
                if !skipped_lead && squash_eq(x, chapter_title) && hcount == 0 {
                    skipped_lead = true;
                    continue;
                }
                skipped_lead = true;
                hcount += 1;
                let id = format!("s{hcount}");
                toc.push((id.clone(), x.clone()));
                let lv = if *level <= 2 { 2 } else { 3 };
                out.push_str(&format!(
                    "<h{lv} id=\"{id}\">{}</h{lv}>",
                    esc(x)
                ));
            }
            Block::Q { x } => {
                out.push_str(&format!("<blockquote class=\"q\">{}</blockquote>", esc(x)));
            }
            Block::Code { x, .. } => {
                out.push_str(&format!(
                    "<div class=\"code\"><div class=\"code-h\"><span class=\"l\">text</span><span>复制</span><span class=\"cp\">···</span></div><pre>{}</pre></div>",
                    esc(x)
                ));
            }
            Block::List { ordered, items } => {
                let tag = if *ordered { "ol" } else { "ul" };
                out.push_str(&format!("<{tag}>"));
                for it in items {
                    out.push_str(&format!("<li>{}</li>", esc(it)));
                }
                out.push_str(&format!("</{tag}>"));
            }
            Block::Img { src, alt } => {
                out.push_str(&format!(
                    "<figure class=\"fig\"><img src=\"{}\" alt=\"{}\" loading=\"lazy\"></figure>",
                    esc(src),
                    esc(alt)
                ));
            }
            Block::Table { head, rows } => {
                out.push_str("<table class=\"tbl\">");
                if !head.is_empty() {
                    out.push_str("<thead><tr>");
                    for h in head {
                        out.push_str(&format!("<th>{}</th>", esc(h)));
                    }
                    out.push_str("</tr></thead>");
                }
                out.push_str("<tbody>");
                for r in rows {
                    out.push_str("<tr>");
                    for c in r {
                        out.push_str(&format!("<td>{}</td>", esc(c)));
                    }
                    out.push_str("</tr>");
                }
                out.push_str("</tbody></table>");
            }
            Block::Hr => out.push_str("<hr class=\"hr\">"),
        }
    }
    out
}

fn squash_eq(a: &str, b: &str) -> bool {
    let norm = |s: &str| -> String {
        s.chars()
            .filter(|c| !c.is_whitespace() && !"，。、；：？！,. ;:?!".contains(*c))
            .collect()
    };
    let (x, y) = (norm(a), norm(b));
    !x.is_empty() && x == y
}

fn abstract_of(chapter_title: &str, terms: &[String], ccount: usize) -> (String, String, String) {
    let t0 = terms.first().cloned().unwrap_or_else(|| "本章主题".into());
    let t1 = terms.get(1).cloned().unwrap_or_else(|| "相关方法".into());
    (
        format!("围绕「{}」展开，先交代问题背景与观测到的现象。", chapter_title),
        format!("结合{}与{}，逐项排查可能原因，并给出可验证的判断依据。", t0, t1),
        format!("结论与取舍见正文各节；文末讨论区有同行补充意见（{} 条）。", ccount),
    )
}

/* ================= 时间 ================= */

fn fmt_ts(ts: i64) -> String {
    let days = ts.div_euclid(86_400);
    let secs = ts.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        y,
        m,
        d,
        secs / 3600,
        (secs % 3600) / 60
    )
}

/// 英文站（Stack Overflow、GitHub 的 issue 时间）用这套写法，
/// 中文时间戳直接贴在英文界面上是现场最能被一眼看破的地方。
fn fmt_ts_en(ts: i64) -> String {
    const MON: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let days = ts.div_euclid(86_400);
    let secs = ts.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let mon = MON[(m as usize).clamp(1, 12) - 1];
    format!(
        "{} {} '{} at {:02}:{:02}",
        mon,
        d,
        y % 100,
        secs / 3600,
        (secs % 3600) / 60
    )
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

fn rel_time(ts: i64) -> String {
    let now = crate::now();
    let d = (now - ts).max(0);
    if d < 3600 {
        format!("{} 分钟前", (d / 60).max(1))
    } else if d < 86_400 {
        format!("{} 小时前", d / 3600)
    } else if d < 86_400 * 30 {
        format!("{} 天前", d / 86_400)
    } else {
        fmt_ts(ts)[..10].to_string()
    }
}

/* ================= 脚本 ================= */

/// 阅读页的键盘层：Boss Key 之外还有翻页、换皮肤、换主题包、字号、回书库、帮助。
///
/// 两处细节：
/// 1. 跳转目标不挂全局变量，全部内联成局部量——`window.__TF_BOSS__` 这种名字
///    在 view-source 里一眼就露馅，之前踩过。
/// 2. Esc 永远最高优先级，其它键都在开关 `reader_keys` 之后；万一哪天快捷键出问题，
///    退出通道不能跟着一起坏。
#[allow(clippy::too_many_arguments)]
fn keys_script(
    boss_url: &str,
    book_id: &str,
    idx: usize,
    site: &str,
    pack: &str,
    sites: &[String],
    packs: &[String],
    on: bool,
) -> String {
    let target = if boss_url.trim().is_empty() {
        "about:blank".to_string()
    } else {
        js_str(boss_url.trim())
    };
    let sites_json = serde_json::to_string(sites).unwrap_or_else(|_| "[]".into());
    let packs_json = serde_json::to_string(packs).unwrap_or_else(|_| "[]".into());

    const TPL: &str = r#"
(function(){
  var boss="%%TARGET%%",book="%%BOOK%%",ch=%%CH%%,site="%%SITE%%",pack="%%PACK%%";
  var SITES=%%SITES%%,PACKS=%%PACKS%%,ON=%%ON%%;
  function go(sel){
    var a=document.querySelector(sel);
    if(a&&a.getAttribute("href")){location.href=a.getAttribute("href");return true}
    return false;
  }
  function cyc(list,cur,step){
    var i=list.indexOf(cur); if(i<0){i=0}
    return list[(i+step+list.length)%list.length];
  }
  function nav(s,p){location.href="/r/"+book+"/"+ch+"?site="+s+"&pack="+p}
  function fs(d){
    var p=document.querySelector(".prose"); if(!p){return}
    var v=parseFloat(p.style.fontSize)||16;
    v=Math.min(24,Math.max(13,v+d));
    p.style.fontSize=v+"px";
    try{localStorage.setItem("tf.fs",v)}catch(e){}
  }
  function help(){
    var d=document.getElementById("kbd");
    if(d){d.parentNode.removeChild(d);return}
    var rows=[["Esc","立即离开"],["← →","上一页 / 下一页"],["S","换一个皮肤（Shift 反向）"],
              ["P","换一个主题包（Shift 反向）"],["+ -","正文调大 / 调小"],["H","回书库"],["?","收起这块"]];
    var html="<b>快捷键</b>";
    for(var i=0;i<rows.length;i++){html+="<div class='kr'><i>"+rows[i][0]+"</i><span>"+rows[i][1]+"</span></div>"}
    html+="<em>这些键只在阅读页生效，不影响浏览器自带的组合键</em>";
    d=document.createElement("div"); d.id="kbd"; d.innerHTML=html;
    document.body.appendChild(d);
  }
  document.addEventListener("keydown",function(e){
    if(e.key==="Escape"){e.preventDefault();location.replace(boss);return}
    if(!ON){return}
    if(e.ctrlKey||e.altKey||e.metaKey){return}
    var t=e.target;
    if(t&&(t.tagName==="INPUT"||t.tagName==="TEXTAREA"||t.isContentEditable)){return}
    switch(e.key){
      case "ArrowRight":
        if(go('[data-nav="page-next"]')||go('[data-nav="next"]')){e.preventDefault()}
        break;
      case "ArrowLeft":
        if(go('[data-nav="page-prev"]')||go('[data-nav="prev"]')){e.preventDefault()}
        break;
      case "h": case "H": location.href="/"; break;
      case "s": nav(cyc(SITES,site,1),pack); break;
      case "S": nav(cyc(SITES,site,-1),pack); break;
      case "p": nav(site,cyc(PACKS,pack,1)); break;
      case "P": nav(site,cyc(PACKS,pack,-1)); break;
      case "+": case "=": fs(1); break;
      case "-": case "_": fs(-1); break;
      case "?": help(); break;
    }
  });
  try{
    var f=localStorage.getItem("tf.fs");
    if(f){var p=document.querySelector(".prose"); if(p){p.style.fontSize=f+"px"}}
  }catch(e){}
})();
"#;

    TPL.replace("%%TARGET%%", &target)
        .replace("%%BOOK%%", &js_str(book_id))
        .replace("%%CH%%", &idx.to_string())
        .replace("%%SITE%%", &js_str(site))
        .replace("%%PACK%%", &js_str(pack))
        .replace("%%SITES%%", &sites_json)
        .replace("%%PACKS%%", &packs_json)
        .replace("%%ON%%", if on { "true" } else { "false" })
}

/// 把一个字符串安全地塞进 JS 双引号字面量里
fn js_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '<' => out.push_str("\\u003c"), // 防止提前闭合 script
            _ => out.push(c),
        }
    }
    out
}

fn live_script(read: usize, like: usize, star: usize) -> String {
    format!(
        r#"
(function(){{
  var st = {{read:{read}, like:{like}, star:{star}}};
  var raf = null, timer = null;
  function fmt(n){{
    if(n>=10000) return (n/10000).toFixed(1)+"w";
    if(n>=1000){{
      var v=n/1000;
      return (v%1===0? v.toFixed(0): v.toFixed(1))+"k";
    }}
    return String(n);
  }}
  function paint(){{
    var r=document.querySelector('[data-tf="read"]');
    if(r) r.textContent = fmt(st.read);
  }}
  function tick(){{
    if(document.hidden) return;
    st.read += Math.floor(Math.random()*2);
    paint();
  }}
  timer = setInterval(tick, 47000);
  document.addEventListener("visibilitychange", function(){{
    if(document.hidden) clearInterval(timer);
    else timer = setInterval(tick, 47000);
  }});
  paint();
}})();
"#
    )
}

/* ================= 模板填充 ================= */

fn fill(tpl: &str, map: &HashMap<&str, String>) -> String {
    let bytes = tpl.as_bytes();
    let mut out = String::with_capacity(tpl.len() + 4096);
    let mut i = 0usize;
    while i < tpl.len() {
        if bytes[i] == b'_' && i + 1 < bytes.len() && bytes[i + 1] == b'_' {
            if let Some(rel) = tpl[i + 2..].find("__") {
                let key = &tpl[i + 2..i + 2 + rel];
                if let Some(v) = map.get(key) {
                    out.push_str(v);
                    i = i + 2 + rel + 2;
                    continue;
                }
            }
        }
        let step = utf8_len(bytes[i]);
        out.push_str(&tpl[i..i + step]);
        i += step;
    }
    out
}

fn utf8_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b >> 5 == 0b110 {
        2
    } else if b >> 4 == 0b1110 {
        3
    } else {
        4
    }
}

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
    appearance: &str,
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

    // 阅读页跟着设置里的外观走：auto 由页面里的 matchMedia 落地。
    // 深色层始终带上——阅读页按 D 可以临时切深色，浅色渲染的页面也要能切过去。
    let theme_mode = match appearance {
        "dark" => "dark",
        "light" => "light",
        _ => "auto",
    };
    css.push_str(&asset_text("reader/dark.css"));
    // 每个皮肤一套自己的暗色，最后加载，覆盖结构层的兜底变量
    css.push_str(&asset_text(&format!("sites/dark/{sid}.css")));

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
    let raw = chapter.map(|c| c.blocks.clone()).unwrap_or_default();
    // 把「一个标题 + 几千个碎段」的小说骨架重排成文章骨架；
    // 原文结构本来就丰富的书会原样返回，不会被揉第二遍
    let blocks = shape_blocks(&raw, &chapter_title, seed);

    // ---- 正文 + 目录 ----
    // 分页伪装：CSDN / 博客园的长文本来就是一页一页翻的，sites.json 里 paging 决定每页多少块
    let per = site.get("paging").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let ranges = paginate(&blocks, per);
    let total_pages = ranges.len().max(1);
    let cur = page_no.clamp(1, total_pages);
    let (b0, b1) = ranges.get(cur - 1).copied().unwrap_or((0, blocks.len()));
    let page_blocks = &blocks[b0..b1];

    let tags: Vec<String> = pack
        .get("terms")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let mut toc: Vec<(String, String)> = Vec::new();
    // 文中广告：跟顶部那张大卡用不同的一条，避免同一页里同一句广告出现两次
    let mid_ads = mid_ads_html(&pack);
    // 正文插图：短文不插图（真实的技术贴也不是每篇都有图），长页一到两张
    let p_page = page_blocks
        .iter()
        .filter(|b| matches!(b, Block::P { .. }))
        .count();
    // 这一页本来就有插图（EPUB 里的 figcaption 图）就别再叠合成的示意图了
    let has_img = page_blocks.iter().any(|b| matches!(b, Block::Img { .. }));
    let kinds = figure_kinds(s(&pack, "id"));
    let figs: Vec<String> = if p_page >= 10 && !has_img {
        let n = if p_page >= 18 { 2 } else { 1 };
        (0..n).map(|i| figure_html(kinds[i], &tags, i + 1)).collect()
    } else {
        Vec::new()
    };
    let icon = topic_icon(s(&pack, "id"));
    let body = body_html(
        page_blocks,
        &chapter_title,
        &mut toc,
        &mid_ads,
        &figs,
        icon,
    );

    // ---- 元信息 ----
    let author = pick_author(&pack, &mut rng, &book.author);
    // 阅读量必须跟篇幅挂钩：一段两百字的短帖配八千阅读，比任何排版破绽都显眼
    let words: usize = blocks.iter().map(block_chars).sum();
    let read = ((words * rng.range(12, 34)) / 10).clamp(160, 480_000);
    let like = (read / rng.range(18, 60)).max(3);
    // 收藏一定少于点赞，不能出现两个数字一模一样这种破绽
    let star = (like / rng.range(3, 7)).max(1);
    let art_ts = book.added_at + idx as i64 * rng.range(1800, 40000) as i64;
    let time = fmt_ts(art_ts.max(1_760_000_000));


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
    m.insert("THEME_MODE", theme_mode.to_string());
    // dark/light 直接落到 HTML 上，禁 JS 也能正确上色；auto 交给片段里的 matchMedia
    m.insert(
        "THEME_VALUE",
        if theme_mode == "auto" {
            String::new()
        } else {
            theme_mode.to_string()
        },
    );
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
        for (i, x) in a.iter().filter_map(|x| x.as_str()).enumerate() {
            if i == 0 {
                // 面包屑第一项就是「首页」：真实站点的这条链能点，点了回书库
                parts.push(format!("<a href=\"/\">{}</a>", esc(x)));
            } else {
                parts.push(format!("<span>{}</span>", esc(x)));
            }
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

/// 文中广告：夹在段落之间的小横条。
/// 顶部那张大卡已经占了 ads[0]，这里从第二条开始取，同一页里不会重复同一句文案。
fn mid_ads_html(pack: &Value) -> Vec<String> {
    let ads = pack
        .get("ads")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    ads.iter()
        .skip(1)
        .filter(|a| !s(a, "t").is_empty())
        .map(|a| {
            format!(
                "<div class=\"ad-mid\"><span class=\"am-tag\">广告</span><div class=\"am-ic\"></div><div class=\"am-x\"><div class=\"adt\">{}</div><div class=\"add\">{}</div></div><a class=\"am-cta\" href=\"#\">{} ›</a></div>",
                esc(s(a, "t")),
                esc(s(a, "d")),
                esc(s(a, "cta"))
            )
        })
        .collect()
}

fn comments_html(pack: &Value, layout: &str, art_ts: i64, rng: &mut Rng) -> String {    let list = pack
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

/// 一块大概多少字。分页和阅读量都用它，避免两套口径。
fn block_chars(b: &Block) -> usize {
    match b {
        Block::P { x, .. } | Block::H { x, .. } | Block::Q { x, .. } | Block::Small { x } => {
            x.chars().count()
        }
        Block::List { items, .. } => items.iter().map(|i| i.chars().count()).sum(),
        Block::Code { x, .. } => x.chars().count(),
        Block::Table { head, rows, .. } => {
            head.iter().map(|c| c.chars().count()).sum::<usize>()
                + rows
                    .iter()
                    .flat_map(|r| r.iter())
                    .map(|c| c.chars().count())
                    .sum::<usize>()
        }
        Block::Img { .. } | Block::Hr => 0,
    }
}

/// 每页大约多少字。真实站点的分页是跟着篇幅走的，不是固定块数：
/// 一页三千字上下，一章一两万字就是六七页，跟 CSDN 上的长文对得上。
const CHARS_PER_PAGE: usize = 3000;

/// 把整章切成每页 per 个块（同时受每页字数约束）。per = 0 表示不分页。
/// 页尾如果正好压在标题上，就把标题推到下一页——真实分页也极少让标题孤零零留在页底。
fn paginate(blocks: &[Block], per: usize) -> Vec<(usize, usize)> {
    if per == 0 || blocks.len() <= per {
        return vec![(0, blocks.len())];
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < blocks.len() {
        let mut j = i;
        let mut chars = 0usize;
        // 先吃块数上限，再看字数够不够一页
        while j < blocks.len() && j - i < per {
            chars += block_chars(&blocks[j]);
            j += 1;
            if chars >= CHARS_PER_PAGE && j - i >= 3 {
                break;
            }
        }
        // 页尾压在标题上时把标题推到下一页
        while j < blocks.len() && j > i + 1 && matches!(blocks[j - 1], Block::H { .. }) {
            j -= 1;
        }
        if j <= i {
            j = (i + 1).min(blocks.len());
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


/* ================= 正文插图 ================= */

/// 主题包的小图标：放在小标题前面的 15px 线条图。
/// 用 currentColor，颜色跟着标题走，深色不用单独写一遍。
fn topic_icon(pack_id: &str) -> &'static str {
    match pack_id {
        "crypto" => r##"<svg class="hico" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="7" width="10" height="6.5" rx="1.6"/><path d="M5.4 7V5.2a2.6 2.6 0 0 1 5.2 0V7"/></svg>"##,
        "infosec" => r##"<svg class="hico" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M8 2l5 1.9v4.6c0 3-2.1 4.7-5 5.5-2.9-.8-5-2.5-5-5.5V3.9L8 2z"/><path d="M5.9 8.1l1.5 1.5 2.8-3"/></svg>"##,
        "med" => r##"<svg class="hico" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M1.8 9h3l1.4-4.2 2.8 8.4L11 9h3.2"/></svg>"##,
        "edu" => r##"<svg class="hico" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M8 4.3C6.6 3.3 4.8 2.9 3 3.1v8.8c1.8-.2 3.6.2 5 1.2 1.4-1 3.2-1.4 5-1.2V3.1c-1.8-.2-3.6.2-5 1.2z"/><path d="M8 4.3v8.8"/></svg>"##,
        "bio" => r##"<svg class="hico" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M5 2c0 4.2 6 5.8 6 10"/><path d="M11 2c0 4.2-6 5.8-6 10"/><path d="M5.7 5h4.6M5.7 9h4.6"/></svg>"##,
        "math" => r##"<svg class="hico" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M2.2 2v11.8H14"/><path d="M3.6 11.4c2.2 0 2.6-7 4.8-7s2 7 4.2 7"/></svg>"##,
        "phys" => r##"<svg class="hico" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M1.5 8c1.4 0 1.4-4.6 2.8-4.6S5.7 12.6 7.1 12.6 8.5 8 9.9 8s1.4 4.6 2.8 4.6"/></svg>"##,
        _ => r##"<svg class="hico" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><circle cx="3.4" cy="8" r="1.9"/><circle cx="12.4" cy="3.6" r="1.9"/><circle cx="12.4" cy="12.4" r="1.9"/><path d="M5.2 8l5.4-3.2M5.2 8l5.4 3.2"/></svg>"##,
    }
}

/// 不同主题包配不同的示意图：后端看架构和时序，医学看曲线，安全看分层。
fn figure_kinds(pack_id: &str) -> [u8; 2] {
    match pack_id {
        "crypto" => [1, 3],
        "infosec" => [3, 0],
        "med" => [2, 3],
        "edu" => [3, 0],
        "bio" => [2, 3],
        "math" => [2, 0],
        "phys" => [2, 3],
        _ => [0, 1],
    }
}

/// 四张示意图，按主题包的术语填标签。
/// 全内联 SVG：不联网、不占体积，颜色全走 CSS 变量，深色模式不用另画一套。
fn figure_svg(kind: u8, terms: &[String]) -> String {
    let t = |i: usize| terms.get(i).cloned().unwrap_or_else(|| "服务".to_string());
    match kind {
        0 => {
            let (a, b, c) = (esc(&t(0)), esc(&t(1)), esc(&t(2)));
            format!(
                r##"<svg class="figsvg" viewBox="0 0 640 300" role="img" aria-label="调用链路示意图">
<rect class="pl" x="24" y="120" width="112" height="60" rx="7"/><text class="tx" x="80" y="155" text-anchor="middle">客户端</text>
<rect class="pl" x="182" y="120" width="112" height="60" rx="7"/><text class="tx" x="238" y="155" text-anchor="middle">接入网关</text>
<rect class="hl" x="340" y="46" width="128" height="52" rx="7"/><text class="tx" x="404" y="77" text-anchor="middle">{a}</text>
<rect class="hl" x="340" y="124" width="128" height="52" rx="7"/><text class="tx" x="404" y="155" text-anchor="middle">{b}</text>
<rect class="hl" x="340" y="202" width="128" height="52" rx="7"/><text class="tx" x="404" y="233" text-anchor="middle">{c}</text>
<rect class="pl" x="510" y="120" width="106" height="60" rx="7"/><text class="tx" x="563" y="155" text-anchor="middle">存储</text>
<path class="ln" d="M136 150h34m-6-4l6 4-6 4"/>
<path class="ln" d="M294 150h20V72h16m-6-4l6 4-6 4"/>
<path class="ln" d="M294 150h34m-6-4l6 4-6 4"/>
<path class="ln" d="M294 150h20v78h16m-6-4l6 4-6 4"/>
<path class="ln" d="M468 72h26v78h8m-6-4l6 4-6 4"/>
<path class="ln" d="M468 150h32m-6-4l6 4-6 4"/>
<path class="ln" d="M468 228h26V150h8m-6-4l6 4-6 4"/>
</svg>"##
            )
        }
        1 => {
            let (a, b) = (esc(&t(0)), esc(&t(1)));
            format!(
                r##"<svg class="figsvg" viewBox="0 0 640 300" role="img" aria-label="请求时序示意图">
<rect class="pl" x="40" y="18" width="82" height="30" rx="6"/><text class="tx" x="81" y="38" text-anchor="middle">客户端</text>
<rect class="pl" x="196" y="18" width="82" height="30" rx="6"/><text class="tx" x="237" y="38" text-anchor="middle">网关</text>
<rect class="hl" x="352" y="18" width="92" height="30" rx="6"/><text class="tx" x="398" y="38" text-anchor="middle">{a}</text>
<rect class="pl" x="516" y="18" width="72" height="30" rx="6"/><text class="tx" x="552" y="38" text-anchor="middle">存储</text>
<path class="gd" d="M81 48v232M237 48v232M398 48v232M552 48v232"/>
<path class="ln" d="M81 86h150m-6-4l6 4-6 4"/><text class="t2" x="90" y="80">发起请求</text>
<path class="ln" d="M237 128h155m-6-4l6 4-6 4"/><text class="t2" x="246" y="122">鉴权 / 路由</text>
<path class="ln" d="M398 170h148m-6-4l6 4-6 4"/><text class="t2" x="407" y="164">{b}</text>
<path class="ln" d="M552 212H398m6-4l-6 4 6 4"/><text class="t2" x="407" y="206">返回结果</text>
<path class="ln" d="M398 254H81m6-4l-6 4 6 4"/><text class="t2" x="90" y="248">响应客户端</text>
</svg>"##
            )
        }
        2 => {
            format!(
                r##"<svg class="figsvg" viewBox="0 0 640 300" role="img" aria-label="指标随并发变化">
<path class="ax" d="M64 34v216h536"/>
<path class="gd" d="M64 76h536M64 119h536M64 162h536M64 205h536"/>
<text class="t2" x="58" y="38" text-anchor="end">高</text>
<text class="t2" x="58" y="250" text-anchor="end">低</text>
<text class="t2" x="64" y="274">低</text>
<text class="t2" x="576" y="274">高</text>
<polyline class="cv" points="64,206 135,196 206,181 277,168 348,150 419,140 490,118 561,96 600,86 600,250 64,250"/>
<polyline class="cv2" points="64,232 135,228 206,222 277,214 348,206 419,196 490,181 561,150 600,138 600,250 64,250"/>
<polyline class="cv" points="64,206 135,196 206,181 277,168 348,150 419,140 490,118 561,96 600,86"/>
<text class="tx" x="474" y="86">P99</text>
<text class="tx" x="474" y="176">P50</text>
<text class="t2" x="64" y="24">指标值</text>
</svg>"##
            )
        }
        _ => {
            let (a, b, c) = (esc(&t(0)), esc(&t(1)), esc(&t(2)));
            format!(
                r##"<svg class="figsvg" viewBox="0 0 640 300" role="img" aria-label="分层结构示意图">
<rect class="pl" x="40" y="26" width="404" height="52" rx="7"/><rect class="ac" x="40" y="26" width="6" height="52" rx="3"/>
<text class="tx" x="64" y="58">接入层</text><text class="t2" x="140" y="58">限流 · 鉴权 · 协议转换</text>
<rect class="pl" x="40" y="96" width="404" height="52" rx="7"/><rect class="ac" x="40" y="96" width="6" height="52" rx="3"/>
<text class="tx" x="64" y="128">逻辑层</text><text class="t2" x="140" y="128">{a} · {b}</text>
<rect class="pl" x="40" y="166" width="404" height="52" rx="7"/><rect class="ac" x="40" y="166" width="6" height="52" rx="3"/>
<text class="tx" x="64" y="198">数据层</text><text class="t2" x="140" y="198">{c} · 快照与日志</text>
<rect class="pl" x="40" y="236" width="404" height="52" rx="7"/><rect class="ac" x="40" y="236" width="6" height="52" rx="3"/>
<text class="tx" x="64" y="268">基础设施</text><text class="t2" x="140" y="268">监控 · 日志 · 告警</text>
<path class="gd" d="M470 52h134M470 122h134M470 192h134M470 262h134"/>
<circle class="dot" cx="478" cy="52" r="3"/><circle class="dot" cx="478" cy="122" r="3"/>
<circle class="dot" cx="478" cy="192" r="3"/><circle class="dot" cx="478" cy="262" r="3"/>
<text class="t2" x="492" y="56">可观测</text><text class="t2" x="492" y="126">可扩展</text>
<text class="t2" x="492" y="196">强一致</text><text class="t2" x="492" y="266">自愈</text>
</svg>"##
            )
        }
    }
}

/// 一整套插图：图形 + 图注，按顺序编号。
fn figure_html(kind: u8, terms: &[String], n: usize) -> String {
    let cap = match kind {
        0 => format!(
            "图 {n}：{} → {} 的调用链路",
            terms.get(0).map(|s| s.as_str()).unwrap_or("客户端"),
            terms.get(2).map(|s| s.as_str()).unwrap_or("存储")
        ),
        1 => format!("图 {n}：一次请求的完整时序"),
        2 => format!("图 {n}：不同条件下的指标变化"),
        _ => format!("图 {n}：分层结构与各层职责"),
    };
    format!(
        "<figure class=\"fig\">{}<figcaption>{}</figcaption></figure>",
        figure_svg(kind, terms),
        esc(&cap)
    )
}

/// 小标题池：技术文章的小节名，跟具体内容无关，所以放在渲染层当模板用。
/// 前面加两位编号，视觉上就是「01 背景与问题」这种结构感。
const SECTION_TITLES: [&str; 16] = [
    "背景", "过程", "细节", "分析", "讨论", "小结", "补充", "延伸", "备注", "说明", "变化", "问题",
    "结果", "方法", "取舍", "其他",
];

/// 原文里「1」「2」「一」这种纯编号标题没有任何信息量，
/// 渲染成 <h2>1</h2> 反而更像小说分节，直接换成可读的小节名。
fn trivial_heading(x: &str) -> bool {
    let t = x.trim();
    if t.is_empty() || t.chars().count() > 4 {
        return false;
    }
    t.chars().all(|c| {
        c.is_ascii_digit()
            || "一二三四五六七八九十零〇".contains(c)
            || c == '.'
            || c == '、'
            || c == '．'
    })
}

/// 章节「文章化」：小说是「一个标题 + 几千个碎段」，技术文章是「小标题 + 中等段落 + 引用」。
/// 这里只做不改变文字的重排：
///   1. 相邻超短段落合并——一行一句的对话式排版是最容易被一眼看穿的特征
///   2. 没有小标题的长章节按段落密度插小标题（模板标题，和摘要卡同一个级别）
///   3. 每个小节挑一段做成引用块，把版面切开
///   4. 「注：」「资料来源：」这类短段降成小字
/// 原文结构本来就丰富的书直接原样返回，不再揉第二遍。
fn shape_blocks(blocks: &[Block], chapter_title: &str, seed: u64) -> Vec<Block> {
    let is_p = |b: &Block| matches!(b, Block::P { .. });
    let ps = blocks.iter().filter(|b| is_p(b)).count();
    let hs = blocks
        .iter()
        .filter(|b| matches!(b, Block::H { x, .. } if !squash_eq(x, chapter_title)))
        .count();
    let rich = blocks
        .iter()
        .filter(|b| {
            matches!(
                b,
                Block::Q { .. }
                    | Block::List { .. }
                    | Block::Table { .. }
                    | Block::Code { .. }
                    | Block::Img { .. }
            )
        })
        .count();
    // 段落太少、或本来就有足够的小标题 / 结构化元素，就别动它
    if ps < 10 || hs * 8 >= ps || rich * 12 >= ps {
        return blocks.to_vec();
    }

    // ---- 1. 合并碎段 ----
    let mut merged: Vec<Block> = Vec::with_capacity(blocks.len());
    let mut run = String::new();
    for b in blocks {
        match b {
            // 只合并纯文本短段：带行内格式的合并会把粗体/链接弄丢
            Block::P { x, inl } if inl.is_empty() && x.chars().count() <= 70 => {
                run.push_str(x);
                if run.chars().count() >= 150 {
                    merged.push(Block::p(std::mem::take(&mut run)));
                }
            }
            _ => {
                if !run.is_empty() {
                    merged.push(Block::p(std::mem::take(&mut run)));
                }
                merged.push(b.clone());
            }
        }
    }
    if !run.is_empty() {
        merged.push(Block::p(run));
    }

    // ---- 2. 短注释降为小字 ----
    for i in 0..merged.len() {
        let note = match &merged[i] {
            Block::P { x, inl } if inl.is_empty() => {
                let t = x.trim_start();
                let is_note = x.chars().count() <= 90
                    && (t.starts_with("注：")
                        || t.starts_with("注:")
                        || t.starts_with("资料来源")
                        || t.starts_with("数据来源")
                        || t.starts_with("（注")
                        || t.starts_with("(")
                        || t.starts_with("编者按"));
                if is_note {
                    Some(x.clone())
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(t) = note {
            merged[i] = Block::small(t);
        }
    }

    // ---- 3. 小标题：先改写原文的无信息量编号，再按段落密度补 ----
    let base = (seed % SECTION_TITLES.len() as u64) as usize;
    let mut sec_idx = 0usize;
    for b in merged.iter_mut() {
        if let Block::H { x, .. } = b {
            if squash_eq(x, chapter_title) {
                continue;
            }
            if trivial_heading(x) {
                sec_idx += 1;
                *x = format!(
                    "{:02} {}",
                    sec_idx,
                    SECTION_TITLES[(base + sec_idx) % SECTION_TITLES.len()]
                );
            }
        }
    }

    // ---- 4. 一边按密度补小标题，一边挑段落做引用 ----
    let ps_now = merged.iter().filter(|b| is_p(b)).count();
    let mut rng = Rng::new(seed ^ 0x5f37_9e11);
    let mut out: Vec<Block> = Vec::with_capacity(merged.len() + 4);
    let mut sec_p = 0usize; // 本小节已经走过多少段
    let mut sec_quoted = false;
    let mut sec_no = 0usize; // 第几节，用来控制引用的稀疏程度
    let mut n = sec_idx;
    let mut seen_p = 0usize;

    for b in merged {
        match &b {
            Block::H { .. } => {
                sec_p = 0;
                sec_quoted = false;
                sec_no += 1;
            }
            Block::P { x, inl } if inl.is_empty() => {
                // 「一页里只有段落、没有小标题」是最像小说的排法：
                // 每走 8 段、且后面还留得下 5 段，就补一个小标题
                if sec_p >= 8 && ps_now - seen_p >= 5 {
                    n += 1;
                    out.push(Block::H {
                        level: 2,
                        x: format!(
                            "{:02} {}",
                            n,
                            SECTION_TITLES[(base + n) % SECTION_TITLES.len()]
                        ),
                    });
                    sec_p = 0;
                    sec_quoted = false;
                }
                seen_p += 1;
                sec_p += 1;
                let len = x.chars().count();
                // 第 3 段之后、长度合适的叙述才够格当引用，节首那段留给导语
                // 引用只在每 4 节里出现一次：密集引用比没有引用更像假的
                if !sec_quoted
                    && sec_no % 4 == base % 4
                    && sec_p >= 3
                    && (60..=220).contains(&len)
                    && rng.range(0, 100) < 60
                {
                    out.push(Block::q(x.clone()));
                    sec_quoted = true;
                    continue;
                }
            }
            _ => {}
        }
        out.push(b);
    }
    out
}

fn body_html(
    blocks: &[Block],
    chapter_title: &str,
    toc: &mut Vec<(String, String)>,
    ads: &[String],
    figs: &[String],
    icon: &str,
) -> String {
    let mut out = String::new();
    let mut hcount = 0usize;
    let mut skipped_lead = false;
    let mut pcount = 0usize;
    let mut led = false;

    // 文中广告的落点：按段落数决定插几条、插在哪，长文才值得打断两次
    let p_total = blocks
        .iter()
        .filter(|b| matches!(b, Block::P { .. }))
        .count();
    let mut slots: Vec<usize> = Vec::new();
    if !ads.is_empty() && p_total >= 5 {
        if p_total >= 18 {
            // 长页两条，间隔拉开：贴太近像广告墙
            slots.push(p_total * 2 / 5);
            slots.push(p_total * 4 / 5);
        } else {
            slots.push(p_total / 2);
        }
    }
    let mut ad_i = 0usize;

    // 插图落点：第一节读完放一张，长页再补一张；和广告位错开，别挤在一起
    let fig_slots: Vec<usize> = if figs.is_empty() {
        Vec::new()
    } else if p_total >= 16 {
        vec![3, 12]
    } else {
        vec![3]
    };
    let mut fig_i = 0usize;

    for blk in blocks {
        match blk {
            Block::P { x, inl } => {
                pcount += 1;
                let mut cls = "";
                // 首段做成导语：专业文章的开头不会和正文一个字号
                if !led && x.chars().count() >= 30 {
                    cls = " class=\"lead\"";
                    led = true;
                }
                if inl.is_empty() {
                    out.push_str(&format!("<p{cls}>{}</p>", esc(x)));
                } else {
                    out.push_str(&format!("<p{cls}>{}</p>", inline_html(inl)));
                }
                if slots.contains(&pcount) && ad_i < ads.len() {
                    out.push_str(&ads[ad_i]);
                    ad_i += 1;
                }
                if fig_slots.contains(&pcount) && fig_i < figs.len() {
                    out.push_str(&figs[fig_i]);
                    fig_i += 1;
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
                // 分页之后每页都从 01 开始编号，不然读者会看到「01 说明 / 09 过程」这种断号
                let shown = renumber_heading(x, hcount);
                toc.push((id.clone(), shown.clone()));
                let lv = if *level <= 2 { 2 } else { 3 };
                let mark = if lv == 2 { icon } else { "" };
                out.push_str(&format!(
                    "<h{lv} id=\"{id}\">{mark}{}</h{lv}>",
                    esc(&shown)
                ));
            }
            Block::Q { x, cite } => {
                let c = if cite.is_empty() {
                    String::new()
                } else {
                    format!("<cite>{}</cite>", esc(cite))
                };
                out.push_str(&format!(
                    "<blockquote class=\"q\"><p>{}</p>{}</blockquote>",
                    esc(x),
                    c
                ));
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
            Block::Img { src, alt, cap } => {
                let figcap = if cap.is_empty() {
                    String::new()
                } else {
                    format!("<figcaption>{}</figcaption>", esc(cap))
                };
                out.push_str(&format!(
                    "<figure class=\"fig\"><img src=\"{}\" alt=\"{}\" loading=\"lazy\">{}</figure>",
                    esc(src),
                    esc(alt),
                    figcap
                ));
            }
            Block::Table { head, rows, cap } => {
                if !cap.is_empty() {
                    out.push_str(&format!("<div class=\"tblcap\">{}</div>", esc(cap)));
                }
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
            Block::Small { x } => out.push_str(&format!("<p class=\"sm\">{}</p>", esc(x))),
            Block::Hr => out.push_str("<hr class=\"hr\">"),
        }
    }
    out
}

/// 把「09 过程」这种章内编号改成页内编号「02 过程」。
/// 只认「数字 + 空格」前缀，原文自己的标题（没有编号）原样返回。
fn renumber_heading(x: &str, n: usize) -> String {
    let t = x.trim_start();
    let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() || digits.len() > 2 {
        return x.to_string();
    }
    let rest = &t[digits.len()..];
    match rest.strip_prefix(' ') {
        Some(r) if !r.trim().is_empty() => format!("{:02} {}", n, r.trim_start()),
        _ => x.to_string(),
    }
}

fn squash_eq(a: &str, b: &str) -> bool {    let norm = |s: &str| -> String {
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
  function theme(){
    var r=document.documentElement;
    var next=(r.dataset.theme==="dark")?"light":"dark";
    r.dataset.theme=next;
    r.dataset.themeMode=next;
    try{localStorage.setItem("tf-theme",next)}catch(e){}
    // 顺带改全局设置，回书库时也是同一个外观；存不下去也不影响当前页
    if(window.fetch){
      window.fetch("/api/settings",{method:"POST",headers:{"content-type":"application/json"},
        body:JSON.stringify({appearance:next})}).catch(function(){});
    }
  }
  function help(){
    var d=document.getElementById("kbd");
    if(d){d.parentNode.removeChild(d);return}
    var rows=[["Esc","立即离开"],["← →","上一页 / 下一页"],["S","换一个皮肤（Shift 反向）"],
              ["P","换一个主题包（Shift 反向）"],["D","深色 / 浅色切换"],["+ -","正文调大 / 调小"],
              ["H","回书库"],["?","收起这块"]];
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
      case "d": case "D": theme(); break;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Chapter;

    fn book() -> Book {
        Book {
            id: "b1".into(),
            title: "测试书".into(),
            author: "作者".into(),
            intro: String::new(),
            cover: None,
            format: "txt".into(),
            added_at: 1_760_000_000,
            chapters: vec![Chapter {
                title: "第一章".into(),
                blocks: vec![Block::p("正文内容")],
            }],
        }
    }

    #[test]
    fn reader_theme_follows_appearance_and_site_palette() {
        let dark = page(&book(), 0, "csdn", "cs", "", 1, false, "dark");
        assert!(dark.contains(r#"data-theme-mode="dark""#));
        assert!(dark.contains(r#"data-theme="dark""#));
        // 结构层 + CSDN 自己的深色变量都要在页面里
        assert!(dark.contains(r#":root[data-theme="dark"] body.ly-blog main"#));
        assert!(dark.contains("--as:rgba(252,85,49,.14)"));

        let light = page(&book(), 0, "csdn", "cs", "", 1, false, "light");
        assert!(light.contains(r#"data-theme="light""#));

        let auto = page(&book(), 0, "csdn", "cs", "", 1, false, "auto");
        assert!(auto.contains(r#"data-theme-mode="auto""#));
        // auto 不写死 data-theme，交给页面里的 matchMedia
        assert!(auto.contains(r#"data-theme="""#));
    }

    #[test]
    fn every_site_has_its_own_dark_palette() {
        let sites = load("data/sites.json");
        assert!(!sites.is_empty(), "sites.json 不能是空的");
        for site in &sites {
            let id = s(site, "id");
            let css = asset_text(&format!("sites/dark/{id}.css"));
            assert!(!css.is_empty(), "{id} 缺少 sites/dark/{id}.css");
            assert!(css.contains("--bg:"), "{id} 的深色样式没有背景色变量");
        }
    }
}

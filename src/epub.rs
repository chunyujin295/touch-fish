//! EPUB 解析：container.xml → OPF → spine 逐文档转 Block。
//! 只依赖 zip + quick-xml，不引入重量级 HTML 解析器，
//! 因为电子书正文的结构远比真实网页规整；遇到畸形标签就跳过该块，不整体失败。

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use sha2::{Digest, Sha256};

use crate::model::{Block, Book, Chapter, Inline};

pub fn parse(path: &Path, id: &str, media_dir: &Path) -> Result<Book> {
    let file = File::open(path).with_context(|| format!("打开失败: {}", path.display()))?;
    let mut zip = zip::ZipArchive::new(file).context("不是合法的 EPUB（zip 解析失败）")?;

    let container = read_zip_text(&mut zip, "META-INF/container.xml")
        .context("缺少 META-INF/container.xml，可能不是 EPUB")?;
    let opf_path = parse_container(&container)?;
    let opf = read_zip_text(&mut zip, &opf_path)?;
    let opf_dir = parent_of(&opf_path);
    let meta = parse_opf(&opf)?;

    // 目录：优先 EPUB3 nav，其次 EPUB2 ncx
    let mut toc: HashMap<String, String> = HashMap::new();
    if let Some(nav) = meta.nav_href.as_deref() {
        let p = join_path(&opf_dir, nav);
        if let Ok(t) = read_zip_text(&mut zip, &p) {
            let base = parent_of(&p);
            for (src, label) in parse_nav(&t) {
                toc.insert(norm_href(&join_path(&base, &src)), label);
            }
        }
    }
    if let Some(ncx) = meta.ncx_href.as_deref() {
        let p = join_path(&opf_dir, ncx);
        if let Ok(t) = read_zip_text(&mut zip, &p) {
            let base = parent_of(&p);
            for (src, label) in parse_ncx(&t) {
                toc.insert(norm_href(&join_path(&base, &src)), label);
            }
        }
    }

    // 封面
    let mut cover: Option<String> = None;
    let cover_href = meta
        .cover_id
        .as_ref()
        .and_then(|cid| meta.items.get(cid).map(|i| i.href.clone()))
        .or_else(|| {
            meta.items
                .values()
                .find(|i| i.props.contains("cover-image"))
                .map(|i| i.href.clone())
        });
    if let Some(href) = cover_href {
        let full = join_path(&opf_dir, &href);
        if let Ok(bytes) = read_zip_bytes(&mut zip, &full) {
            if let Some(url) = save_media(media_dir, id, &full, &bytes) {
                cover = Some(url);
            }
        }
    }

    let mut chapters: Vec<Chapter> = Vec::new();
    for (n, href) in meta.spine.iter().enumerate() {
        let full = join_path(&opf_dir, href);
        let Ok(xml) = read_zip_text(&mut zip, &full) else {
            continue;
        };
        let mut ctx = Ctx {
            zip: &mut zip,
            media_dir,
            book_id: id,
        };
        let (blocks, doc_title) = parse_doc(&xml, &full, &mut ctx);
        if blocks.is_empty() {
            continue;
        }
        let title = toc
            .get(&norm_href(&full))
            .filter(|t| !t.trim().is_empty())
            .cloned()
            .or(doc_title.filter(|t| !t.trim().is_empty()))
            .or_else(|| {
                blocks.iter().find_map(|b| match b {
                    Block::H { x, .. } if x.chars().count() <= 40 => Some(x.clone()),
                    _ => None,
                })
            })
            .unwrap_or_else(|| format!("第 {} 节", n + 1));
        chapters.push(Chapter { title, blocks });
    }

    if chapters.is_empty() {
        bail!("这本 EPUB 没有解析出任何正文");
    }
    // 目录条目常把同一文件拆成多个锚点，导致重复章；去掉连续重复标题的空章
    chapters.retain(|c| c.blocks.len() > 0);

    let title = if meta.title.trim().is_empty() {
        path.file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "未命名".into())
    } else {
        meta.title
    };

    Ok(Book {
        id: id.to_string(),
        title,
        author: if meta.author.trim().is_empty() {
            "佚名".into()
        } else {
            meta.author
        },
        intro: meta.intro,
        cover,
        format: "epub".into(),
        added_at: crate::now(),
        chapters,
    })
}

/* ---------------- zip 读写 ---------------- */

fn read_zip_bytes(zip: &mut zip::ZipArchive<File>, name: &str) -> Result<Vec<u8>> {
    let mut f = zip
        .by_name(name)
        .with_context(|| format!("zip 内找不到 {name}"))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    Ok(buf)
}

fn read_zip_text(zip: &mut zip::ZipArchive<File>, name: &str) -> Result<String> {
    let buf = read_zip_bytes(zip, name)?;
    Ok(String::from_utf8_lossy(&buf).trim_start_matches('\u{feff}').to_string())
}

fn save_media(dir: &Path, book_id: &str, href: &str, bytes: &[u8]) -> Option<String> {
    let ext = href
        .rsplit('.')
        .next()
        .filter(|e| e.len() <= 5)
        .unwrap_or("bin")
        .to_lowercase();
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let name = format!("{}.{}", hex8(&digest[..8]), ext);
    std::fs::create_dir_all(dir).ok()?;
    std::fs::write(dir.join(&name), bytes).ok()?;
    Some(format!("/media/{book_id}/{name}"))
}

fn hex8(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/* ---------------- 路径 ---------------- */

fn parent_of(path: &str) -> String {
    match path.rsplit_once('/') {
        Some((dir, _)) => dir.to_string(),
        None => String::new(),
    }
}

fn join_path(base: &str, rel: &str) -> String {
    if rel.starts_with('/') {
        return rel.trim_start_matches('/').to_string();
    }
    let mut parts: Vec<&str> = base.split('/').filter(|s| !s.is_empty()).collect();
    for seg in rel.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

fn norm_href(href: &str) -> String {
    let h = href.split('#').next().unwrap_or(href);
    h.replace("%20", " ")
}

/* ---------------- 文本工具 ---------------- */

fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let Some(end) = tail.find(';').filter(|e| *e <= 10) else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let ent = &tail[1..end];
        let decoded = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            "mdash" => Some('—'),
            "ndash" => Some('–'),
            "hellip" => Some('…'),
            "ldquo" => Some('“'),
            "rdquo" => Some('”'),
            "lsquo" => Some('‘'),
            "rsquo" => Some('’'),
            _ => {
                if let Some(num) = ent.strip_prefix("#x").or(ent.strip_prefix("#X")) {
                    u32::from_str_radix(num, 16).ok().and_then(char::from_u32)
                } else if let Some(num) = ent.strip_prefix('#') {
                    num.parse::<u32>().ok().and_then(char::from_u32)
                } else {
                    None
                }
            }
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &tail[end + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn squash(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = true;
    for c in s.chars() {
        if c.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

fn attr(e: &BytesStart, key: &[u8]) -> Option<String> {
    for a in e.attributes().flatten() {
        if a.key.local_name().as_ref() == key {
            return Some(unescape(&String::from_utf8_lossy(&a.value)));
        }
    }
    None
}

/* ---------------- container / OPF ---------------- */

fn parse_container(xml: &str) -> Result<String> {
    let mut reader = Reader::from_str(xml);
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                if e.name().local_name().as_ref() == b"rootfile" {
                    if let Some(p) = attr(&e, b"full-path") {
                        return Ok(p);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(anyhow!("container.xml 解析失败: {e}")),
            _ => {}
        }
    }
    Err(anyhow!("container.xml 里没有 rootfile"))
}

#[derive(Debug, Default, Clone)]
struct Item {
    href: String,
    media_type: String,
    props: String,
}

#[derive(Debug, Default)]
struct OpfMeta {
    title: String,
    author: String,
    intro: String,
    nav_href: Option<String>,
    ncx_href: Option<String>,
    cover_id: Option<String>,
    items: HashMap<String, Item>,
    spine: Vec<String>,
}

fn parse_opf(xml: &str) -> Result<OpfMeta> {
    let mut m = OpfMeta::default();
    let mut reader = Reader::from_str(xml);
    let mut field: Option<&'static str> = None;
    let mut buf = String::new();
    let mut toc_id: Option<String> = None;
    let mut manifest: Vec<(String, Item)> = Vec::new();

    // 自闭合元素（<item .../> <meta .../> <itemref .../>）走 Empty 分支，
    // 带子节点的（<spine>）走 Start 分支，两边都要认。
    fn on_elem(
        e: &BytesStart,
        manifest: &mut Vec<(String, Item)>,
        spine: &mut Vec<String>,
        toc_id: &mut Option<String>,
        cover_id: &mut Option<String>,
    ) {
        match e.name().local_name().as_ref() {
            b"item" => {
                if let (Some(id), Some(href)) = (attr(e, b"id"), attr(e, b"href")) {
                    manifest.push((
                        id,
                        Item {
                            href,
                            media_type: attr(e, b"media-type").unwrap_or_default(),
                            props: attr(e, b"properties").unwrap_or_default(),
                        },
                    ));
                }
            }
            b"itemref" => {
                if let Some(id) = attr(e, b"idref") {
                    spine.push(id);
                }
            }
            b"spine" => {
                if let Some(t) = attr(e, b"toc") {
                    *toc_id = Some(t);
                }
            }
            b"meta" => {
                if attr(e, b"name").as_deref() == Some("cover") {
                    *cover_id = attr(e, b"content");
                }
            }
            _ => {}
        }
    }

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = e.name().local_name();
                match name.as_ref() {
                    b"title" => {
                        field = Some("title");
                        buf.clear();
                    }
                    b"creator" => {
                        field = Some("creator");
                        buf.clear();
                    }
                    b"description" => {
                        field = Some("intro");
                        buf.clear();
                    }
                    _ => {}
                }
                on_elem(&e, &mut manifest, &mut m.spine, &mut toc_id, &mut m.cover_id);
            }
            Ok(Event::Empty(e)) => {
                on_elem(&e, &mut manifest, &mut m.spine, &mut toc_id, &mut m.cover_id);
            }
            Ok(Event::End(e)) => {
                let name = e.name().local_name();
                if matches!(name.as_ref(), b"title" | b"creator" | b"description") {
                    if let Some(f) = field.take() {
                        let val = squash(&unescape(&buf));
                        match f {
                            "title" if m.title.is_empty() => m.title = val,
                            "creator" if m.author.is_empty() => m.author = val,
                            "intro" if m.intro.is_empty() => m.intro = val,
                            _ => {}
                        }
                        buf.clear();
                    }
                }
            }
            Ok(Event::Text(t)) => {
                if field.is_some() {
                    buf.push_str(&decode_text(&t));
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(anyhow!("OPF 解析失败: {e}")),
            _ => {}
        }
    }

    m.items = manifest.iter().cloned().collect();
    m.nav_href = manifest
        .iter()
        .find(|(_, i)| i.props.split_whitespace().any(|p| p == "nav"))
        .map(|(_, i)| i.href.clone());
    m.ncx_href = toc_id
        .as_ref()
        .and_then(|id| m.items.get(id))
        .map(|i| i.href.clone())
        .or_else(|| {
            manifest
                .iter()
                .find(|(_, i)| i.media_type == "application/x-dtbncx+xml")
                .map(|(_, i)| i.href.clone())
        });

    // spine 里的 idref → href
    m.spine = m
        .spine
        .iter()
        .filter_map(|id| m.items.get(id).map(|i| i.href.clone()))
        .collect();

    // 保底：有些书 spine 缺失或 idref 对不上，按 manifest 里的 xhtml 顺序读
    if m.spine.is_empty() {
        m.spine = manifest
            .iter()
            .filter(|(_, i)| {
                i.media_type.contains("xhtml")
                    && !i.props.split_whitespace().any(|p| p == "nav")
            })
            .map(|(_, i)| i.href.clone())
            .collect();
    }

    Ok(m)
}

/* ---------------- 目录 ---------------- */

/// 返回 (src, label)
fn parse_ncx(xml: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut reader = Reader::from_str(xml);
    let mut in_label = false;
    let mut label = String::new();
    let mut pending_src: Option<String> = None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let n = e.name().local_name();
                if n.as_ref() == b"navlabel" {
                    in_label = true;
                    label.clear();
                } else if n.as_ref() == b"content" {
                    if let Some(src) = attr(&e, b"src") {
                        pending_src = Some(src);
                    }
                }
            }
            Ok(Event::Empty(e)) => {
                if e.name().local_name().as_ref() == b"content" {
                    if let Some(src) = attr(&e, b"src") {
                        pending_src = Some(src);
                    }
                }
            }
            Ok(Event::End(e)) => {
                let n = e.name().local_name();
                if n.as_ref() == b"navlabel" {
                    in_label = false;
                } else if n.as_ref() == b"navpoint" {
                    if let (Some(src), l) = (pending_src.take(), squash(&label)) {
                        if !l.is_empty() {
                            out.push((src, l));
                        }
                    }
                }
            }
            Ok(Event::Text(t)) => {
                if in_label {
                    label.push_str(&decode_text(&t));
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    out
}

/// EPUB3 nav.xhtml，返回 (href, label)
fn parse_nav(xml: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut reader = Reader::from_str(xml);
    let mut in_a = false;
    let mut href = String::new();
    let mut label = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                if e.name().local_name().as_ref() == b"a" {
                    in_a = true;
                    label.clear();
                    href = attr(&e, b"href").unwrap_or_default();
                }
            }
            Ok(Event::End(e)) => {
                if e.name().local_name().as_ref() == b"a" && in_a {
                    in_a = false;
                    let l = squash(&label);
                    if !href.is_empty() && !l.is_empty() {
                        out.push((std::mem::take(&mut href), l));
                    }
                }
            }
            Ok(Event::Text(t)) => {
                if in_a {
                    label.push_str(&decode_text(&t));
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    out
}

/* ---------------- 正文 ---------------- */

struct Ctx<'a> {
    zip: &'a mut zip::ZipArchive<File>,
    media_dir: &'a Path,
    book_id: &'a str,
}

fn decode_text(t: &quick_xml::events::BytesText) -> String {
    unescape(&String::from_utf8_lossy(t))
}

fn parse_doc(xml: &str, doc_path: &str, ctx: &mut Ctx) -> (Vec<Block>, Option<String>) {
    let mut p = Parser::default();
    let base = parent_of(doc_path);
    let mut reader = Reader::from_str(xml);

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let n = e.name().local_name();
                p.on_start(n.as_ref(), &e, &base, ctx);
            }
            Ok(Event::Empty(e)) => {
                let n = e.name().local_name();
                if n.as_ref() == b"img" {
                    if let Some(b) = p.take_img(&e, &base, ctx) {
                        p.flush_pending();
                        p.blocks.push(b);
                    }
                } else {
                    p.on_start(n.as_ref(), &e, &base, ctx);
                    p.on_end(n.as_ref());
                }
            }
            Ok(Event::End(e)) => {
                let n = e.name().local_name();
                p.on_end(n.as_ref());
            }
            Ok(Event::Text(t)) => {
                let s = decode_text(&t);
                p.on_text(&s);
            }
            Ok(Event::CData(t)) => {
                let s = String::from_utf8_lossy(&t).to_string();
                p.on_text(&s);
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    p.flush_pending();
    (p.blocks, p.title)
}

#[derive(Clone, Copy, PartialEq)]
enum RunKind {
    Text,
    Strong,
    Em,
    Code,
    Link,
}

/// 把一段文本按当前样式接到行内序列末尾；同类的相邻片段直接合并，
/// 免得一本书的段落里堆出几百个只有几个字的小片段。
fn push_run(inl: &mut Vec<Inline>, kind: RunKind, href: &str, s: &str) {
    if s.is_empty() {
        return;
    }
    if let Some(last) = inl.last_mut() {
        let slot = match (last, kind) {
            (Inline::Text(t), RunKind::Text) => Some(t),
            (Inline::Strong(t), RunKind::Strong) => Some(t),
            (Inline::Em(t), RunKind::Em) => Some(t),
            (Inline::Code(t), RunKind::Code) => Some(t),
            (Inline::Link { href: h, text: t }, RunKind::Link) if h == href => Some(t),
            _ => None,
        };
        if let Some(t) = slot {
            t.push_str(s);
            return;
        }
    }
    inl.push(match kind {
        RunKind::Text => Inline::Text(s.to_string()),
        RunKind::Strong => Inline::Strong(s.to_string()),
        RunKind::Em => Inline::Em(s.to_string()),
        RunKind::Code => Inline::Code(s.to_string()),
        RunKind::Link => Inline::Link {
            href: href.to_string(),
            text: s.to_string(),
        },
    });
}

/// 折叠空白但**不裁两端**——裁了的话相邻片段之间该有的空格会丢，"foo"+" bar" 会粘成 "foobar"
fn collapse(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out
}

/// 收尾：折叠空白、丢掉空片段、裁掉整段的头尾空格
fn finish_runs(runs: Vec<Inline>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::new();
    for r in runs {
        let r = match r {
            Inline::Text(t) => Inline::Text(collapse(&t)),
            Inline::Strong(t) => Inline::Strong(collapse(&t)),
            Inline::Em(t) => Inline::Em(collapse(&t)),
            Inline::Code(t) => Inline::Code(collapse(&t)),
            Inline::Link { href, text } => Inline::Link {
                href,
                text: collapse(&text),
            },
        };
        let empty = match &r {
            Inline::Text(t) | Inline::Strong(t) | Inline::Em(t) | Inline::Code(t) => t.is_empty(),
            Inline::Link { text, .. } => text.is_empty(),
        };
        if empty {
            continue;
        }
        push_run(
            &mut out,
            match &r {
                Inline::Text(_) => RunKind::Text,
                Inline::Strong(_) => RunKind::Strong,
                Inline::Em(_) => RunKind::Em,
                Inline::Code(_) => RunKind::Code,
                Inline::Link { .. } => RunKind::Link,
            },
            match &r {
                Inline::Link { href, .. } => href,
                _ => "",
            },
            match &r {
                Inline::Text(t) | Inline::Strong(t) | Inline::Em(t) | Inline::Code(t) => t,
                Inline::Link { text, .. } => text,
            },
        );
    }
    if let Some(Inline::Text(t) | Inline::Strong(t) | Inline::Em(t) | Inline::Code(t)) =
        out.first_mut()
    {
        *t = t.trim_start().to_string();
    } else if let Some(Inline::Link { text, .. }) = out.first_mut() {
        *text = text.trim_start().to_string();
    }
    if let Some(Inline::Text(t) | Inline::Strong(t) | Inline::Em(t) | Inline::Code(t)) =
        out.last_mut()
    {
        *t = t.trim_end().to_string();
    } else if let Some(Inline::Link { text, .. }) = out.last_mut() {
        *text = text.trim_end().to_string();
    }
    out.retain(|r| match r {
        Inline::Text(t) | Inline::Strong(t) | Inline::Em(t) | Inline::Code(t) => !t.is_empty(),
        Inline::Link { text, .. } => !text.is_empty(),
    });
    // 全是纯文本就没必要带着 inl 走，省一半存储
    if out.len() == 1 && matches!(out[0], Inline::Text(_)) {
        return Vec::new();
    }
    out
}

#[derive(Default)]
struct Parser {
    blocks: Vec<Block>,
    buf: String,
    /// 段落的行内片段（粗体/斜体/链接/行内代码），只在 P 上用
    inl: Vec<Inline>,
    bold: usize,
    ital: usize,
    icode: usize,
    link: Option<String>,
    /// 已打开但还没结束的 p/h/quote/code，用于判断是不是最外层
    open: Vec<&'static str>,
    pre: usize,
    list_depth: usize,
    list_ordered: bool,
    list_items: Vec<String>,
    table_depth: usize,
    rows: Vec<Vec<String>>,
    row: Vec<String>,
    cell: String,
    in_title: bool,
    title: Option<String>,
    title_buf: String,
    skip: usize,
}

impl Parser {
    fn on_text(&mut self, s: &str) {
        if self.in_title {
            self.title_buf.push_str(s);
            return;
        }
        if self.skip > 0 {
            return;
        }
        if self.pre > 0 || !self.open.is_empty() || self.table_depth > 0 || self.list_depth > 0 {
            self.buf.push_str(s);
        }
        // 段落/引用的行内格式只在这两种块上收集；列表项、表格单元格、代码块都当纯文本
        if self.pre == 0
            && self.list_depth == 0
            && self.table_depth == 0
            && !self.open.is_empty()
        {
            let kind = if self.icode > 0 {
                RunKind::Code
            } else if self.link.is_some() {
                RunKind::Link
            } else if self.bold > 0 {
                RunKind::Strong
            } else if self.ital > 0 {
                RunKind::Em
            } else {
                RunKind::Text
            };
            let href = self.link.clone().unwrap_or_default();
            push_run(&mut self.inl, kind, &href, s);
        }
    }

    fn take_img(&mut self, e: &BytesStart, base: &str, ctx: &mut Ctx) -> Option<Block> {
        let src = attr(e, b"src")?;
        if src.starts_with("data:") {
            return None;
        }
        let full = join_path(base, &src);
        let bytes = read_zip_bytes(ctx.zip, &full).ok()?;
        let url = save_media(ctx.media_dir, ctx.book_id, &full, &bytes)?;
        Some(Block::Img {
            src: url,
            alt: attr(e, b"alt").unwrap_or_default(),
        })
    }

    fn on_start(&mut self, n: &[u8], e: &BytesStart, base: &str, ctx: &mut Ctx) {
        match n {
            b"title" => {
                self.in_title = true;
                self.title_buf.clear();
            }
            b"script" | b"style" | b"head" => self.skip += 1,
            b"p" => {
                self.open.push("p");
                if !self.buf.is_empty() {
                    self.buf.push(' ');
                }
            }
            b"h1" => {
                self.open.push("h1");
            }
            b"h2" => {
                self.open.push("h2");
            }
            b"h3" => self.open.push("h3"),
            b"h4" => self.open.push("h4"),
            b"h5" => self.open.push("h5"),
            b"h6" => self.open.push("h6"),
            b"blockquote" => self.open.push("q"),
            b"b" | b"strong" => self.bold += 1,
            b"i" | b"em" => self.ital += 1,
            b"code" | b"tt" | b"kbd" | b"samp" if self.pre == 0 => self.icode += 1,
            b"a" => {
                if let Some(href) = attr(e, b"href") {
                    if !href.trim().is_empty() {
                        self.link = Some(href.trim().to_string());
                    }
                }
            }
            b"pre" => {
                self.pre += 1;
                self.open.push("code");
                self.buf.clear();
            }
            b"br" => {
                if self.pre > 0 {
                    self.buf.push('\n');
                } else {
                    self.buf.push(' ');
                }
            }
            b"ul" | b"ol" => {
                if self.list_depth == 0 {
                    self.list_ordered = n == b"ol";
                    self.list_items.clear();
                }
                self.list_depth += 1;
            }
            b"li" => {
                self.buf.clear();
            }
            b"table" => {
                self.table_depth += 1;
                if self.table_depth == 1 {
                    self.rows.clear();
                }
            }
            b"tr" => {
                self.row.clear();
            }
            b"td" | b"th" => {
                self.cell.clear();
                self.buf.clear();
            }
            b"img" => {
                if let Some(b) = self.take_img(e, base, ctx) {
                    self.flush_pending();
                    self.blocks.push(b);
                }
            }
            b"hr" => {
                self.flush_pending();
                self.blocks.push(Block::Hr);
            }
            _ => {}
        }
    }

    fn on_end(&mut self, n: &[u8]) {
        match n {
            b"title" => {
                self.in_title = false;
                if self.title.is_none() {
                    let t = squash(&self.title_buf);
                    if !t.is_empty() {
                        self.title = Some(t);
                    }
                }
            }
            b"script" | b"style" | b"head" => {
                self.skip = self.skip.saturating_sub(1);
            }
            b"p" => {
                self.open.retain(|k| *k != "p");
                if self.list_depth == 0 && self.table_depth == 0 && self.open.is_empty() {
                    self.flush_para(Block::p(""));
                }
            }
            b"h1" | b"h2" | b"h3" | b"h4" | b"h5" | b"h6" => {
                let key = match n {
                    b"h1" => "h1",
                    b"h2" => "h2",
                    b"h3" => "h3",
                    b"h4" => "h4",
                    b"h5" => "h5",
                    _ => "h6",
                };
                self.open.retain(|k| *k != key);
                if self.open.is_empty() {
                    let level = (n[1] - b'0') as u8;
                    let x = squash(&std::mem::take(&mut self.buf));
                    self.inl.clear();
                    if !x.is_empty() {
                        self.blocks.push(Block::H { level, x });
                    }
                }
            }
            b"blockquote" => {
                self.open.retain(|k| *k != "q");
                self.inl.clear();
                if self.open.is_empty() {
                    self.flush_para(Block::q(""));
                }
            }
            b"b" | b"strong" => self.bold = self.bold.saturating_sub(1),
            b"i" | b"em" => self.ital = self.ital.saturating_sub(1),
            b"code" | b"tt" | b"kbd" | b"samp" => self.icode = self.icode.saturating_sub(1),
            b"a" => self.link = None,
            b"pre" => {
                self.pre = self.pre.saturating_sub(1);
                self.open.retain(|k| *k != "code");
                if self.pre == 0 {
                    let x = std::mem::take(&mut self.buf);
                    let x = x.trim_matches('\n').to_string();
                    if !x.trim().is_empty() {
                        self.blocks.push(Block::Code {
                            x,
                            lang: String::new(),
                        });
                    }
                }
            }
            b"li" => {
                let item = squash(&std::mem::take(&mut self.buf));
                if !item.is_empty() {
                    self.list_items.push(item);
                }
            }
            b"ul" | b"ol" => {
                self.list_depth = self.list_depth.saturating_sub(1);
                if self.list_depth == 0 && !self.list_items.is_empty() {
                    self.blocks.push(Block::List {
                        ordered: self.list_ordered,
                        items: std::mem::take(&mut self.list_items),
                    });
                }
            }
            b"td" | b"th" => {
                let c = squash(&std::mem::take(&mut self.buf));
                self.row.push(c);
            }
            b"tr" => {
                if !self.row.is_empty() {
                    self.rows.push(std::mem::take(&mut self.row));
                }
            }
            b"table" => {
                self.table_depth = self.table_depth.saturating_sub(1);
                if self.table_depth == 0 && !self.rows.is_empty() {
                    let mut rows = std::mem::take(&mut self.rows);
                    let head = if rows.len() > 1 {
                        rows.remove(0)
                    } else {
                        Vec::new()
                    };
                    self.blocks.push(Block::Table { head, rows });
                }
            }
            _ => {}
        }
    }

    /// 用给定构造器替换 x 字段，避免重复代码
    fn flush_para(&mut self, proto: Block) {
        let x = squash(&std::mem::take(&mut self.buf));
        let inl = finish_runs(std::mem::take(&mut self.inl));
        if x.is_empty() {
            return;
        }
        match proto {
            Block::P { .. } => self.blocks.push(Block::rich(x, inl)),
            Block::Q { .. } => self.blocks.push(Block::q(x)),
            _ => {}
        }
    }

    /// 收尾：把残留文本补成段落
    fn flush_pending(&mut self) {
        let x = squash(&std::mem::take(&mut self.buf));
        let inl = finish_runs(std::mem::take(&mut self.inl));
        if !x.is_empty() && self.pre == 0 {
            self.blocks.push(Block::rich(x, inl));
        }
    }
}

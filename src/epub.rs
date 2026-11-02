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

use crate::html::{attr, decode_text, parse as parse_html, squash, unescape};
use crate::media;
use crate::model::{Block, Book, Chapter};

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
            if let Some(url) = media::save(media_dir, id, ext_of(&full), &bytes) {
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
        // 图片：从 zip 里按相对路径取出来存成站内文件，再交给解析器
        let mut sink = |e: &BytesStart, base: &str| -> Option<Block> {
            let src = attr(e, b"src")?;
            if src.starts_with("data:") {
                return None;
            }
            let href_full = join_path(base, &src);
            let bytes = read_zip_bytes(&mut zip, &href_full).ok()?;
            let url = media::save(media_dir, id, ext_of(&href_full), &bytes)?;
            Some(Block::Img {
                src: url,
                alt: attr(e, b"alt").unwrap_or_default(),
            })
        };
        let (blocks, doc_title) = parse_html(&xml, &full, &mut sink);
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

/// 从路径里取扩展名，交给 media::save 决定文件名
fn ext_of(path: &str) -> &str {
    path.rsplit('.')
        .next()
        .filter(|e| e.len() <= 5)
        .unwrap_or("")
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

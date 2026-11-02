//! TXT 解析：编码探测 → 章节切分 → 段落切分。
//! 中文 txt 电子书编码极杂（GBK/GB18030/UTF-8/BIG5），必须靠探测而不是假设。

use std::path::Path;

use anyhow::Result;
use encoding_rs::Encoding;

use crate::model::{Block, Book, Chapter};

pub fn parse(path: &Path, id: &str, display_name: &str) -> Result<Book> {
    let bytes = std::fs::read(path)?;
    let enc = detect(&bytes);
    let (decoded, _, _) = enc.decode(&bytes);
    let text = decoded
        .trim_start_matches('\u{feff}')
        .replace("\r\n", "\n")
        .replace('\r', "\n");

    let mut chapters = split_chapters(&text);
    if chapters.is_empty() {
        chapters.push(Chapter {
            title: "全文".into(),
            blocks: vec![Block::p(text.trim())],
        });
    }

    // 标题用原始文件名，不用临时文件路径
    let stem = display_name
        .rsplit('/')
        .next()
        .unwrap_or(display_name)
        .rsplit_once('.')
        .map(|(a, _)| a)
        .unwrap_or(display_name);
    let (title, author) = split_name_author(stem);

    Ok(Book {
        id: id.to_string(),
        title,
        author,
        intro: String::new(),
        cover: None,
        format: "txt".into(),
        added_at: crate::now(),
        chapters,
    })
}

fn detect(bytes: &[u8]) -> &'static Encoding {
    let mut det = chardetng::EncodingDetector::new();
    det.feed(bytes, true);
    det.guess(None, true)
}

fn split_name_author(stem: &str) -> (String, String) {
    if let (Some(a), Some(b)) = (stem.find('《'), stem.find('》')) {
        if b > a {
            // 注意：《 占 3 个字节，不能写 a + 1，否则切在多字节字符中间会 panic
            let title = stem[a + '《'.len_utf8()..b].trim().to_string();
            let rest = stem[b + '》'.len_utf8()..].trim().to_string();
            let author = rest
                .trim_start_matches(|c: char| matches!(c, '作' | '者' | ':' | '：' | ' '))
                .trim()
                .to_string();
            if !title.is_empty() {
                return (
                    title,
                    if author.is_empty() { "佚名".into() } else { author },
                );
            }
        }
    }
    (stem.trim().to_string(), "佚名".into())
}

/// 书名 / 作者 / 版权这类元信息行，渲染进正文会很出戏
fn is_meta(b: &Block) -> bool {
    let Block::P { x, .. } = b else {
        return false;
    };
    let t = x.trim();
    if t.chars().count() > 40 {
        return false;
    }
    ["《", "作者", "版权所有", "版权", "声明", "Copyright"]
        .iter()
        .any(|p| t.starts_with(p))
}

fn is_heading(line: &str) -> bool {    let t = line.trim();
    let n = t.chars().count();
    if n == 0 || n > 30 {
        return false;
    }
    if t.starts_with('第') {
        let head: String = t.chars().take(8).collect();
        if head.contains('章') || head.contains('回') || head.contains('节') || head.contains('卷') {
            return true;
        }
    }
    let low = t.to_lowercase();
    low.starts_with("chapter ") || low.starts_with("prologue") || low.starts_with("epilogue")
}

fn split_chapters(text: &str) -> Vec<Chapter> {
    let mut chapters: Vec<Chapter> = Vec::new();
    let mut cur_title: Option<String> = None;
    let mut paras: Vec<String> = Vec::new();
    let mut buf = String::new();

    let flush_paras = |chapters: &mut Vec<Chapter>, title: &Option<String>, paras: &mut Vec<String>| {
        if paras.is_empty() {
            return;
        }
        let blocks: Vec<Block> = paras
            .drain(..)
            .map(Block::p)
            .collect();
        chapters.push(Chapter {
            title: title.clone().unwrap_or_else(|| "正文".into()),
            blocks,
        });
    };

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            let p = buf.trim().to_string();
            if !p.is_empty() {
                paras.push(p);
            }
            buf.clear();
            continue;
        }
        if is_heading(trimmed) {
            let p = buf.trim().to_string();
            if !p.is_empty() {
                paras.push(p);
            }
            buf.clear();
            flush_paras(&mut chapters, &cur_title, &mut paras);
            cur_title = Some(trimmed.to_string());
            continue;
        }
        if !buf.is_empty() {
            // 中文换行多为排版折行，直接接上；纯英文按空格接
            let last = buf.chars().last().unwrap_or(' ');
            if last.is_ascii_alphanumeric() {
                buf.push(' ');
            }
        }
        buf.push_str(trimmed);
    }

    let p = buf.trim().to_string();
    if !p.is_empty() {
        paras.push(p);
    }
    flush_paras(&mut chapters, &cur_title, &mut paras);

    // 开头那段没有章节标记的文字通常只是书名 / 作者 / 版权页。
    // 让它单独成章的话，帖子里会出现一个标题叫「正文」的帖子，一眼假，所以并进第一章；
    // 但书名作者这类元信息本身也不该出现在正文第一段里，一并丢掉。
    if chapters.len() >= 2 && chapters[0].title == "正文" && chapters[0].blocks.len() <= 3 {
        let head = chapters.remove(0);
        let mut blocks: Vec<Block> = head.blocks.into_iter().filter(|b| !is_meta(b)).collect();
        blocks.extend(std::mem::take(&mut chapters[0].blocks));
        chapters[0].blocks = blocks;
    }

    // 完全没有章节标记时，按长度切块，避免一个几千段的巨型页面
    if chapters.len() == 1 && chapters[0].blocks.len() > 120 {
        let blocks = std::mem::take(&mut chapters[0].blocks);
        let per = 60;
        let total = blocks.len().div_ceil(per);
        chapters.clear();
        for (i, chunk) in blocks.chunks(per).enumerate() {
            chapters.push(Chapter {
                title: format!("第 {} 节 / 共 {} 节", i + 1, total),
                blocks: chunk.to_vec(),
            });
        }
    }

    chapters
}

//! MOBI / PRC 解析：PalmDB 容器 → PalmDOC 头 → 解压文本记录 → HTML → Block。
//!
//! 结构说明（照着 KindleUnpack 的字段顺序抄的）：
//! - 文件本身是 PalmDB：78 字节头 + 每记录 8 字节的偏移表，记录 0 是 PalmDOC/MOBI 头。
//! - PalmDOC 头 16 字节：压缩方式 / 文本长度 / 文本记录数 / 记录大小 / 加密标志。
//! - 紧随其后是 MOBI 头：文本编码、版本、首图记录号都在这里；EXTH 里放书名与作者。
//! - 正文是记录 1..N 解压后拼起来的 HTML，用 `<mbp:pagebreak/>` 分章。
//!
//! 暂时不支持 HUFF/CDIC 压缩（AZW 常见）与 DRM；这两种都会给出明确提示，
//! 而不是像以前那样把二进制当 TXT 读出一堆乱码。

use std::path::Path;

use anyhow::{bail, Context, Result};
use quick_xml::events::BytesStart;

use crate::html::{attr, parse as parse_html};
use crate::media;
use crate::model::{Block, Book, Chapter};

const HUFF_COMPRESSION: u16 = 17480;
const NO_IMAGE: usize = 0xffff_ffff;

pub fn parse(path: &Path, id: &str, media_dir: &Path) -> Result<Book> {
    let data = std::fs::read(path).with_context(|| format!("读取失败: {}", path.display()))?;
    let pdb = Pdb::new(&data)?;
    let r0 = pdb.record(0).context("MOBI 缺少 0 号记录")?;
    if r0.len() < 132 {
        bail!("不是 MOBI：0 号记录太短");
    }
    if &r0[16..20] != b"MOBI" {
        bail!("这个 PDB 不是 MOBI（没有 MOBI 头）");
    }

    let compression = be16(r0, 0);
    let text_len = be32(r0, 4) as usize;
    let text_records = be16(r0, 8) as usize;
    let encryption = be16(r0, 12);
    let header_len = be32(r0, 20) as usize;
    let encoding = be32(r0, 28);
    let version = be32(r0, 36);
    let first_image = be32(r0, 108) as usize;

    if encryption != 0 {
        bail!("这本 MOBI 有 DRM 加密，解不开。请先用 Calibre 转换（会顺带去 DRM）再导入");
    }
    match compression {
        1 | 2 => {}
        HUFF_COMPRESSION => bail!(
            "这本用的是 HUFF/CDIC 压缩（AZW 常见），暂不支持。请用 Calibre 转成 epub 再导入"
        ),
        other => bail!("未知的 MOBI 压缩方式 {other}"),
    }
    if text_records == 0 || text_records + 1 > pdb.records() {
        bail!("MOBI 头里的文本记录数不对（{text_records}）");
    }

    // 逐条解压。UTF-8 的字符可能被记录边界切断，多出来的半个字符要顺延到下一段，
    // 否则拼接处会出现一串替换字符。
    let mut raw: Vec<u8> = Vec::with_capacity(text_len.max(4096));
    let mut carry: Vec<u8> = Vec::new();
    for i in 0..text_records {
        let rec = pdb
            .record(i + 1)
            .with_context(|| format!("缺少第 {} 条文本记录", i + 1))?;
        let mut chunk = if compression == 1 {
            rec.to_vec()
        } else {
            palmdoc_decompress(rec)
                .with_context(|| format!("第 {} 条文本记录解压失败", i + 1))?
        };
        if !carry.is_empty() {
            let mut joined = std::mem::take(&mut carry);
            joined.extend_from_slice(&chunk);
            chunk = joined;
        }
        if encoding == 65001 {
            let (head, tail) = split_incomplete_utf8(&chunk);
            raw.extend_from_slice(head);
            carry = tail.to_vec();
        } else {
            raw.extend_from_slice(&chunk);
        }
    }
    raw.extend_from_slice(&carry);

    let text = if encoding == 65001 {
        String::from_utf8_lossy(&raw).to_string()
    } else {
        let (s, _, _) = encoding_rs::WINDOWS_1252.decode(&raw);
        s.to_string()
    };
    if text.trim().is_empty() {
        bail!("MOBI 正文是空的（版本 v{version}，可能是 KF8/AZW3 的混合容器，请用 Calibre 转 epub）");
    }

    // 分章：优先 mbp:pagebreak，没有就看 h1/h2
    let mut pages = split_on(&text, &[b"<mbp:pagebreak"]);
    if pages.len() <= 1 {
        pages = split_on(&text, &[b"<h1", b"<h2"]);
    }
    if pages.is_empty() {
        pages.push(text.clone());
    }

    // 图片：<img recindex="00003"/> 指向 PalmDB 的第 first_image + recindex - 1 条记录
    let mut sink = |e: &BytesStart, _base: &str| -> Option<Block> {
        let idx: usize = attr(e, b"recindex")
            .or_else(|| attr(e, b"mediarecindex"))?
            .trim()
            .parse()
            .ok()?;
        if first_image == NO_IMAGE || first_image == 0 || idx == 0 {
            return None;
        }
        let rec = pdb.record(first_image + idx - 1).ok()?;
        let url = media::save(media_dir, id, "", rec)?;
        Some(Block::Img {
            src: url,
            alt: String::new(),
            cap: String::new(),
        })
    };

    let mut chapters: Vec<Chapter> = Vec::new();
    for (n, page) in pages.iter().enumerate() {
        if page.trim().is_empty() {
            continue;
        }
        let (blocks, doc_title) = parse_html(page, "", &mut sink);
        if blocks.is_empty() {
            continue;
        }
        let title = doc_title
            .filter(|t| !t.trim().is_empty())
            .or_else(|| {
                blocks.iter().find_map(|b| match b {
                    Block::H { x, level } if *level <= 3 && x.chars().count() <= 40 => {
                        Some(x.clone())
                    }
                    _ => None,
                })
            })
            .unwrap_or_else(|| format!("第 {} 节", n + 1));
        chapters.push(Chapter { title, blocks });
    }

    if chapters.is_empty() {
        bail!("这本 MOBI 没有解析出正文（可能是 KF8/AZW3 容器，请用 Calibre 转 epub）");
    }

    let (exth_title, exth_author) = read_exth(r0, header_len, encoding);
    let title = exth_title
        .filter(|t| !t.trim().is_empty())
        .or_else(|| pdb.name().filter(|t| !t.trim().is_empty()))
        .unwrap_or_else(|| "未命名".into());
    let author = exth_author
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| "佚名".into());

    Ok(Book {
        id: id.to_string(),
        title,
        author,
        intro: String::new(),
        cover: None,
        format: "mobi".into(),
        added_at: crate::now(),
        chapters,
    })
}

/* ---------------- PalmDB ---------------- */

struct Pdb<'a> {
    data: &'a [u8],
    /// 每条记录的起点；多存一个结束位置，省得每次算
    offsets: Vec<usize>,
}

impl<'a> Pdb<'a> {
    fn new(data: &'a [u8]) -> Result<Self> {
        if data.len() < 86 {
            bail!("不是 MOBI：文件太小");
        }
        let n = be16(data, 76) as usize;
        if n == 0 || data.len() < 78 + n * 8 {
            bail!("PalmDB 记录表损坏");
        }
        let mut offsets = Vec::with_capacity(n + 1);
        for i in 0..n {
            offsets.push(be32(data, 78 + i * 8) as usize);
        }
        offsets.push(data.len());
        // 偏移必须单调递增，否则是坏文件
        if offsets.windows(2).any(|w| w[0] > w[1] || w[1] > data.len()) {
            bail!("PalmDB 记录偏移非法");
        }
        Ok(Self { data, offsets })
    }

    fn records(&self) -> usize {
        self.offsets.len() - 1
    }

    fn record(&self, i: usize) -> Result<&'a [u8]> {
        if i + 1 >= self.offsets.len() {
            bail!("记录 {i} 越界");
        }
        Ok(&self.data[self.offsets[i]..self.offsets[i + 1]])
    }

    /// 文件头前 32 字节是书名（以 0 结尾），常带着一串填充
    fn name(&self) -> Option<String> {
        let raw = &self.data[..32];
        let end = raw.iter().position(|b| *b == 0).unwrap_or(raw.len());
        let s = String::from_utf8_lossy(&raw[..end]).trim().to_string();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }
}

fn be16(b: &[u8], at: usize) -> u16 {
    if at + 2 > b.len() {
        return 0;
    }
    u16::from_be_bytes([b[at], b[at + 1]])
}

fn be32(b: &[u8], at: usize) -> u32 {
    if at + 4 > b.len() {
        return 0;
    }
    u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/* ---------------- 解压 ---------------- */

/// PalmDOC 的 LZ77 变体：
///   0x00            → 原样输出 0
///   0x01..0x08      → 后面紧跟 n 个字节原样输出
///   0x09..0x7F      → 输出这个字节本身
///   0x80..0xBF      → 两字节：距离 11 位 + 长度 3 位（长度 +3），从已输出区回溯复制
///   0xC0..0xFF      → 空格 + (c ^ 0x80)
fn palmdoc_decompress(src: &[u8]) -> Result<Vec<u8>> {
    let mut out: Vec<u8> = Vec::with_capacity(src.len() * 2);
    let mut i = 0;
    while i < src.len() {
        let c = src[i];
        if c == 0 {
            out.push(0);
            i += 1;
        } else if c <= 8 {
            let n = c as usize;
            i += 1;
            let end = (i + n).min(src.len());
            out.extend_from_slice(&src[i..end]);
            i = end;
        } else if c <= 0x7f {
            out.push(c);
            i += 1;
        } else if c <= 0xbf {
            if i + 1 >= src.len() {
                break;
            }
            let v = ((c as usize) << 8) | src[i + 1] as usize;
            i += 2;
            let dist = (v >> 3) & 0x7ff;
            let len = (v & 7) + 3;
            if dist == 0 || dist > out.len() {
                bail!("PalmDOC 回溯距离越界");
            }
            let start = out.len() - dist;
            for k in 0..len {
                // 这里必须逐字节复制：dist 小于 len 时源区间和自己重叠
                let byte = out[start + k];
                out.push(byte);
            }
        } else {
            out.push(b' ');
            out.push(c ^ 0x80);
            i += 1;
        }
    }
    Ok(out)
}

/// 切掉末尾不完整的 UTF-8 字符，返回 (完整部分, 残片)
fn split_incomplete_utf8(b: &[u8]) -> (&[u8], &[u8]) {
    let n = b.len();
    let floor = n.saturating_sub(3);
    let mut i = n;
    while i > floor {
        i -= 1;
        let c = b[i];
        if c & 0xc0 == 0x80 {
            continue; // 续字节，继续往前找首字节
        }
        let need = if c < 0x80 {
            1
        } else if c >> 5 == 0b110 {
            2
        } else if c >> 4 == 0b1110 {
            3
        } else if c >> 3 == 0b11110 {
            4
        } else {
            return (&b[..], &b[..0]);
        };
        return if n - i < need {
            (&b[..i], &b[i..])
        } else {
            (&b[..], &b[..0])
        };
    }
    (&b[..], &b[..0])
}

/* ---------------- 分章 ---------------- */

/// 按若干 ASCII 标记切段，标记本身丢掉。位置都在字节上找，
/// 因为标记是纯 ASCII，命中点必然落在字符边界上，切片安全。
fn split_on(text: &str, markers: &[&[u8]]) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut cuts: Vec<usize> = Vec::new();
    for m in markers {
        let mut from = 0;
        while let Some(p) = find_ci(bytes, m, from) {
            cuts.push(p);
            from = p + m.len();
        }
    }
    if cuts.is_empty() {
        return vec![text.to_string()];
    }
    cuts.sort_unstable();
    cuts.dedup();

    let mut out = Vec::new();
    let mut start = 0usize;
    for c in cuts {
        if c <= start {
            continue;
        }
        out.push(text[start..c].to_string());
        // 跳过标记所在的整个标签
        start = match text[c..].find('>') {
            Some(e) => c + e + 1,
            None => text.len(),
        };
    }
    if start < text.len() {
        out.push(text[start..].to_string());
    }
    out.retain(|p| !p.trim().is_empty());
    out
}

fn find_ci(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    let n = needle.len();
    if n == 0 || hay.len() < n {
        return None;
    }
    let mut i = from;
    while i + n <= hay.len() {
        if hay[i..i + n].eq_ignore_ascii_case(needle) {
            return Some(i);
        }
        i += 1;
    }
    None
}

/* ---------------- 元信息 ---------------- */

/// EXTH：MOBI 头之后的一张键值表，书名 503、作者 100
fn read_exth(r0: &[u8], header_len: usize, encoding: u32) -> (Option<String>, Option<String>) {
    let at = 16 + header_len;
    if at + 12 > r0.len() || &r0[at..at + 4] != b"EXTH" {
        return (None, None);
    }
    let count = be32(r0, at + 8) as usize;
    let mut p = at + 12;
    let mut title = None;
    let mut author = None;
    for _ in 0..count {
        if p + 8 > r0.len() {
            break;
        }
        let kind = be32(r0, p);
        let size = be32(r0, p + 4) as usize;
        if size < 8 || p + size > r0.len() {
            break;
        }
        let data = &r0[p + 8..p + size];
        let text = || {
            if encoding == 65001 {
                String::from_utf8_lossy(data).to_string()
            } else {
                encoding_rs::WINDOWS_1252.decode(data).0.to_string()
            }
        };
        match kind {
            100 if author.is_none() => author = Some(text()),
            503 if title.is_none() => title = Some(text()),
            _ => {}
        }
        p += size;
    }
    (title, author)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palmdoc_literal_block() {
        // 0x05 表示"后面 5 个字节原样搬"
        let src = [5u8, b'h', b'e', b'l', b'l', b'o'];
        assert_eq!(palmdoc_decompress(&src).unwrap(), b"hello");
    }

    #[test]
    fn palmdoc_back_reference() {
        // 'A''B''C' 然后回溯 dist=3 len=6：0x8000 | (3<<3) | (6-3)
        let code: u16 = 0x8000 | (3 << 3) | 3;
        let src = [b'A', b'B', b'C', (code >> 8) as u8, (code & 0xff) as u8];
        assert_eq!(palmdoc_decompress(&src).unwrap(), b"ABCABCABC");
    }

    #[test]
    fn palmdoc_space_encoding() {
        // 0xC0..0xFF 表示"空格 + (c ^ 0x80)"，用来省掉常见的中英混排空格
        let src = [0xC1u8];
        assert_eq!(palmdoc_decompress(&src).unwrap(), b" A");
    }

    #[test]
    fn palmdoc_keeps_zero_bytes() {
        assert_eq!(palmdoc_decompress(&[0u8, b'a']).unwrap(), vec![0u8, b'a']);
    }

    #[test]
    fn utf8_carry_across_records() {
        // "中" 是 e4 b8 ad，记录边界切在 b8 后面
        let (head, tail) = split_incomplete_utf8(b"abc\xe4\xb8");
        assert_eq!(head, b"abc");
        assert_eq!(tail, b"\xe4\xb8");
        // 完整字符不该被切
        let (head, tail) = split_incomplete_utf8("abc中".as_bytes());
        assert_eq!(head, "abc中".as_bytes());
        assert!(tail.is_empty());
    }

    #[test]
    fn split_on_pagebreak() {
        let text = "<p>one</p><mbp:pagebreak/><p>two</p><MBP:PAGEBREAK /><p>three</p>";
        let pages = split_on(text, &[b"<mbp:pagebreak"]);
        assert_eq!(pages.len(), 3);
        assert_eq!(pages[0], "<p>one</p>");
        assert_eq!(pages[2], "<p>three</p>");
    }
}

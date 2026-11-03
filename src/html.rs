//! 共用的 HTML → Block 解析器。
//!
//! EPUB 的正文是 zip 里的 XHTML，MOBI 的正文是一段内嵌 HTML，
//! 除了"图片从哪来"之外标签处理完全一样，所以解析器只认一种输入：HTML 字符串 + 图片回调。
//! 皮肤、主题包、站点全在这一层之外。

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

use crate::model::{Block, Inline};

/* ---------------- 文本工具 ---------------- */

pub fn unescape(s: &str) -> String {
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

pub fn squash(s: &str) -> String {
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

pub fn attr(e: &BytesStart, key: &[u8]) -> Option<String> {
    for a in e.attributes().flatten() {
        if a.key.local_name().as_ref() == key {
            return Some(unescape(&String::from_utf8_lossy(&a.value)));
        }
    }
    None
}

pub fn decode_text(t: &quick_xml::events::BytesText) -> String {
    unescape(&String::from_utf8_lossy(t))
}

/// 把一段（X）HTML 转成 Block 序列。
///
/// 图片交给调用方处理：EPUB 要从 zip 里按路径取，MOBI 要按 recindex 去记录里取，
/// 两者只有这一步不同，其余标签处理完全共用，所以做成回调而不是复制一份解析器。
pub fn parse(
    html: &str,
    base: &str,
    img: &mut dyn FnMut(&BytesStart, &str) -> Option<Block>,
) -> (Vec<Block>, Option<String>) {
    let mut p = Parser::default();
    let mut reader = Reader::from_str(html);

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let n = e.name().local_name();
                p.on_start(n.as_ref(), &e, base, img);
            }
            Ok(Event::Empty(e)) => {
                let n = e.name().local_name();
                if n.as_ref() == b"img" {
                    if let Some(b) = img(&e, base) {
                        p.flush_pending();
                        p.blocks.push(b);
                    }
                } else {
                    p.on_start(n.as_ref(), &e, base, img);
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
    /// `<cite>` 里的出处：只在 blockquote 内当引用来源，正文里的书名号不当出处
    in_cite: bool,
    cite_buf: String,
    pending_cite: Option<String>,
    /// `<figcaption>` / `<caption>` 的图注、表注，结束后回填到最近的图/表上
    in_cap: bool,
    cap_buf: String,
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
        // 出处 / 图注不能混进正文：它们各自有缓冲区，最后贴到对应的块上
        if self.in_cite {
            self.cite_buf.push_str(s);
            return;
        }
        if self.in_cap {
            self.cap_buf.push_str(s);
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

    fn on_start(
        &mut self,
        n: &[u8],
        e: &BytesStart,
        base: &str,
        img: &mut dyn FnMut(&BytesStart, &str) -> Option<Block>,
    ) {
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
            // 引用块里的 <cite> 是出处；正文里的 <cite>（书名、作品名）当普通文字
            b"cite" if self.open.iter().any(|k| *k == "q") => {
                self.in_cite = true;
                self.cite_buf.clear();
            }
            // 图注 / 表注：先攒着，结束的时候回填到刚才那张图或表上
            b"figcaption" | b"caption" => {
                self.in_cap = true;
                self.cap_buf.clear();
            }
            // 顶层的小字（版权、注释、数据来源）单独成块；段落内的小字仍算正文
            b"small" if self.open.is_empty() => self.open.push("small"),
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
                if let Some(b) = img(e, base) {
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
            b"cite" => {
                if self.in_cite {
                    self.in_cite = false;
                    let c = squash(&self.cite_buf);
                    if !c.is_empty() {
                        self.pending_cite = Some(c);
                    }
                }
            }
            b"figcaption" | b"caption" => {
                if self.in_cap {
                    self.in_cap = false;
                    let c = squash(&self.cap_buf);
                    if c.is_empty() {
                        // 空的 figcaption 就不用往下走了
                    } else {
                        // 回填到最近的图或表上；中间可能隔了空段落，所以最多往回看 3 个
                        let mut done = false;
                        for blk in self.blocks.iter_mut().rev().take(3) {
                            if done {
                                break;
                            }
                            match blk {
                                Block::Img { cap, .. } if cap.is_empty() => {
                                    *cap = c.clone();
                                    done = true;
                                }
                                Block::Table { cap, .. } if cap.is_empty() => {
                                    *cap = c.clone();
                                    done = true;
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            b"small" => {
                self.open.retain(|k| *k != "small");
                if self.open.is_empty() {
                    self.flush_para(Block::small(""));
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
                    self.blocks.push(Block::Table {
                        head,
                        rows,
                        cap: String::new(),
                    });
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
            Block::Q { .. } => {
                // 出处只跟着紧邻的那一条引用走，取完就清掉
                let cite = self.pending_cite.take().unwrap_or_default();
                self.blocks.push(Block::q_cite(x, cite));
            }
            Block::Small { .. } => self.blocks.push(Block::small(x)),
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

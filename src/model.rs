//! 中立内容模型：解析层只产出 Block 序列，渲染层只消费 Block 序列。
//! 皮肤、主题包、排版全部在这一层之外，互不影响。

use serde::{Deserialize, Serialize};

/// 正文的最小单位。前端不直接处理它，渲染层负责转成 HTML。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum Block {
    /// 段落
    P { x: String },
    /// 标题，level 1..=6
    H { level: u8, x: String },
    /// 引用
    Q { x: String },
    /// 代码块
    Code { x: String, lang: String },
    /// 列表
    List { ordered: bool, items: Vec<String> },
    /// 图片，src 是站内绝对路径 /media/...
    Img { src: String, alt: String },
    /// 表格
    Table { head: Vec<String>, rows: Vec<Vec<String>> },
    /// 分隔线
    Hr,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Chapter {
    pub title: String,
    pub blocks: Vec<Block>,
}

impl Chapter {
    /// 章节字数（按字符计，粗略）
    pub fn words(&self) -> usize {
        self.blocks
            .iter()
            .map(|b| match b {
                Block::P { x } | Block::H { x, .. } | Block::Q { x } => x.chars().count(),
                Block::Code { x, .. } => x.chars().count(),
                Block::List { items, .. } => items.iter().map(|i| i.chars().count()).sum(),
                Block::Img { .. } | Block::Hr => 0,
                Block::Table { head, rows } => {
                    head.iter().map(|c| c.chars().count()).sum::<usize>()
                        + rows
                            .iter()
                            .flat_map(|r| r.iter())
                            .map(|c| c.chars().count())
                            .sum::<usize>()
                }
            })
            .sum()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Book {
    pub id: String,
    pub title: String,
    pub author: String,
    #[serde(default)]
    pub intro: String,
    /// 站内路径，如 /media/ab12/cover.jpg
    #[serde(default)]
    pub cover: Option<String>,
    /// epub | txt
    pub format: String,
    pub added_at: i64,
    #[serde(default)]
    pub chapters: Vec<Chapter>,
}

impl Book {
    pub fn words(&self) -> usize {
        self.chapters.iter().map(|c| c.words()).sum()
    }

    pub fn summary(&self, progress: Option<usize>) -> BookSummary {
        BookSummary {
            id: self.id.clone(),
            title: self.title.clone(),
            author: self.author.clone(),
            intro: self.intro.clone(),
            cover: self.cover.clone(),
            format: self.format.clone(),
            added_at: self.added_at,
            chapters: self.chapters.len(),
            words: self.words(),
            progress,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct BookSummary {
    pub id: String,
    pub title: String,
    pub author: String,
    pub intro: String,
    pub cover: Option<String>,
    pub format: String,
    pub added_at: i64,
    pub chapters: usize,
    pub words: usize,
    /// 已读到第几章，None 表示没开始
    pub progress: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChapterInfo {
    pub title: String,
    pub blocks: usize,
    pub words: usize,
}

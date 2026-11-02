//! 本地书库：书籍 JSON、封面/插图、阅读进度、设置。
//! 全部落在数据目录里，不依赖数据库，删掉目录即彻底卸载。

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::RwLock;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::model::{Book, BookSummary};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub port: u16,
    pub default_site: String,
    pub default_pack: String,
    /// Boss Key 跳转地址（留空则跳到本站的"工作页"）
    #[serde(default)]
    pub boss_url: String,
    /// 窗口失焦时暂停阅读量自增
    #[serde(default = "yes")]
    pub pause_on_blur: bool,
    /// 全局唤出热键，形如 "ctrl+alt+f"；留空表示不注册
    #[serde(default = "default_hotkey")]
    pub hotkey: String,
    /// 是否显示通知区托盘图标
    #[serde(default = "yes")]
    pub tray: bool,
    /// 阅读页是否响应快捷键（Esc 不受这个开关影响）
    #[serde(default = "yes")]
    pub reader_keys: bool,
}

fn default_hotkey() -> String {
    "ctrl+alt+f".into()
}

fn yes() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            port: 7317,
            default_site: "csdn".into(),
            default_pack: "cs".into(),
            boss_url: String::new(),
            pause_on_blur: true,
            hotkey: default_hotkey(),
            tray: true,
            reader_keys: true,
        }
    }
}

pub struct Store {
    root: PathBuf,
    settings: RwLock<Settings>,
    progress: RwLock<HashMap<String, usize>>,
}

impl Store {
    pub fn new(root: PathBuf) -> Self {
        let _ = fs::create_dir_all(root.join("books"));
        let _ = fs::create_dir_all(root.join("media"));

        let settings = fs::read_to_string(root.join("settings.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<Settings>(&t).ok())
            .unwrap_or_default();

        let progress = fs::read_to_string(root.join("progress.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<HashMap<String, usize>>(&t).ok())
            .unwrap_or_default();

        Self {
            root,
            settings: RwLock::new(settings),
            progress: RwLock::new(progress),
        }
    }

    pub fn root(&self) -> &PathBuf {
        &self.root
    }

    pub fn media_dir(&self, id: &str) -> PathBuf {
        self.root.join("media").join(id)
    }

    fn book_path(&self, id: &str) -> PathBuf {
        self.root.join("books").join(format!("{id}.json"))
    }

    pub fn save_book(&self, book: &Book) -> Result<()> {
        fs::create_dir_all(self.root.join("books"))?;
        let text = serde_json::to_string(book)?;
        fs::write(self.book_path(&book.id), text)
            .with_context(|| format!("写入书籍失败: {}", book.id))?;
        Ok(())
    }

    pub fn get_book(&self, id: &str) -> Result<Book> {
        let text = fs::read_to_string(self.book_path(id))
            .with_context(|| format!("找不到这本书: {id}"))?;
        Ok(serde_json::from_str(&text)?)
    }

    pub fn has_book(&self, id: &str) -> bool {
        self.book_path(id).exists()
    }

    pub fn list_books(&self) -> Vec<BookSummary> {
        let mut out = Vec::new();
        let Ok(dir) = fs::read_dir(self.root.join("books")) else {
            return out;
        };
        for entry in dir.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e != "json").unwrap_or(true) {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else { continue };
            let Ok(book) = serde_json::from_str::<Book>(&text) else {
                continue;
            };
            let p = self.progress.read().ok().and_then(|m| m.get(&book.id).copied());
            out.push(book.summary(p));
        }
        out.sort_by(|a, b| b.added_at.cmp(&a.added_at));
        out
    }

    pub fn delete_book(&self, id: &str) -> Result<()> {
        let _ = fs::remove_file(self.book_path(id));
        let _ = fs::remove_dir_all(self.media_dir(id));
        if let Ok(mut p) = self.progress.write() {
            p.remove(id);
        }
        self.persist_progress();
        Ok(())
    }

    pub fn progress_of(&self, id: &str) -> Option<usize> {
        self.progress.read().ok().and_then(|m| m.get(id).copied())
    }

    pub fn set_progress(&self, id: &str, chapter: usize) {
        if let Ok(mut p) = self.progress.write() {
            p.insert(id.to_string(), chapter);
        }
        self.persist_progress();
    }

    fn persist_progress(&self) {
        if let Ok(p) = self.progress.read() {
            if let Ok(text) = serde_json::to_string(&*p) {
                let _ = fs::write(self.root.join("progress.json"), text);
            }
        }
    }

    pub fn settings(&self) -> Settings {
        self.settings
            .read()
            .map(|s| s.clone())
            .unwrap_or_default()
    }

    pub fn update_settings(&self, f: impl FnOnce(&mut Settings)) -> Settings {
        let next = {
            let mut guard = match self.settings.write() {
                Ok(g) => g,
                Err(e) => e.into_inner(),
            };
            f(&mut guard);
            guard.clone()
        };
        if let Ok(text) = serde_json::to_string_pretty(&next) {
            let _ = fs::write(self.root.join("settings.json"), text);
        }
        next
    }
}

//! 本地书库：书籍 JSON、封面/插图、阅读进度、设置。
//! 全部落在数据目录里，不依赖数据库，删掉目录即彻底卸载。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use anyhow::{bail, Context, Result};
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
    /// 书库界面外观：auto（跟随系统）/ light / dark
    #[serde(default = "default_appearance")]
    pub appearance: String,
}

fn default_hotkey() -> String {
    "ctrl+alt+f".into()
}

fn yes() -> bool {
    true
}

fn default_appearance() -> String {
    "auto".into()
}

/// 外观只有三种合法取值。settings.json 被手改成别的值时回落到"跟随系统"，
/// 否则前端拿到无法识别的值会停在浅色，用户以为自己选的深色没生效。
pub fn appearance_ok(v: &str) -> bool {
    matches!(v, "auto" | "light" | "dark")
}

/* ---------------- 数据目录 ---------------- */

/// 默认数据目录：Windows 在 `%APPDATA%\TouchFish`，其余平台在 `~/.local/share/touch-fish`
pub fn default_data_dir() -> PathBuf {
    #[cfg(windows)]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return PathBuf::from(appdata).join("TouchFish");
        }
    }
    #[cfg(not(windows))]
    {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(".local/share/touch-fish");
        }
    }
    PathBuf::from("./data")
}

/// 记录"数据目录换到哪了"的指针文件。刻意放在数据目录之外：
/// 旧目录整个删掉也不会把新位置弄丢。
fn pointer_path() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var("APPDATA")
            .ok()
            .filter(|p| !p.trim().is_empty())
            .map(|p| PathBuf::from(p).join("TouchFish-location.json"))
    }
    #[cfg(not(windows))]
    {
        let base = std::env::var("XDG_CONFIG_HOME")
            .ok()
            .filter(|p| !p.trim().is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var("HOME")
                    .ok()
                    .map(|h| PathBuf::from(h).join(".config"))
            });
        base.map(|b| b.join("touch-fish").join("location.json"))
    }
}

#[derive(Serialize, Deserialize)]
struct DataLocation {
    data_dir: String,
}

fn load_data_dir() -> Option<PathBuf> {
    let text = fs::read_to_string(pointer_path()?).ok()?;
    let loc: DataLocation = serde_json::from_str(&text).ok()?;
    let p = loc.data_dir.trim();
    if p.is_empty() {
        return None;
    }
    Some(PathBuf::from(p))
}

/// 下一次启动该用哪个目录：环境变量 > 指针文件 > 默认目录
pub fn data_dir() -> PathBuf {
    if let Ok(p) = std::env::var("TOUCHFISH_DATA") {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    if let Some(p) = load_data_dir() {
        // 移动硬盘没插上时，盘符这一层就不存在；先退回默认目录把程序跑起来，
        // 指针保留着，盘接回来下次启动仍然回到原处。
        if p.ancestors().any(|a| a.exists()) {
            return p;
        }
        eprintln!(
            "[warn] 数据目录 {} 暂时不可用，这次先用默认目录",
            p.display()
        );
    }
    default_data_dir()
}

/// 迁移成功后记下新位置。写不进去就让调用方报错——宁可这次不切，
/// 也别让下次启动又指回旧目录。
pub fn save_data_dir(dir: &Path) -> Result<()> {
    let Some(path) = pointer_path() else {
        bail!("找不到保存配置的位置（APPDATA / HOME 未设置）");
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let loc = DataLocation {
        data_dir: dir.display().to_string(),
    };
    fs::write(path, serde_json::to_string_pretty(&loc)?)?;
    Ok(())
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
            appearance: default_appearance(),
        }
    }
}

pub struct Store {
    /// 迁移数据目录时会整体换掉，所有读写都走 root() 取当前值
    root: RwLock<PathBuf>,
    settings: RwLock<Settings>,
    progress: RwLock<HashMap<String, usize>>,
}

impl Store {
    pub fn new(root: PathBuf) -> Self {
        let _ = fs::create_dir_all(root.join("books"));
        let _ = fs::create_dir_all(root.join("media"));

        let mut settings = fs::read_to_string(root.join("settings.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<Settings>(&t).ok())
            .unwrap_or_default();
        if !appearance_ok(&settings.appearance) {
            settings.appearance = default_appearance();
        }

        let progress = fs::read_to_string(root.join("progress.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<HashMap<String, usize>>(&t).ok())
            .unwrap_or_default();

        Self {
            root: RwLock::new(root),
            settings: RwLock::new(settings),
            progress: RwLock::new(progress),
        }
    }

    /// 当前数据目录（迁移后会变）
    pub fn root(&self) -> PathBuf {
        self.root
            .read()
            .map(|p| p.clone())
            .unwrap_or_else(|_| default_data_dir())
    }

    pub fn media_dir(&self, id: &str) -> PathBuf {
        self.root().join("media").join(id)
    }

    /// 自定义应用图标放这里（brand/logo.png 等），删掉即回默认
    pub fn brand_dir(&self) -> PathBuf {
        self.root().join("brand")
    }

    fn book_path(&self, id: &str) -> PathBuf {
        self.root().join("books").join(format!("{id}.json"))
    }

    pub fn save_book(&self, book: &Book) -> Result<()> {
        fs::create_dir_all(self.root().join("books"))?;
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
        let Ok(dir) = fs::read_dir(self.root().join("books")) else {
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
                let _ = fs::write(self.root().join("progress.json"), text);
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
            let _ = fs::write(self.root().join("settings.json"), text);
        }
        next
    }

    /// 只复制，不动当前状态：目标必须不存在或是空目录。
    /// 复制中途失败时旧目录仍然完整，调用方可以放心报错。
    pub fn copy_data_to(&self, target: &Path) -> Result<()> {
        let from = self.root();
        if from.as_path() == target {
            bail!("新目录和当前目录是同一个");
        }
        if target.starts_with(&from) {
            bail!("新目录不能放在当前数据目录里面");
        }
        if target.exists() {
            if !target.is_dir() {
                bail!("目标位置已经有一个同名文件，换一个目录");
            }
            if !dir_is_empty(target)? {
                bail!("目标目录不是空的，请换一个空目录");
            }
        }
        fs::create_dir_all(target)?;
        // 目标落在当前数据目录里面的话，复制时会把正在写入的东西也算进去
        if let (Ok(f), Ok(t)) = (from.canonicalize(), target.canonicalize()) {
            if t.starts_with(&f) {
                bail!("新目录不能放在当前数据目录里面");
            }
        }

        for name in ["books", "media", "brand"] {
            let src = from.join(name);
            if src.is_dir() {
                copy_tree(&src, &target.join(name))?;
            }
        }
        for name in ["settings.json", "progress.json", "window.json"] {
            let src = from.join(name);
            if src.is_file() {
                fs::copy(&src, target.join(name))
                    .with_context(|| format!("复制 {} 失败", src.display()))?;
            }
        }
        Ok(())
    }

    /// 迁移：复制 → 记下新位置 → 运行中的读写切过去 → 清理旧目录里的数据。
    /// `webview` 缓存不搬——可以重建，而且运行时经常被 WebView2 占着。
    pub fn migrate_to(&self, target: &Path) -> Result<()> {
        let from = self.root();
        self.copy_data_to(target)?;
        save_data_dir(target)?;
        if !self.switch_root(target) {
            bail!("切换数据目录失败，请重启后再试");
        }
        // 复制期间内存里可能又记了进度，切过去之后按最新的再写一遍
        self.persist_progress();
        self.update_settings(|_| {});

        for name in ["books", "media", "brand"] {
            let _ = fs::remove_dir_all(from.join(name));
        }
        for name in ["settings.json", "progress.json", "window.json"] {
            let _ = fs::remove_file(from.join(name));
        }
        Ok(())
    }

    fn switch_root(&self, target: &Path) -> bool {
        match self.root.write() {
            Ok(mut r) => {
                *r = target.to_path_buf();
                true
            }
            Err(_) => false,
        }
    }
}

/// 系统自己塞进来的文件不算"占用"：几乎每个 Windows 文件夹都可能有 desktop.ini
fn dir_is_empty(dir: &Path) -> Result<bool> {
    for entry in fs::read_dir(dir)? {
        let name = entry?.file_name().to_string_lossy().to_lowercase();
        if matches!(name.as_str(), "desktop.ini" | "thumbs.db" | ".ds_store") {
            continue;
        }
        return Ok(false);
    }
    Ok(true)
}

fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let to = dst.join(entry.file_name());
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_tree(&entry.path(), &to)?;
        } else if ty.is_file() {
            fs::copy(entry.path(), &to)
                .with_context(|| format!("复制 {} 失败", entry.path().display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_settings_json_without_appearance_is_auto() {
        // 老版本 settings.json 里没有 appearance 字段，必须还能读，且默认跟随系统
        let old = r#"{"port":7317,"default_site":"csdn","default_pack":"cs"}"#;
        let s: Settings = serde_json::from_str(old).expect("老设置应当能反序列化");
        assert_eq!(s.appearance, "auto");
    }

    #[test]
    fn appearance_only_accepts_three_values() {
        for v in ["auto", "light", "dark"] {
            assert!(appearance_ok(v), "{v} 应当合法");
        }
        for v in ["", "AUTO", "darkish", "system"] {
            assert!(!appearance_ok(v), "{v} 应当被拒绝");
        }
    }

    #[test]
    fn migrate_copies_files_and_switches_root() {
        let base = std::env::temp_dir().join(format!("tf-migrate-{}", std::process::id()));
        let from = base.join("old");
        let to = base.join("new");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(from.join("books")).unwrap();
        fs::create_dir_all(from.join("media/b1")).unwrap();
        fs::write(from.join("books/b1.json"), r#"{"id":"b1"}"#).unwrap();
        fs::write(from.join("media/b1/cover.png"), b"png").unwrap();
        fs::write(from.join("progress.json"), r#"{"b1":3}"#).unwrap();

        let store = Store::new(from.clone());
        store.copy_data_to(&to).expect("复制应当成功");
        assert!(to.join("books/b1.json").is_file());
        assert!(to.join("media/b1/cover.png").is_file());
        assert_eq!(store.root(), from, "只复制时不该提前切换");

        assert!(store.switch_root(&to));
        assert_eq!(store.root(), to);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn migrate_rejects_non_empty_target() {
        let base = std::env::temp_dir().join(format!("tf-migrate-busy-{}", std::process::id()));
        let from = base.join("old");
        let to = base.join("new");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&from).unwrap();
        fs::create_dir_all(&to).unwrap();
        fs::write(to.join("keep.txt"), b"x").unwrap();

        let store = Store::new(from);
        let err = store.copy_data_to(&to).expect_err("非空目录应当被拒绝");
        assert!(err.to_string().contains("不是空的"));
        let _ = fs::remove_dir_all(&base);
    }
}

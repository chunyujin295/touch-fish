pub mod render;

use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use include_dir::{include_dir, Dir};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::library::Store;
use crate::model::ChapterInfo;

pub static ASSETS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/assets");

/// 读资源：开发模式（TOUCHFISH_DEV=1）直接读磁盘，改 CSS / JSON 不用重新编译；
/// 正式运行读打进二进制里的副本，单文件即可分发。
pub fn asset_text(path: &str) -> String {
    if dev_mode() {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join(path);
        if let Ok(s) = std::fs::read_to_string(p) {
            return s;
        }
    }
    asset_str(path).unwrap_or("").to_string()
}

fn dev_mode() -> bool {
    std::env::var("TOUCHFISH_DEV").map(|v| v != "0").unwrap_or(false)
}

pub struct App {
    pub store: Arc<Store>,
}

pub type Shared = Arc<App>;

pub fn asset_str(path: &str) -> Option<&'static str> {
    ASSETS.get_file(path).and_then(|f| f.contents_utf8())
}

pub fn router(store: Arc<Store>) -> Router {
    let app = Arc::new(App { store });
    Router::new()
        .route("/", get(index))
        .route("/assets/*path", get(asset))
        .route("/media/:id/:file", get(media))
        .route("/api/books", get(list_books))
        .route("/api/books/:id", get(book_detail).delete(delete_book))
        .route(
            "/api/import",
            post(import).layer(DefaultBodyLimit::max(256 * 1024 * 1024)),
        )
        .route("/api/progress/:id", post(set_progress))
        .route("/api/settings", get(get_settings).post(set_settings))
        .route("/api/sites", get(sites))
        .route("/api/packs", get(packs))
        .route("/r/:id/:idx", get(reader))
        .with_state(app)
}

/* ---------------- 静态资源 ---------------- */

async fn index() -> Response {
    let html = asset_text("app/index.html");
    if html.is_empty() {
        return (StatusCode::NOT_FOUND, "app/index.html 缺失").into_response();
    }
    Html(html).into_response()
}

fn mime_for(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" => "application/javascript; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

async fn asset(Path(path): Path<String>) -> Response {
    if path.contains("..") {
        return (StatusCode::BAD_REQUEST, "bad path").into_response();
    }
    let (bytes, mime) = if dev_mode() {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join(&path);
        match std::fs::read(&p) {
            Ok(b) => (b, mime_for(&path)),
            Err(_) => return (StatusCode::NOT_FOUND, "not found").into_response(),
        }
    } else {
        match ASSETS.get_file(&path) {
            Some(f) => (f.contents().to_vec(), mime_for(&path)),
            None => return (StatusCode::NOT_FOUND, "not found").into_response(),
        }
    };
    (
        [
            (header::CONTENT_TYPE, mime),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        bytes,
    )
        .into_response()
}

async fn media(
    State(app): State<Shared>,
    Path((id, file)): Path<(String, String)>,
) -> Response {
    if file.contains("..") || file.contains('/') || file.contains('\\') {
        return (StatusCode::BAD_REQUEST, "bad path").into_response();
    }
    let path: PathBuf = app.store.media_dir(&id).join(&file);
    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, mime_for(&file)),
                (header::CACHE_CONTROL, "public, max-age=31536000"),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/* ---------------- 书库 API ---------------- */

fn bad(msg: impl ToString) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, msg.to_string())
}

async fn list_books(State(app): State<Shared>) -> Json<Value> {
    Json(json!(app.store.list_books()))
}

async fn book_detail(
    State(app): State<Shared>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let book = app
        .store
        .get_book(&id)
        .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;
    let chapters: Vec<ChapterInfo> = book
        .chapters
        .iter()
        .map(|c| ChapterInfo {
            title: c.title.clone(),
            blocks: c.blocks.len(),
            words: c.words(),
        })
        .collect();
    Ok(Json(json!({
        "id": book.id,
        "title": book.title,
        "author": book.author,
        "intro": book.intro,
        "cover": book.cover,
        "format": book.format,
        "chapters": chapters,
        "progress": app.store.progress_of(&book.id),
    })))
}

async fn delete_book(
    State(app): State<Shared>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (StatusCode, String)> {
    app.store.delete_book(&id).map_err(bad)?;
    Ok(Json(json!({ "ok": true })))
}

async fn import(
    State(app): State<Shared>,
    mut mp: Multipart,
) -> Result<Json<Value>, (StatusCode, String)> {
    while let Some(field) = mp.next_field().await.map_err(bad)? {
        if field.name() != Some("file") {
            continue;
        }
        let filename = field
            .file_name()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "book.epub".into());
        let ext = filename
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_lowercase();
        let supported = ["epub", "mobi", "prc", "azw", "txt"];
        if !supported.contains(&ext.as_str()) {
            return Err(bad(format!(
                "暂不支持 .{ext}，目前支持 epub / mobi / azw / txt"
            )));
        }
        let bytes = field.bytes().await.map_err(bad)?;
        if bytes.is_empty() {
            return Err(bad("文件是空的"));
        }

        let mut hasher = sha2::Sha256::new();
        use sha2::Digest;
        hasher.update(&bytes);
        let digest = hasher.finalize();
        let id: String = digest[..12]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();

        if app.store.has_book(&id) {
            let book = app.store.get_book(&id).map_err(bad)?;
            return Ok(Json(json!({
                "ok": true,
                "duplicated": true,
                "book": book.summary(app.store.progress_of(&id)),
            })));
        }

        let tmp_dir = app.store.root().join("tmp");
        std::fs::create_dir_all(&tmp_dir).map_err(bad)?;
        let tmp = tmp_dir.join(format!("{id}.{ext}"));
        std::fs::write(&tmp, &bytes).map_err(bad)?;

        let media_dir = app.store.media_dir(&id);
        // 坏书不该把整条连接打崩：解析层万一 panic，也要变成一条正常的 400
        let parsed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if ext == "epub" {
                crate::epub::parse(&tmp, &id, &media_dir)
            } else if ext == "mobi" || ext == "prc" || ext == "azw" {
                crate::mobi::parse(&tmp, &id, &media_dir)
            } else {
                crate::txtbook::parse(&tmp, &id, &filename)
            }
        }));
        let _ = std::fs::remove_file(&tmp);

        let mut book = match parsed {
            Ok(r) => r.map_err(bad)?,
            Err(_) => return Err(bad("这本书解析到一半出错了，可能文件结构异常")),
        };
        if book.title.trim().is_empty() {
            book.title = filename.clone();
        }
        app.store.save_book(&book).map_err(bad)?;

        return Ok(Json(json!({
            "ok": true,
            "book": book.summary(None),
        })));
    }
    Err(bad("没有收到文件"))
}

#[derive(Deserialize)]
struct ProgressBody {
    chapter: usize,
}

async fn set_progress(
    State(app): State<Shared>,
    Path(id): Path<String>,
    Json(body): Json<ProgressBody>,
) -> Json<Value> {
    app.store.set_progress(&id, body.chapter);
    Json(json!({ "ok": true }))
}

async fn get_settings(State(app): State<Shared>) -> Json<Value> {
    let mut v = serde_json::to_value(app.store.settings()).unwrap_or_else(|_| json!({}));
    if let Some(obj) = v.as_object_mut() {
        obj.insert(
            "data_dir".into(),
            json!(app.store.root().display().to_string()),
        );
        obj.insert("version".into(), json!(env!("CARGO_PKG_VERSION")));
    }
    Json(v)
}

/// 热键写法的合法性校验。没有 GUI 的构建里只做非空判断
/// （解析器在 desktop.rs，它跟着 gui feature 一起开关）。
fn hotkey_valid(spec: &str) -> bool {
    #[cfg(feature = "gui")]
    {
        crate::desktop::hotkey_ok(spec)
    }
    #[cfg(not(feature = "gui"))]
    {
        !spec.trim().is_empty()
    }
}

async fn set_settings(
    State(app): State<Shared>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let next = app.store.update_settings(|s| {
        if let Some(v) = body.get("port").and_then(|v| v.as_u64()) {
            if v > 0 && v < 65536 {
                s.port = v as u16;
            }
        }
        if let Some(v) = body.get("default_site").and_then(|v| v.as_str()) {
            s.default_site = v.to_string();
        }
        if let Some(v) = body.get("default_pack").and_then(|v| v.as_str()) {
            s.default_pack = v.to_string();
        }
        if let Some(v) = body.get("boss_url").and_then(|v| v.as_str()) {
            s.boss_url = v.to_string();
        }
        if let Some(v) = body.get("pause_on_blur").and_then(|v| v.as_bool()) {
            s.pause_on_blur = v;
        }
        if let Some(v) = body.get("hotkey").and_then(|v| v.as_str()) {
            // 热键写法不校验的话，拼错的字符串会让热键静默失效，用户以为设置没生效
            if v.trim().is_empty() || hotkey_valid(v) {
                s.hotkey = v.trim().to_string();
            }
        }
        if let Some(v) = body.get("tray").and_then(|v| v.as_bool()) {
            s.tray = v;
        }
        if let Some(v) = body.get("reader_keys").and_then(|v| v.as_bool()) {
            s.reader_keys = v;
        }
    });
    let mut v = serde_json::to_value(next).unwrap_or_else(|_| json!({}));
    if let Some(obj) = v.as_object_mut() {
        obj.insert(
            "data_dir".into(),
            json!(app.store.root().display().to_string()),
        );
        obj.insert("version".into(), json!(env!("CARGO_PKG_VERSION")));
    }
    Json(v)
}

async fn sites() -> Json<Value> {
    let v: Value = serde_json::from_str(&asset_text("data/sites.json"))
        .unwrap_or_else(|_| json!([]));
    let slim: Vec<Value> = v
        .as_array()
        .map(|a| {
            a.iter()
                .map(|s| {
                    json!({
                        "id": s.get("id"),
                        "name": s.get("name"),
                        "layout": s.get("layout"),
                        "host": s.get("host"),
                        "accent": s.get("accent"),
                        "desc": s.get("desc"),
                        "ready": s.get("ready").and_then(|x| x.as_bool()).unwrap_or(true),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Json(json!(slim))
}

async fn packs() -> Json<Value> {
    let v: Value = serde_json::from_str(&asset_text("data/packs.json"))
        .unwrap_or_else(|_| json!([]));
    let slim: Vec<Value> = v
        .as_array()
        .map(|a| {
            a.iter()
                .map(|p| {
                    json!({
                        "id": p.get("id"),
                        "name": p.get("name"),
                        "desc": p.get("desc"),
                        "accent": p.get("accent"),
                        "terms": p.get("terms"),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Json(json!(slim))
}

/* ---------------- 阅读页 ---------------- */

#[derive(Deserialize)]
struct ReaderQuery {
    site: Option<String>,
    pack: Option<String>,
    /// 文章分页页码，从 1 开始；不传就是第一页
    p: Option<usize>,
}

async fn reader(
    State(app): State<Shared>,
    Path((id, idx)): Path<(String, usize)>,
    Query(q): Query<ReaderQuery>,
) -> Response {
    let Ok(book) = app.store.get_book(&id) else {
        return (StatusCode::NOT_FOUND, "书不存在").into_response();
    };
    let settings = app.store.settings();
    let site = q.site.unwrap_or(settings.default_site);
    let pack = q.pack.unwrap_or(settings.default_pack);

    let total = book.chapters.len().max(1);
    let idx = idx.min(total - 1);

    let html = render::page(
        &book,
        idx,
        &site,
        &pack,
        &settings.boss_url,
        q.p.unwrap_or(1),
        settings.reader_keys,
    );
    app.store.set_progress(&id, idx);

    Html(html).into_response()
}

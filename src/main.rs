#![cfg_attr(
    all(windows, not(debug_assertions), feature = "gui"),
    windows_subsystem = "windows"
)]

#[cfg(feature = "gui")]
mod desktop;
mod epub;
mod html;
mod library;
mod media;
mod mobi;
mod model;
mod noise;
mod txtbook;
mod web;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 桌面壳注册给网页层的两个钩子。
///
/// 书库页面里的「在浏览器中打开」和改热键，都需要窗口配合，但 web 层不该知道
/// tao / wry 的存在（纯网页模式根本没有窗口）。桌面壳启动时把自己的能力注册进来，
/// 没注册就说明当前是 `--server` 模式，调用方按"没有窗口"处理即可。
pub(crate) struct UiHooks {
    /// 把窗口收起来（本来就没开时什么都不做）
    pub hide_window: Box<dyn Fn() + Send + Sync>,
    /// 换一个全局热键，立即生效
    pub set_hotkey: Box<dyn Fn(String) + Send + Sync>,
}

pub(crate) static UI_HOOKS: OnceLock<UiHooks> = OnceLock::new();

/// 当前 Unix 时间戳（秒）
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 用系统默认浏览器打开地址。桌面包里的托盘菜单也要走这个入口，
/// 否则"打开网页页面"就得再抄一份 CREATE_NO_WINDOW 的 cmd 调用。
pub(crate) fn open_browser(url: &str) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // 别让 cmd 的黑色控制台闪一下——尤其在桌面窗口里点"在浏览器中打开"时很显眼
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = std::process::Command::new("cmd")
            .args(["/c", "start", "", url])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(url).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
}

/// 这个端口上是不是已经有一个 TouchFish 在跑了？
/// 双击两次图标不该起两个进程、也不该报"端口被占用"，直接连上已有的那个。
fn probe(port: u16) -> bool {
    use std::io::{Read, Write};

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut s) = std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(250)) else {
        return false;
    };
    s.set_read_timeout(Some(Duration::from_millis(500))).ok();
    let req = format!("GET /api/settings HTTP/1.0\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    if s.write_all(req.as_bytes()).is_err() {
        return false;
    }
    let mut buf = [0u8; 1024];
    let mut got = String::new();
    while let Ok(n) = s.read(&mut buf) {
        if n == 0 {
            break;
        }
        got.push_str(&String::from_utf8_lossy(&buf[..n]));
        if got.len() > 4096 {
            break;
        }
    }
    got.contains("\"version\"")
}

fn router_blocking(
    store: Arc<library::Store>,
    port: u16,
    web_mode: bool,
    ready: impl FnOnce(),
) -> anyhow::Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    rt.block_on(async move {
        let app = web::router(store, port, web_mode);
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let listener = tokio::net::TcpListener::bind(addr).await?;
        ready();
        axum::serve(listener, app).await?;
        Ok(())
    })
}

/// 后台线程里起服务，主线程留给桌面窗口（tao 事件循环必须占用主线程）
fn spawn_server(store: Arc<library::Store>, port: u16) -> anyhow::Result<()> {
    let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
    std::thread::Builder::new()
        .name("touchfish-http".into())
        .spawn(move || {
            let res = router_blocking(store, port, false, || {
                let _ = tx.send(Ok(()));
            });
            if let Err(e) = res {
                eprintln!("本地服务启动失败：{e}");
            }
        })?;
    match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => anyhow::bail!(e),
        Err(_) => anyhow::bail!("本地服务启动超时"),
    }
}

/// 应用版本号。唯一来源是 `scripts/version.txt`，编译期嵌进来：
/// 改完那个文件直接 build 就生效，不用碰 Cargo.toml（它的 version 只当 crate 元数据）。
pub(crate) fn version() -> &'static str {
    include_str!("../scripts/version.txt").trim()
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |f: &str| args.iter().any(|a| a == f);

    // 目录位置可能来自 TOUCHFISH_DATA，也可能是设置里迁移过、记在指针文件里的
    let data = library::data_dir();
    std::fs::create_dir_all(&data)?;
    let store = Arc::new(library::Store::new(data.clone()));

    let port = std::env::var("TF_PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or_else(|| store.settings().port);
    let url = std::env::var("TF_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
        .unwrap_or_else(|| format!("http://127.0.0.1:{port}/"));

    // 纯服务模式：给浏览器用、给调试用、给无 GUI 的环境用
    if has("--server") || has("--no-gui") {
        // 已经有一个实例（桌面窗口或上一次网页版）：只把浏览器叫起来就退出，
        // 这样双击启动脚本两次也不会报"端口被占用"
        if probe(port) {
            if has("--open") {
                open_browser(&url);
            }
            return Ok(());
        }
        println!();
        println!("  TouchFish  v{}", version());
        println!("  数据目录  {}", data.display());
        println!("  书库界面  {url}");
        println!("  按 Ctrl+C 退出");
        println!();
        let hook_url = url.clone();
        return router_blocking(store, port, true, move || {
            if has("--open") {
                open_browser(&hook_url);
            }
        });
    }

    let reused = probe(port);
    // 热键与托盘在进程启动时注册，改了要重启；这里先读一次快照
    let (hotkey, tray) = {
        let s = store.settings();
        (s.hotkey.clone(), s.tray)
    };
    if !reused {
        spawn_server(store, port)?;
    }

    #[cfg(feature = "gui")]
    {
        println!();
        println!("  TouchFish  v{}", version());
        println!("  数据目录  {}", data.display());
        if reused {
            println!("  复用已在运行的实例  {url}");
        } else {
            println!("  本地服务  {url}");
        }
        println!();
        return desktop::run(
            &url,
            data.join("webview"),
            data.join("window.json"),
            hotkey,
            tray,
        );
    }

    #[cfg(not(feature = "gui"))]
    {
        println!("这个构建没有桌面窗口，只提供网页界面：{url}");
        let hook_url = url.clone();
        // 服务已经在后台线程里跑着了，主线程只需要停住
        if has("--open") {
            open_browser(&hook_url);
        }
        loop {
            std::thread::sleep(Duration::from_secs(3600));
        }
    }
}

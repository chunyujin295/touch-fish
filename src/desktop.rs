//! 桌面壳：一个真正的原生窗口 + WebView2，指向进程内起的本地服务。
//!
//! 为什么不用 Tauri：Tauri 的价值在于前端工程化（node、打包、插件），
//! 而这里的界面是手写 HTML/CSS、服务端直出，没有构建步骤。
//! 拿 wry（Tauri 的底层）直接开窗，依赖少一个数量级，编译从十几分钟变成两分钟。
//!
//! 关窗与隐藏的分工：
//! - 点右上角 X → 真的退出（用户以为它关了，那就关掉）
//! - Boss Key（页面跳 about:blank）→ 只隐藏窗口，进程留着，热键一按就回来
//! 这条设计的理由：摸鱼时最怕的是"关掉之后手忙脚乱重新启动"，
//! 隐藏的窗口不进任务栏、不进 Alt+Tab，比退出更安全也更快恢复。

use std::path::{Path, PathBuf};

use anyhow::Result;
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use tao::dpi::{PhysicalPosition, PhysicalSize};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::window::{Icon, Window, WindowBuilder};
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use wry::{WebContext, WebViewBuilder};

/// 唤出 / 收起的全局热键：Ctrl+Alt+F（F = Fish）
const HOTKEY_TEXT: &str = "Ctrl+Alt+F";

/// 跨线程把事件丢回主线程：WebView 回调、托盘、热键都不在 UI 线程上
pub enum UserEvent {
    /// 页面标题变了，窗口标题要跟着变——浏览器就是这样，我们得一样
    Title(String),
    /// 页面跳到了 about:blank（Boss Key），把窗口收起来
    Boss,
    /// 热键 / 托盘左键：可见且在前台就收起，否则唤出
    Toggle,
    /// 托盘菜单「退出」
    Quit,
}

const ICON_RGBA: &[u8] = include_bytes!("../assets/app/icon.rgba");
const ICON_SIZE: u32 = 64;

fn icon() -> Option<Icon> {
    Icon::from_rgba(ICON_RGBA.to_vec(), ICON_SIZE, ICON_SIZE).ok()
}

#[derive(serde::Serialize, serde::Deserialize)]
struct WinState {
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    maximized: bool,
}

impl Default for WinState {
    fn default() -> Self {
        Self {
            x: 120,
            y: 80,
            w: 1320,
            h: 880,
            maximized: false,
        }
    }
}

fn load_state(path: &Path) -> WinState {
    let st = std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<WinState>(&s).ok())
        .unwrap_or_default();
    // 上次在副屏、这次副屏拔了的情况：给个下限，别把窗口丢到看不见的地方
    if st.w < 800 || st.h < 560 || st.x < -6000 || st.y < -6000 {
        return WinState::default();
    }
    st
}

fn save_to(window: &Window, path: &Path) {
    let prev = load_state(path);
    // 最大化时 inner_size 是最大化后的尺寸，直接存下来下次就成了个铺满的普通窗口，
    // 所以最大化只记一个标记，宽高沿用上一次的普通窗口尺寸。
    let st = if window.is_maximized() {
        WinState {
            maximized: true,
            ..prev
        }
    } else {
        let pos = window
            .outer_position()
            .unwrap_or(PhysicalPosition::new(prev.x, prev.y));
        let size = window.inner_size();
        WinState {
            x: pos.x,
            y: pos.y,
            w: size.width,
            h: size.height,
            maximized: false,
        }
    };
    if let Ok(s) = serde_json::to_string_pretty(&st) {
        let _ = std::fs::write(path, s);
    }
}

/// 收起窗口：先存位置，再隐藏。
/// 隐藏之后任务栏、Alt+Tab 里都没有它，但 WebView 还活着，页面滚动位置不会丢。
fn hide_window(window: &Window, state_path: &Path) {
    save_to(window, state_path);
    window.set_visible(false);
}

fn show_window(window: &Window) {
    window.set_visible(true);
    window.set_minimized(false);
    window.set_focus();
}

fn env_flag(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(v) => !(v == "0" || v.eq_ignore_ascii_case("false")),
        Err(_) => default,
    }
}

/// 托盘图标：左键切换、右键菜单。
/// 换来的好处是"忘了热键也能找回来"，代价是通知区多一个图标——可以用 TOUCHFISH_TRAY=0 关掉。
fn build_tray(proxy: EventLoopProxy<UserEvent>) -> Option<tray_icon::TrayIcon> {
    let menu = Menu::new();
    let toggle = MenuItem::with_id("toggle", format!("显示 / 收起  ({HOTKEY_TEXT})"), true, None);
    let quit = MenuItem::with_id("quit", "退出", true, None);
    menu.append_items(&[&toggle, &quit]).ok()?;

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip(format!("TouchFish · {HOTKEY_TEXT}"))
        .with_icon(tray_icon::Icon::from_rgba(ICON_RGBA.to_vec(), ICON_SIZE, ICON_SIZE).ok()?)
        .build()
        .ok()?;

    let click_proxy = proxy.clone();
    TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
        let hit = matches!(
            e,
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } | TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            }
        );
        if hit {
            let _ = click_proxy.send_event(UserEvent::Toggle);
        }
    }));

    let menu_proxy = proxy;
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        let ev = match e.id().0.as_str() {
            "quit" => UserEvent::Quit,
            _ => UserEvent::Toggle,
        };
        let _ = menu_proxy.send_event(ev);
    }));

    Some(tray)
}

/// 注册全局热键。注册失败（被别的软件占了）时返回 None，
/// 调用方据此把 Boss Key 退回"直接退出"，免得窗口收起来之后再也叫不出来。
fn build_hotkey(proxy: EventLoopProxy<UserEvent>) -> Option<GlobalHotKeyManager> {
    let manager = GlobalHotKeyManager::new().ok()?;
    let hk = HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyF);
    if manager.register(hk).is_err() {
        return None;
    }
    GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
        if e.state() == HotKeyState::Pressed {
            let _ = proxy.send_event(UserEvent::Toggle);
        }
    }));
    Some(manager)
}

/// 起窗口并阻塞在主线程。`url` 是已经就绪的本地地址。
pub fn run(url: &str, webview_dir: PathBuf, state_path: PathBuf) -> Result<()> {
    let st = load_state(&state_path);

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy: EventLoopProxy<UserEvent> = event_loop.create_proxy();

    let window = WindowBuilder::new()
        // 初始标题先给个不显眼的，页面一加载就会被真实标题顶掉
        .with_title("技术文档")
        .with_inner_size(PhysicalSize::new(st.w, st.h))
        .with_position(PhysicalPosition::new(st.x, st.y))
        .with_min_inner_size(PhysicalSize::new(940, 620))
        .with_window_icon(icon())
        .build(&event_loop)?;
    if st.maximized {
        window.set_maximized(true);
    }

    // WebView2 的缓存目录塞进数据目录，别在 exe 旁边拉一地文件
    std::fs::create_dir_all(&webview_dir).ok();
    let mut ctx = WebContext::new(Some(webview_dir));

    // 热键先注册：它决定了 Boss Key 是"收起"还是"退出"
    let hotkey_guard = build_hotkey(event_loop.create_proxy());
    let can_restore = hotkey_guard.is_some();
    if !can_restore && cfg!(windows) {
        // 没有热键就没人能把窗口叫回来，只能退回老行为
        eprintln!("[warn] 全局热键 {HOTKEY_TEXT} 注册失败（可能被其他软件占用），Esc 将直接退出");
    }

    let tray_guard = if env_flag("TOUCHFISH_TRAY", true) {
        build_tray(event_loop.create_proxy())
    } else {
        None
    };

    #[cfg(debug_assertions)]
    eprintln!(
        "[desk] 热键 {} {}, 托盘 {}",
        HOTKEY_TEXT,
        if can_restore { "就绪" } else { "不可用" },
        if tray_guard.is_some() { "已创建" } else { "未创建" }
    );

    // 自测用：等窗口被收起来之后，模拟一次热键唤出（真实按键没法脚本化）
    #[cfg(debug_assertions)]
    if std::env::var("TF_HOTKEY_TEST").is_ok() {
        let p = event_loop.create_proxy();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(4));
            let _ = p.send_event(UserEvent::Toggle);
            eprintln!("[test] 模拟热键唤出");
        });
    }

    let nav_proxy = event_loop.create_proxy();
    let mut builder = WebViewBuilder::new_with_web_context(&mut ctx)
        .with_url(url)
        .with_devtools(cfg!(debug_assertions))
        .with_document_title_changed_handler(move |t: String| {
            let _ = proxy.send_event(UserEvent::Title(t));
        })
        // Boss Key 会让页面跳 about:blank。拦下来：窗口收起、页面停在原处，
        // 下次唤出时还在刚才读的那一段，不用重新翻。
        .with_navigation_handler(move |target: String| {
            if target == "about:blank" {
                let _ = nav_proxy.send_event(UserEvent::Boss);
                return false;
            }
            true
        });

    // 自测开关：调试构建下按 Esc 的效果没法自动按键验证，就用脚本模拟一次跳转
    if cfg!(debug_assertions) && std::env::var("TF_BOSS_TEST").is_ok() {
        builder = builder.with_initialization_script(
            "setTimeout(function(){ location.replace('about:blank'); }, 1500);",
        );
    }

    // webview 必须比事件循环活得久，否则窗口一重绘就白屏
    let _webview = builder.build(&window)?;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        // 这两行看着多余，其实是保命：管理器 / 托盘对象一旦被 drop，
        // 热键就注销、图标就消失。借用一次强行把它们捕获进闭包。
        let _keep_hotkey = &hotkey_guard;
        let _keep_tray = &tray_guard;

        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                save_to(&window, &state_path);
                *control_flow = ControlFlow::Exit;
            }
            Event::UserEvent(UserEvent::Title(t)) => window.set_title(&t),
            Event::UserEvent(UserEvent::Boss) => {
                if can_restore {
                    hide_window(&window, &state_path);
                    #[cfg(debug_assertions)]
                    eprintln!("[boss] 已收起窗口 visible={}", window.is_visible());
                } else {
                    save_to(&window, &state_path);
                    *control_flow = ControlFlow::Exit;
                }
            }
            Event::UserEvent(UserEvent::Quit) => {
                save_to(&window, &state_path);
                *control_flow = ControlFlow::Exit;
            }
            Event::UserEvent(UserEvent::Toggle) => {
                // 在前台就收起，在后台（或已隐藏）就拎到最前面——
                // 这比无脑 toggle 更符合直觉：别人刚切走，一按热键就想看回来。
                if window.is_visible() && window.is_focused() {
                    hide_window(&window, &state_path);
                } else {
                    show_window(&window);
                }
            }
            _ => {}
        }
    });
}

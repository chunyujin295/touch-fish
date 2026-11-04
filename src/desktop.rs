//! 桌面壳：一个真正的原生窗口 + WebView2，指向进程内起的本地服务。
//!
//! 为什么不用 Tauri：Tauri 的价值在于前端工程化（node、打包、插件），
//! 而这里的界面是手写 HTML/CSS、服务端直出，没有构建步骤。
//! 拿 wry（Tauri 的底层）直接开窗，依赖少一个数量级，编译从十几分钟变成两分钟。
//!
//! 关窗与隐藏的分工（v0.8 起）：
//! - 点右上角 X → 收起到托盘，进程、本地服务、阅读进度都留着
//! - Boss Key（页面跳 about:blank）→ 同样只是隐藏，热键一按就回来
//! - 真退出只走托盘右键菜单「退出」
//! 这条设计的理由：摸鱼时最怕的是"关掉之后手忙脚乱重新启动"，
//! 隐藏的窗口不进任务栏、不进 Alt+Tab，比退出更安全也更快恢复；
//! 而托盘是唯一常驻入口，所以它一定得给出"真的关掉"这个出口。
//!
//! 托盘右键菜单（并列两项，不记选择）：
//! - 打开主页面：把桌面窗口拎到最前
//! - 打开网页页面：先把窗口收起来，再用系统浏览器打开同一个地址
//! 两种前端同一时刻只露出一个——网页版本来就更像"真网页"，
//! 窗口和标签页同时开着反而更容易露馅。

use std::path::{Path, PathBuf};

use anyhow::Result;
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use tao::dpi::{PhysicalPosition, PhysicalSize};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::window::{Icon, Window, WindowBuilder};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use wry::{WebContext, WebViewBuilder};

/// 唤出 / 收起的全局热键，默认 Ctrl+Alt+F（F = Fish），可在设置里改
pub const DEFAULT_HOTKEY: &str = "ctrl+alt+f";

/// 给设置接口用的校验：写得对就能解析出修饰键 + 主键
pub fn hotkey_ok(spec: &str) -> bool {
    parse_hotkey(spec).is_some()
}

/// 把 "ctrl+alt+f" 这样的写法解析成 (修饰键, 主键)。
/// 键名走 keyboard-types 的 `Code::from_str`，所以 "Backquote" "F1" "Space" 这些也能直接用。
fn parse_hotkey(spec: &str) -> Option<(Modifiers, Code)> {
    let mut mods = Modifiers::empty();
    let mut key: Option<Code> = None;
    let parts: Vec<String> = spec
        .split(['+', '-'])
        .map(|p| p.trim().to_lowercase())
        .filter(|p| !p.is_empty())
        .collect();
    if parts.is_empty() {
        return None;
    }
    for p in parts {
        match p.as_str() {
            "ctrl" | "control" => mods |= Modifiers::CONTROL,
            "alt" | "option" | "opt" => mods |= Modifiers::ALT,
            "shift" => mods |= Modifiers::SHIFT,
            "win" | "super" | "meta" | "cmd" | "command" => mods |= Modifiers::META,
            other => key = key_code(other),
        }
    }
    // 只按修饰键没有意义，必须有一个主键
    key.map(|k| (mods, k))
}

fn key_code(t: &str) -> Option<Code> {
    let name = match t {
        "space" | "spacebar" => "Space".to_string(),
        "enter" | "return" => "Enter".to_string(),
        "tab" => "Tab".to_string(),
        "esc" | "escape" => "Escape".to_string(),
        "up" => "ArrowUp".to_string(),
        "down" => "ArrowDown".to_string(),
        "left" => "ArrowLeft".to_string(),
        "right" => "ArrowRight".to_string(),
        "backquote" | "`" | "~" => "Backquote".to_string(),
        "minus" | "-" => "Minus".to_string(),
        "equal" | "=" => "Equal".to_string(),
        "comma" | "," => "Comma".to_string(),
        "period" | "." => "Period".to_string(),
        "slash" | "/" => "Slash".to_string(),
        "semicolon" | ";" => "Semicolon".to_string(),
        "quote" | "'" => "Quote".to_string(),
        "backslash" | "\\" => "Backslash".to_string(),
        "bracketleft" | "[" => "BracketLeft".to_string(),
        "bracketright" | "]" => "BracketRight".to_string(),
        _ => {
            let mut cs = t.chars();
            let (c, single) = match (cs.next(), cs.next()) {
                (Some(c), None) => (c, true),
                _ => ('\0', false),
            };
            if single && c.is_ascii_alphabetic() {
                format!("Key{}", c.to_ascii_uppercase())
            } else if single && c.is_ascii_digit() {
                format!("Digit{c}")
            } else if t.starts_with('f') && t[1..].parse::<u8>().is_ok() {
                format!("F{}", &t[1..])
            } else {
                t.to_string()
            }
        }
    };
    name.parse::<Code>().ok()
}

/// 给界面和托盘看的人类可读写法：ctrl+alt+f → Ctrl+Alt+F
fn hotkey_label(spec: &str) -> String {
    spec.split(['+', '-'])
        .filter(|p| !p.trim().is_empty())
        .map(|p| {
            let p = p.trim().to_lowercase();
            match p.as_str() {
                "ctrl" | "control" => "Ctrl".to_string(),
                "alt" | "option" => "Alt".to_string(),
                "shift" => "Shift".to_string(),
                "win" | "super" | "meta" | "cmd" => "Win".to_string(),
                "space" => "Space".to_string(),
                other => {
                    let mut it = other.chars();
                    match it.next() {
                        Some(ch) if it.next().is_none() => ch.to_uppercase().collect(),
                        Some(ch) => {
                            let mut s: String = ch.to_uppercase().collect();
                            s.push_str(&other[ch.len_utf8()..]);
                            s
                        }
                        None => other.to_string(),
                    }
                }
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

/// 跨线程把事件丢回主线程：WebView 回调、托盘、热键都不在 UI 线程上
pub enum UserEvent {
    /// 页面标题变了，窗口标题要跟着变——浏览器就是这样，我们得一样
    Title(String),
    /// 页面跳到了 about:blank（Boss Key），把窗口收起来
    Boss,
    /// 热键 / 托盘左键：可见且在前台就收起，否则唤出
    Toggle,
    /// 托盘菜单「打开主页面」：把桌面窗口拎到最前
    OpenWindow,
    /// 托盘菜单「打开网页页面」：收起窗口，交给系统浏览器
    OpenWeb,
    /// 页面里的「在浏览器中打开」：只收起窗口，浏览器由 web 层自己开
    Hide,
    /// 设置里改了全局热键：注销旧的、注册新的，立即生效
    Hotkey(String),
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

/// 让托盘菜单的勾选跟上窗口状态。统一放在事件循环的 MainEventsCleared 里同步，
/// 比在每个 hide/show 分支各写一遍更不容易漏（Esc、热键、托盘左键、点网页菜单…共六条路径）。
fn sync_tray_check(item: Option<&CheckMenuItem>, window: &Window) {
    if let Some(item) = item {
        let visible = window.is_visible();
        if item.is_checked() != visible {
            item.set_checked(visible);
            #[cfg(debug_assertions)]
            eprintln!("[tray] 「打开主页面」勾选 = {visible}");
        }
    }
}

fn env_flag(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(v) => !(v == "0" || v.eq_ignore_ascii_case("false")),
        Err(_) => default,
    }
}

/// 托盘图标：左键切换、右键菜单。
/// 换来的好处是"忘了热键也能找回来"，代价是通知区多一个图标——设置里可以关掉。
///
/// 菜单是并列三项而不是一个"显示/收起"：收起这件事左键已经做了，
/// 菜单要回答的是"现在想用哪种前端"——主窗口，还是浏览器标签页。
fn build_tray(
    proxy: EventLoopProxy<UserEvent>,
    label: &str,
) -> Option<(tray_icon::TrayIcon, CheckMenuItem)> {
    let menu = Menu::new();
    // 「打开主页面」前面带勾：窗口现在是收起的还是已经开着，一眼能看出来。
    // 勾选状态由事件循环里的 MainEventsCleared 同步（见 sync_tray_check）。
    let open_win = CheckMenuItem::with_id("open-window", "打开主页面", true, true, None);
    let open_web = MenuItem::with_id("open-web", "打开网页页面", true, None);
    let sep = PredefinedMenuItem::separator();
    let quit = MenuItem::with_id("quit", "退出", true, None);
    menu.append_items(&[&open_win, &open_web, &sep, &quit]).ok()?;

    let tip = if label.is_empty() {
        "TouchFish".to_string()
    } else {
        format!("TouchFish · {label}")
    };
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip(tip)
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
        // 未知的菜单项一律忽略：早先这里是 `_ => Toggle`，
        // 以后再加菜单项时点一下就会莫名其妙把窗口收起来。
        let ev = match e.id().0.as_str() {
            "open-window" => UserEvent::OpenWindow,
            "open-web" => UserEvent::OpenWeb,
            "quit" => UserEvent::Quit,
            _ => return,
        };
        let _ = menu_proxy.send_event(ev);
    }));

    Some((tray, open_win))
}

/// 全局热键的按下事件永远走这一条：解成 Toggle，交给 UI 线程决定收起还是唤出。
/// 和注册分开，是为了运行中换键时不用重设 handler。
fn install_hotkey_handler(proxy: EventLoopProxy<UserEvent>) {
    GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
        if e.state() == HotKeyState::Pressed {
            let _ = proxy.send_event(UserEvent::Toggle);
        }
    }));
}

/// 注册全局热键。注册失败（写法不对，或被别的软件占了）时返回 None，
/// 调用方据此把 Boss Key / 关窗退回"直接退出"，免得窗口收起来之后再也叫不出来。
fn build_hotkey(spec: &str) -> Option<(GlobalHotKeyManager, HotKey)> {
    let (mods, code) = parse_hotkey(spec)?;
    let hk = HotKey::new(Some(mods), code);
    let manager = GlobalHotKeyManager::new().ok()?;
    if manager.register(hk).is_err() {
        return None;
    }
    Some((manager, hk))
}

/// 运行中换热键：先注销旧的再试新的，失败就把旧的装回去。
/// 宁可维持原状，也不能让用户点一下"保存"就失去唤出窗口的唯一入口。
fn apply_hotkey(
    cur: Option<(GlobalHotKeyManager, HotKey)>,
    spec: &str,
) -> Option<(GlobalHotKeyManager, HotKey)> {
    let Some((mods, code)) = parse_hotkey(spec) else {
        return cur;
    };
    let next = HotKey::new(Some(mods), code);
    match cur {
        // HotKey 的 id 每次 new 都会自增，只能比较修饰键 + 主键
        Some((m, old)) if old.mods == next.mods && old.key == next.key => Some((m, old)),
        Some((m, old)) => {
            m.unregister(old).ok();
            if m.register(next).is_ok() {
                #[cfg(debug_assertions)]
                eprintln!("[hotkey] 已换成 {}", hotkey_label(spec));
                Some((m, next))
            } else {
                m.register(old).ok();
                eprintln!("[warn] 热键 {} 注册失败，保持原设置", hotkey_label(spec));
                Some((m, old))
            }
        }
        None => {
            // 启动时没注册上（被占用或设置里写错），用户这次改对了就现开一个
            let m = GlobalHotKeyManager::new().ok()?;
            if m.register(next).is_err() {
                eprintln!("[warn] 热键 {} 注册失败", hotkey_label(spec));
                return None;
            }
            Some((m, next))
        }
    }
}

/// 起窗口并阻塞在主线程。`url` 是已经就绪的本地地址。
pub fn run(
    url: &str,
    webview_dir: PathBuf,
    state_path: PathBuf,
    hotkey_spec: String,
    tray_on: bool,
) -> Result<()> {
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

    // 环境变量仍可临时覆盖（调试用），否则以设置为准
    let spec = if hotkey_spec.trim().is_empty() {
        DEFAULT_HOTKEY.to_string()
    } else {
        hotkey_spec
    };
    let spec = std::env::var("TF_HOTKEY").unwrap_or(spec);
    let label = hotkey_label(&spec);

    // 热键先注册：它决定了 Boss Key / 关窗是"收起"还是"退出"
    install_hotkey_handler(event_loop.create_proxy());
    let mut hotkey_state = build_hotkey(&spec);
    let hotkey_ok = hotkey_state.is_some();
    if !hotkey_ok {
        eprintln!("[warn] 全局热键 {label} 注册失败（写法不对或被其他软件占用）");
    }

    let (tray_guard, tray_check) = if tray_on && env_flag("TOUCHFISH_TRAY", true) {
        match build_tray(event_loop.create_proxy(), if hotkey_ok { &label } else { "" }) {
            Some((tray, item)) => (Some(tray), Some(item)),
            None => (None, None),
        }
    } else {
        (None, None)
    };

    // 只要还有一个"能把窗口叫回来"的入口，Esc 就只收起；两个都没有才退回退出
    let can_restore = hotkey_ok || tray_guard.is_some();
    if !can_restore {
        eprintln!("[warn] 热键与托盘都不可用，Esc 将直接退出");
    }

    // 网页层要窗口配合的两件事：收起来、换热键。桌面壳在这里把能力交出去，
    // 纯网页模式（--server）永远不会注册，那边的调用会自然跳过。
    let hide_proxy = event_loop.create_proxy();
    let hotkey_proxy = event_loop.create_proxy();
    let _ = crate::UI_HOOKS.set(crate::UiHooks {
        hide_window: Box::new(move || {
            let _ = hide_proxy.send_event(UserEvent::Hide);
        }),
        set_hotkey: Box::new(move |s: String| {
            let _ = hotkey_proxy.send_event(UserEvent::Hotkey(s));
        }),
    });

    #[cfg(debug_assertions)]
    eprintln!(
        "[desk] 热键 {} {}, 托盘 {}",
        label,
        if hotkey_ok { "就绪" } else { "不可用" },
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
    // 网页页面要用的地址：webview 已经拿走了 &str，这里留一份给托盘菜单
    let page_url = url.to_string();
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
        // 托盘对象一旦被 drop 图标就消失，借用一次强行捕获进闭包。
        // hotkey_state 会被 move 进来（换热键时要改它），生命周期一样没问题。
        let _keep_tray = &tray_guard;
        // 每帧重新判断：运行中换热键、注册失败回退，都可能改变"还能不能叫回来"
        let can_restore = tray_guard.is_some() || hotkey_state.is_some();

        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                // 点 X = 收起，后端（本地服务 + 托盘 + 热键）继续常驻；
                // 只有托盘的「退出」才真的结束进程。
                // 例外：托盘被关掉、热键又没注册上时，收起来就再也叫不出来，只能真退出。
                if can_restore {
                    hide_window(&window, &state_path);
                } else {
                    save_to(&window, &state_path);
                    *control_flow = ControlFlow::Exit;
                }
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
            Event::UserEvent(UserEvent::OpenWindow) => show_window(&window),
            Event::UserEvent(UserEvent::OpenWeb) => {
                // 两种前端只露一个：先把窗口收起来，再交给浏览器。
                // 窗口只是隐藏，不销毁，网页版看完点托盘就能换回来。
                hide_window(&window, &state_path);
                crate::open_browser(&page_url);
            }
            Event::UserEvent(UserEvent::Hide) => {
                // 书库里的「在浏览器中打开」走的是这条：浏览器由 web 层自己开，
                // 这里只负责把窗口收起来，和托盘「打开网页页面」保持同一个效果。
                if can_restore {
                    hide_window(&window, &state_path);
                }
            }
            Event::UserEvent(UserEvent::Hotkey(spec)) => {
                // FnMut 闭包里不能把捕获的 Option 整个 move 出去，先 take 再放回
                let cur = hotkey_state.take();
                hotkey_state = apply_hotkey(cur, &spec);
                // 托盘的 tooltip 里带着热键，换完跟着改，不然提示还是旧的
                if let Some(t) = tray_guard.as_ref() {
                    let _ = t.set_tooltip(Some(format!("TouchFish · {}", hotkey_label(&spec))));
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
            Event::MainEventsCleared => sync_tray_check(tray_check.as_ref(), &window),
            _ => {}
        }
    });
}

//! 桌面壳：一个真正的原生窗口 + WebView2，指向进程内起的本地服务。
//!
//! 为什么不用 Tauri：Tauri 的价值在于前端工程化（node、打包、插件），
//! 而这里的界面是手写 HTML/CSS、服务端直出，没有构建步骤。
//! 拿 wry（Tauri 的底层）直接开窗，依赖少一个数量级，编译从十几分钟变成两分钟。

use std::path::{Path, PathBuf};

use anyhow::Result;
use tao::dpi::{PhysicalPosition, PhysicalSize};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::window::{Icon, Window, WindowBuilder};
use wry::{WebContext, WebViewBuilder};

/// 页面标题变了，窗口标题要跟着变——浏览器就是这样，我们得一样
pub enum UserEvent {
    Title(String),
    /// 页面跳到了 about:blank（Boss Key），原生窗口直接退出更彻底
    Boss,
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

    let nav_proxy = event_loop.create_proxy();
    let mut builder = WebViewBuilder::new_with_web_context(&mut ctx)
        .with_url(url)
        .with_devtools(cfg!(debug_assertions))
        .with_document_title_changed_handler(move |t: String| {
            let _ = proxy.send_event(UserEvent::Title(t));
        })
        // Boss Key 跳 about:blank 时，原生窗口直接关掉——留个白窗口比什么都可疑
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
                save_to(&window, &state_path);
                *control_flow = ControlFlow::Exit;
            }
            _ => {}
        }
    });
}

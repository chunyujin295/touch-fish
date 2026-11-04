//! 给 exe 嵌入图标和版本信息。
//!
//! 运行时窗口和托盘用的是 `assets/app/icon.rgba`（见 desktop.rs），那是「跑起来之后」
//! 的任务栏图标；而资源管理器、开始菜单和桌面快捷方式读的是 **exe 文件里的 ICON 资源**。
//! 不嵌的话这些地方就是一张白纸——便携版直接双击 exe 时不容易察觉，做成安装包、
//! 快捷方式一多立刻露馅。
//!
//! 这里不走 winres：它在 Windows 上要调 reg.exe 去翻注册表找 SDK，沙箱/精简系统里
//! 会被拦。改成直接扫 `Windows Kits\10\bin\<版本>\x64\rc.exe`，找到就编译，
//! 找不到只是警告——图标是锦上添花，不该让没装 SDK 的人编不过。

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets/app/icon.ico");
    println!("cargo:rerun-if-changed=scripts/version.txt");

    #[cfg(windows)]
    windows_icon();
}

#[cfg(windows)]
fn windows_icon() {
    let Ok(out_dir) = std::env::var("OUT_DIR") else { return };
    let out = PathBuf::from(out_dir);
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    let icon = root.join("assets/app/icon.ico");
    if !icon.exists() {
        println!("cargo:warning=找不到 assets/app/icon.ico，跳过图标资源");
        return;
    }

    let ver = std::fs::read_to_string(root.join("scripts/version.txt"))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "0.0.0".into());
    let mut nums: Vec<u32> = ver.split('.').filter_map(|x| x.parse().ok()).collect();
    nums.resize(4, 0);

    // rc 的相对路径以 .rc 文件所在目录为准，所以图标给绝对路径
    let rc_text = format!(
        r#"IDI_ICON1 ICON "{icon}"
1 VERSIONINFO
 FILEVERSION {a},{b},{c},{d}
 PRODUCTVERSION {a},{b},{c},{d}
 FILEFLAGSMASK 0x3fL
 FILEFLAGS 0x0L
 FILEOS 0x40004L
 FILETYPE 0x1L
 FILESUBTYPE 0x0L
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "080404B0"
    BEGIN
      VALUE "CompanyName", "chunyujin295"
      VALUE "FileDescription", "TouchFish 摸鱼阅读器"
      VALUE "FileVersion", "{ver}"
      VALUE "InternalName", "touchfish"
      VALUE "OriginalFilename", "touchfish.exe"
      VALUE "ProductName", "TouchFish"
      VALUE "ProductVersion", "{ver}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x804, 1200
  END
END
"#,
        // rc 的字符串里反斜杠会被当转义（\t 变制表符），统一用正斜杠
        icon = icon.display().to_string().replace('\\', "/"),
        a = nums[0],
        b = nums[1],
        c = nums[2],
        d = nums[3],
        ver = ver
    );

    let rc_path = out.join("app.rc");
    let res_path = out.join("app.res");
    if std::fs::write(&rc_path, rc_text).is_err() {
        return;
    }

    let Some(rc_exe) = find_rc() else {
        println!("cargo:warning=没找到 rc.exe（Windows Kits），exe 将不带图标资源");
        return;
    };

    match Command::new(&rc_exe)
        .arg("/nologo")
        // 脚本是 UTF-8，带中文，不指定代码页 rc 会按 GBK 读成乱码
        .arg("/c")
        .arg("65001")
        .arg("/fo")
        .arg(&res_path)
        .arg(&rc_path)
        .status()
    {
        Ok(s) if s.success() => {
            // link.exe 直接吃 .res，会自己转成 COFF 资源段
            println!("cargo:rustc-link-arg={}", res_path.display());
        }
        Ok(s) => println!("cargo:warning=rc.exe 退出码 {s}，exe 不带图标资源"),
        Err(e) => println!("cargo:warning=调用 rc.exe 失败：{e}"),
    }
}

/// 扫 SDK 目录找最新一版 rc.exe（版本号最大的优先）。
#[cfg(windows)]
fn find_rc() -> Option<PathBuf> {
    use std::fs;
    let roots = [
        r"C:\Program Files (x86)\Windows Kits\10\bin",
        r"C:\Program Files\Windows Kits\10\bin",
    ];
    let mut best: Option<(String, PathBuf)> = None;
    for root in roots {
        let Ok(rd) = fs::read_dir(root) else { continue };
        for entry in rd.flatten() {
            let cand = entry.path().join("x64").join("rc.exe");
            if !cand.exists() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            // 目录名就是 SDK 版本号，字符串比较对 10.0.xxxxx.0 这种格式够用
            if best.as_ref().is_none_or(|(b, _)| name > *b) {
                best = Some((name, cand));
            }
        }
    }
    best.map(|(_, p)| p)
}

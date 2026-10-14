//! Linux: a `.desktop` entry plus `xdg-mime`.
//!
//! Both steps are user-scoped and need no root: the launcher goes into
//! `$XDG_DATA_HOME/applications` (`~/.local/share/applications` by default) and
//! `xdg-mime default` writes `~/.config/mimeapps.list`.

use std::path::PathBuf;
use std::process::Command;

use super::plan::AssocTarget;
use crate::error::AppError;

pub const SUPPORTS_PER_FORMAT_ICONS: bool = false;
pub const CAN_ASSOCIATE: bool = true;
pub const CAN_SET_DEFAULT: bool = true;

pub const NOTE: &str = "关联写入 ~/.local/share/applications 并调用 xdg-mime 设为默认，无需 root。\
Linux 桌面不支持按扩展名自定义文件图标，图标由当前图标主题决定。";

/// MIME type per extension. `xdg-mime` keys off MIME, not extension.
fn mime_for(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "ico" => "image/vnd.microsoft.icon",
        "svg" => "image/svg+xml",
        _ => return None,
    })
}

fn desktop_file() -> Result<PathBuf, AppError> {
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .ok_or_else(|| AppError::Other("无法确定 XDG 数据目录（HOME 未设置）".into()))?;
    Ok(data_home.join("applications/lumina.desktop"))
}

/// Whether the desktop entry already lists Lumina as the handler for `ext`.
pub fn is_associated(ext: &str) -> bool {
    let Some(mime) = mime_for(ext) else {
        return false;
    };
    let Some(home) = std::env::var_os("HOME") else {
        return false;
    };
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(&home).join(".config"));
    let Ok(list) = std::fs::read_to_string(config_home.join("mimeapps.list")) else {
        return false;
    };
    // Look only inside [Default Applications] for a line mapping the MIME to us.
    list.split("[Default Applications]")
        .nth(1)
        .map(|section| {
            section.lines().any(|line| {
                let line = line.trim();
                line.starts_with(&format!("{mime}=")) && line.contains("lumina.desktop")
            })
        })
        .unwrap_or(false)
}

pub fn associate(target: &AssocTarget, exts: &[String]) -> Result<(), AppError> {
    write_desktop_entry(target)?;
    run("update-desktop-database", &[desktop_dir()?.to_string_lossy().as_ref()])?;
    for ext in exts {
        if let Some(mime) = mime_for(ext) {
            run("xdg-mime", &["default", "lumina.desktop", mime])?;
        }
    }
    Ok(())
}

/// Point every MIME back at whatever else is installed.
///
/// There is no "unset" in `xdg-mime`, so the honest move is to remove our
/// launcher and let the desktop fall back to its own default.
pub fn dissociate(_target: &AssocTarget, _exts: &[String]) -> Result<(), AppError> {
    let entry = desktop_file()?;
    if entry.exists() {
        std::fs::remove_file(&entry).map_err(|e| {
            AppError::Other(format!("无法删除 {}：{e}", entry.display()))
        })?;
    }
    let dir = desktop_dir()?;
    let _ = Command::new("update-desktop-database")
        .arg(&dir)
        .status();
    Ok(())
}

fn desktop_dir() -> Result<PathBuf, AppError> {
    desktop_file()?
        .parent()
        .map(PathBuf::from)
        .ok_or_else(|| AppError::Other("无法确定桌面项目录".into()))
}

fn write_desktop_entry(target: &AssocTarget) -> Result<(), AppError> {
    let path = desktop_file()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| AppError::Other(format!("无法创建 {}：{e}", dir.display())))?;
    }
    let mut mimes: Vec<&str> = super::all_extensions()
        .iter()
        .filter_map(|e| mime_for(e))
        .collect();
    mimes.sort_unstable();
    mimes.dedup();

    let body = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Lumina\n\
         Comment=轻量、跨平台的图片查看器\n\
         Exec=\"{exe}\" %f\n\
         Terminal=false\n\
         Categories=Graphics;Viewer;\n\
         MimeType={mimes};\n",
        exe = target.exe.display(),
        mimes = mimes.join(";"),
    );
    std::fs::write(&path, body)
        .map_err(|e| AppError::Other(format!("无法写入 {}：{e}", path.display())))
}

fn run(program: &str, args: &[&str]) -> Result<(), AppError> {
    let status = Command::new(program)
        .args(args)
        .status()
        .map_err(|e| AppError::Other(format!("无法执行 {program}：{e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(AppError::Other(format!("{program} 返回 {status}")))
    }
}

//! Windows: execute a [`RegPlan`] against `HKEY_CURRENT_USER\Software\Classes`.
//!
//! Per-user (`HKCU`) rather than machine-wide (`HKLM`) on purpose: it needs no
//! administrator elevation, and the per-user hive takes precedence over the
//! machine one anyway. Nothing here can damage another application's
//! registration -- see [`plan::build_unassoc_plan`].

use winreg::enums::HKEY_CURRENT_USER;
use winreg::RegKey;

use super::plan::{self, AssocTarget, RegPlan};
use crate::error::AppError;

pub const SUPPORTS_PER_FORMAT_ICONS: bool = true;
pub const CAN_ASSOCIATE: bool = true;
pub const CAN_SET_DEFAULT: bool = false;

pub const NOTE: &str = "关联写入当前用户注册表，无需管理员权限；每种格式在资源管理器中显示各自的图标。\
Windows 10/11 不允许程序静默把自己设为默认打开方式，请在系统「默认应用」中确认一次。";

/// True when Lumina's ProgID is advertised for `ext`.
pub fn is_associated(ext: &str) -> bool {
    let path = format!("Software\\Classes\\.{ext}\\OpenWithProgids");
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(path)
        .and_then(|key| key.get_value::<String, _>(plan::prog_id(ext)))
        .is_ok()
}

pub fn associate(target: &AssocTarget, exts: &[String]) -> Result<(), AppError> {
    apply(&plan::build_assoc_plan(target, exts))
}

pub fn dissociate(target: &AssocTarget, exts: &[String]) -> Result<(), AppError> {
    apply(&plan::build_unassoc_plan(target, exts))
}

fn apply(plan: &RegPlan) -> Result<(), AppError> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);

    for op in &plan.write {
        let (key, _) = hkcu
            .create_subkey(&op.key)
            .map_err(|e| fail("创建注册表项", &op.key, e))?;

        // Preserve whoever owned the default before we claim it, so withdrawing
        // can hand it back.
        if op.backup_previous {
            if let Ok(previous) = key.get_value::<String, _>(&op.name) {
                key.set_value(plan::BACKUP_VALUE, &previous)
                    .map_err(|e| fail("备份原有默认程序", &op.key, e))?;
            }
        }

        key.set_value(&op.name, &op.value)
            .map_err(|e| fail("写入注册表值", &op.key, e))?;
    }

    // Withdrawing twice, or withdrawing something that was never written, is not
    // an error -- a missing key is simply nothing to do.
    for (key_path, name) in &plan.remove_values {
        if let Ok(key) = hkcu.open_subkey(key_path) {
            let _ = key.delete_value(name);
        }
    }
    for key_path in &plan.remove_keys {
        let _ = hkcu.delete_subkey_all(key_path);
    }
    for key_path in &plan.restore_defaults {
        if let Ok(key) = hkcu.open_subkey(key_path) {
            if let Ok(previous) = key.get_value::<String, _>(plan::BACKUP_VALUE) {
                let _ = key.set_value("", &previous);
            }
            let _ = key.delete_value(plan::BACKUP_VALUE);
        }
    }

    notify_shell();
    Ok(())
}

/// Tell Explorer its association and icon caches are stale.
///
/// Microsoft is explicit that this is required: without it the change "may not
/// be recognized until after the system is rebooted", and Explorer's icon cache
/// is famously sticky.
fn notify_shell() {
    use windows_sys::Win32::UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_FLUSH};

    // SAFETY: the documented call for an association change -- a well-known
    // event id, a flags value, and two null item pointers.
    unsafe {
        SHChangeNotify(
            SHCNE_ASSOCCHANGED as i32,
            SHCNF_FLUSH,
            std::ptr::null(),
            std::ptr::null(),
        );
    }
}

fn fail(action: &str, key: &str, e: std::io::Error) -> AppError {
    AppError::Other(format!("{action}失败（{key}）：{e}"))
}

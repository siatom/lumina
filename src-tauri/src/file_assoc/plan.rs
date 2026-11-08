//! The Windows association layout, expressed as **pure data**.
//!
//! Everything Windows-specific about file association is "which registry values
//! get written where". That part is a pure function of (executable, icon
//! directory, extension list), so it lives here -- free of `winreg`, free of
//! `#[cfg]`, and unit-tested on every platform. [`super::windows`] is then a
//! thin executor: walk the plan, call `RegKey::set_value`, notify the shell.
//!
//! The layout follows Microsoft's file-association guidance:
//!
//! * **One ProgID per extension** (`Lumina.png`, `Lumina.jpg`, ...). The shell
//!   resolves a file's icon as `.<ext> -> ProgID -> ProgID\DefaultIcon`, so a
//!   single shared ProgID would force every format to share one icon.
//! * `OpenWithProgids` advertises Lumina in the "Open with" list *without*
//!   taking the default.
//! * `RegisteredApplications` + `Capabilities` is what makes Lumina appear in
//!   Settings > Apps > Default apps.
//! * Windows 8+ protects the user's actual choice in `FileExts\.<ext>\UserChoice`
//!   behind a hash. Writing the extension's default value is therefore a
//!   *request*, not a guarantee -- the user still confirms once in Settings.

use std::path::{Path, PathBuf};

use super::format_of;

/// Application name used for the `RegisteredApplications` entry and the
/// `Applications\<exe>` subkey's friendly name.
pub const APP_NAME: &str = "Lumina";

/// `HKCU` path of the capabilities block that `RegisteredApplications` points at.
pub const CAPABILITIES_PATH: &str = "Software\\Lumina\\Capabilities";

/// Value name holding whatever the extension's default ProgID was before Lumina
/// overwrote it, so [`build_unassoc_plan`] can put it back.
pub const BACKUP_VALUE: &str = "Lumina_backup";

/// Where the association should point.
pub struct AssocTarget {
    /// Absolute path of the running executable.
    pub exe: PathBuf,
    /// Directory holding `<key>.ico`, one per [`super::Format::key`].
    pub ico_dir: PathBuf,
}

impl AssocTarget {
    /// File name of the executable, e.g. `Lumina.exe`.
    ///
    /// Split on both separators rather than using `Path::file_name`: the plan
    /// always describes a *Windows* layout, but its unit tests run on whatever
    /// host happens to be building Lumina, where `\` is not a separator.
    pub fn exe_name(&self) -> String {
        self.exe
            .to_string_lossy()
            .rsplit(['\\', '/'])
            .next()
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{APP_NAME}.exe"))
    }
}

/// Render a path the way a Windows registry value expects it.
///
/// Same reasoning as [`AssocTarget::exe_name`]: `Path::display()` follows the
/// host separator, which would emit `/` on the machines Lumina is developed on.
/// Normalising here keeps the generated layout correct *and* keeps the tests
/// meaningful everywhere.
fn win_path(path: &Path) -> String {
    path.to_string_lossy().replace('/', "\\")
}

/// One registry value to write.
pub struct RegOp {
    /// Key path below `HKEY_CURRENT_USER`.
    pub key: String,
    /// Value name; `""` is the key's default value.
    pub name: String,
    /// `REG_SZ` payload.
    pub value: String,
    /// Read the value currently stored under `name` and stash it in
    /// [`BACKUP_VALUE`] before overwriting. Set only for an extension's default
    /// value, which may belong to another application.
    pub backup_previous: bool,
}

impl RegOp {
    fn sz(key: String, name: &str, value: String) -> Self {
        Self {
            key,
            name: name.to_string(),
            value,
            backup_previous: false,
        }
    }

    fn claim_default(key: String, value: String) -> Self {
        Self {
            key,
            name: String::new(),
            value,
            backup_previous: true,
        }
    }
}

/// A declarative description of the registry work for one association change.
#[derive(Default)]
pub struct RegPlan {
    pub write: Vec<RegOp>,
    /// Keys to delete recursively (only ever Lumina's own).
    pub remove_keys: Vec<String>,
    /// Individual values to delete: `(key, value name)`.
    pub remove_values: Vec<(String, String)>,
    /// Extension keys whose [`BACKUP_VALUE`] should be restored into the default
    /// value (and the backup dropped). Applied last.
    pub restore_defaults: Vec<String>,
}

impl RegPlan {
    /// Every value the plan intends to write under `key` with `name`.
    ///
    /// Only the tests inspect a plan by key; the executor in
    /// [`super::windows`] just walks `write`, so this stays test-only.
    #[cfg(test)]
    pub fn find(&self, key: &str, name: &str) -> Option<&RegOp> {
        self.write.iter().find(|op| op.key == key && op.name == name)
    }
}

/// `Software\Classes\<rest>` -- the per-user view of `HKEY_CLASSES_ROOT`.
fn classes(rest: &str) -> String {
    format!("Software\\Classes\\{rest}")
}

/// ProgID for one extension, e.g. `Lumina.png`.
pub fn prog_id(ext: &str) -> String {
    format!("{APP_NAME}.{ext}")
}

/// The `shell\open\command` payload.
///
/// **Both** paths are quoted. Without the quotes a `C:\Program Files\...`
/// executable path or an image called `my photo.png` is split into several
/// arguments and the open silently fails.
pub fn command_line(exe: &Path) -> String {
    format!("\"{}\" \"%1\"", win_path(exe))
}

/// Icon reference for a `DefaultIcon` value: `"<path>",<index>`.
pub fn icon_ref(ico: &Path) -> String {
    format!("\"{}\",0", win_path(ico))
}

/// Registry work to make Lumina a handler for every extension in `exts`.
pub fn build_assoc_plan(target: &AssocTarget, exts: &[String]) -> RegPlan {
    let mut plan = RegPlan::default();
    let exe_name = target.exe_name();
    let command = command_line(&target.exe);

    for ext in exts {
        let pid = prog_id(ext);
        let format = format_of(ext);
        let description = match format {
            Some(f) => format!("{APP_NAME} {}", f.description),
            None => APP_NAME.to_string(),
        };

        // The ProgID carries the friendly name, the icon and the open verb.
        plan.write
            .push(RegOp::sz(classes(&pid), "", description));
        if let Some(f) = format {
            let ico = target.ico_dir.join(format!("{}.ico", f.key));
            plan.write.push(RegOp::sz(
                classes(&format!("{pid}\\DefaultIcon")),
                "",
                icon_ref(&ico),
            ));
        }
        plan.write.push(RegOp::sz(
            classes(&format!("{pid}\\shell\\open\\command")),
            "",
            command.clone(),
        ));

        // Advertise in "Open with" without touching the default.
        plan.write.push(RegOp::sz(
            classes(&format!(".{ext}\\OpenWithProgids")),
            &pid,
            String::new(),
        ));

        // Request the default, remembering the previous owner.
        plan.write
            .push(RegOp::claim_default(classes(&format!(".{ext}")), pid));

        // How the shell identifies this executable, independent of extensions.
        plan.write.push(RegOp::sz(
            classes(&format!("Applications\\{exe_name}\\SupportedTypes")),
            &format!(".{ext}"),
            String::new(),
        ));
    }

    plan.write.push(RegOp::sz(
        classes(&format!("Applications\\{exe_name}")),
        "FriendlyAppName",
        APP_NAME.to_string(),
    ));
    plan.write.push(RegOp::sz(
        classes(&format!("Applications\\{exe_name}\\shell\\open\\command")),
        "",
        command,
    ));

    // Application registration -> Settings > Apps > Default apps.
    plan.write.push(RegOp::sz(
        "Software\\RegisteredApplications".to_string(),
        APP_NAME,
        CAPABILITIES_PATH.to_string(),
    ));
    plan.write.push(RegOp::sz(
        CAPABILITIES_PATH.to_string(),
        "ApplicationName",
        APP_NAME.to_string(),
    ));
    plan.write.push(RegOp::sz(
        CAPABILITIES_PATH.to_string(),
        "ApplicationDescription",
        "轻量、跨平台的图片查看器".to_string(),
    ));
    plan.write.push(RegOp::sz(
        CAPABILITIES_PATH.to_string(),
        "ApplicationIcon",
        icon_ref(&target.exe),
    ));
    for ext in exts {
        plan.write.push(RegOp::sz(
            format!("{CAPABILITIES_PATH}\\FileAssociations"),
            &format!(".{ext}"),
            prog_id(ext),
        ));
    }

    plan
}

/// Registry work to withdraw Lumina, leaving other applications untouched.
///
/// Microsoft is explicit that an uninstaller should **not** clear the
/// extension's default value: once the ProgID is unregistered the shell ignores
/// it, and clearing it could stomp on whatever the user chose afterwards. A
/// value we backed up earlier is restored instead.
pub fn build_unassoc_plan(target: &AssocTarget, exts: &[String]) -> RegPlan {
    let mut plan = RegPlan::default();

    for ext in exts {
        let pid = prog_id(ext);
        plan.remove_keys.push(classes(&pid));
        plan.remove_values
            .push((classes(&format!(".{ext}\\OpenWithProgids")), pid));
        plan.restore_defaults.push(classes(&format!(".{ext}")));
    }

    plan.remove_keys
        .push(classes(&format!("Applications\\{}", target.exe_name())));
    plan.remove_keys.push("Software\\Lumina".to_string());
    plan.remove_values
        .push(("Software\\RegisteredApplications".to_string(), APP_NAME.to_string()));

    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> AssocTarget {
        AssocTarget {
            // A path with a space, so the quoting rule is actually exercised.
            exe: PathBuf::from(r"C:\Program Files\Lumina\Lumina.exe"),
            ico_dir: PathBuf::from(r"C:\Program Files\Lumina\icons\file-icons"),
        }
    }

    fn exts() -> Vec<String> {
        vec!["png".into(), "jpg".into(), "jpeg".into(), "svg".into()]
    }

    #[test]
    fn open_command_quotes_both_the_exe_and_the_file() {
        assert_eq!(
            command_line(Path::new(r"C:\Program Files\Lumina\Lumina.exe")),
            r#""C:\Program Files\Lumina\Lumina.exe" "%1""#
        );

        let plan = build_assoc_plan(&target(), &exts());
        let op = plan
            .find(&classes(r"Lumina.png\shell\open\command"), "")
            .expect("ProgID open command");
        assert_eq!(op.value, r#""C:\Program Files\Lumina\Lumina.exe" "%1""#);
    }

    #[test]
    fn every_extension_gets_its_own_progid_and_icon() {
        let plan = build_assoc_plan(&target(), &exts());

        let png = plan
            .find(&classes(r"Lumina.png\DefaultIcon"), "")
            .expect("png icon");
        let svg = plan
            .find(&classes(r"Lumina.svg\DefaultIcon"), "")
            .expect("svg icon");
        assert!(png.value.ends_with(r#"file-icons\png.ico",0"#), "{}", png.value);
        assert!(svg.value.ends_with(r#"file-icons\svg.ico",0"#), "{}", svg.value);

        // jpg and jpeg are one format family, so they share a wordmark but not
        // a ProgID.
        let jpg = plan.find(&classes(r"Lumina.jpg\DefaultIcon"), "").unwrap();
        let jpeg = plan.find(&classes(r"Lumina.jpeg\DefaultIcon"), "").unwrap();
        assert!(jpg.value.ends_with(r#"file-icons\jpg.ico",0"#));
        assert!(jpeg.value.ends_with(r#"file-icons\jpg.ico",0"#));
    }

    #[test]
    fn open_with_progids_lists_the_progid_without_taking_the_default() {
        let plan = build_assoc_plan(&target(), &exts());
        let op = plan
            .find(&classes(r".png\OpenWithProgids"), "Lumina.png")
            .expect("OpenWithProgids entry");
        assert!(op.value.is_empty(), "the marker value carries no data");
        assert!(!op.backup_previous);
    }

    #[test]
    fn claiming_the_default_backs_up_the_previous_owner() {
        let plan = build_assoc_plan(&target(), &exts());
        let op = plan.find(&classes(".png"), "").expect("default value");
        assert_eq!(op.value, "Lumina.png");
        assert!(op.backup_previous, "the previous default must be preserved");
    }

    #[test]
    fn registers_the_application_so_it_shows_up_in_default_apps() {
        let plan = build_assoc_plan(&target(), &exts());
        assert_eq!(
            plan.find("Software\\RegisteredApplications", "Lumina")
                .unwrap()
                .value,
            CAPABILITIES_PATH
        );
        assert_eq!(
            plan.find(CAPABILITIES_PATH, "ApplicationName").unwrap().value,
            "Lumina"
        );
        assert_eq!(
            plan.find(
                &format!("{CAPABILITIES_PATH}\\FileAssociations"),
                ".jpeg"
            )
            .unwrap()
            .value,
            "Lumina.jpeg"
        );
    }

    #[test]
    fn withdraw_removes_only_lumina_keys() {
        let plan = build_unassoc_plan(&target(), &exts());

        assert!(plan.remove_keys.contains(&classes("Lumina.png")));
        assert!(plan.remove_keys.contains(&classes(r"Applications\Lumina.exe")));
        assert!(plan.remove_keys.contains(&"Software\\Lumina".to_string()));

        // The extension key itself must survive -- deleting it would take the
        // user's own default value with it.
        assert!(
            !plan.remove_keys.iter().any(|k| k == &classes(".png")),
            "the .png key must not be deleted"
        );
        assert!(
            plan.remove_values
                .contains(&(classes(r".png\OpenWithProgids"), "Lumina.png".to_string()))
        );
        assert!(plan.restore_defaults.contains(&classes(".png")));
    }
}

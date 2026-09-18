//! Mod Art Installation.
//! Due to a Firaxis oversight, the Mods locations are never scanned for blp art files. So as a workaround
//! they are installed into the DLC folder of the game install. The art consists of a .dep (dependencies)
//! and a Platforms folder with StandardAsset.blp linkers, and shared assets folder of the actual content, like:
//! DLC/<coolname>/<coolname>.dep
//! DLC/<coolname>/Platforms/Windows/BLPs/...
//!
//! `<coolname>` is what the modinfo uses when it does an `<UpdateArt><Item>` action, so parse the modinfo for that
//! art name, if none is found, it falls back on the mod's name. Admittedly, forgetting an updateArt means no art
//! at all. But should still install the regular way.

use serde::Serialize;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use super::traversal::sanitize_xml;

/// adds a file in case the modder wrote their own .dep. The Civ Art Tool manually has a line in the .dep indicating
/// this, <!-- CUSTOM MODDED --> but might as well double protect it, so never accidentally interact with real DLC:
const ART_DLC_MARKER_FILE: &str = ".civmods-art-dlc";

#[derive(Debug, Serialize)]
pub struct ArtDlcInstallInfo {
    pub folder_name: String,
    pub installed_path: String, // Absolute path
    pub dep_files: usize,       // Num of `.dep` files
    pub platforms_dirs: usize,  // Number of `Platforms` folders copied.
}

fn is_hidden(entry: &walkdir::DirEntry) -> bool {
    let name = entry.file_name().to_string_lossy();
    name.starts_with('.') || name.eq_ignore_ascii_case("__MACOSX")
}

/// Art files to copy over should be placed next to the .modinfo in the root of the mod.
struct ArtPayload {
    platforms_dirs: Vec<PathBuf>,
    dep_files: Vec<PathBuf>,
}

impl ArtPayload {
    fn is_empty(&self) -> bool {
        self.platforms_dirs.is_empty() && self.dep_files.is_empty()
    }
}

fn find_art_payload(source_root: &Path) -> ArtPayload {
    let mut platforms_dirs = Vec::new();
    let mut dep_files = Vec::new();

    let iter = WalkDir::new(source_root)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !is_hidden(e))
        .filter_map(Result::ok);

    for entry in iter {
        let path = entry.path();
        if platforms_dirs.iter().any(|dir| path.starts_with(dir)) {
            continue;       // Ignore Platforms folder already scanned.
        }

        if entry.file_type().is_dir() {
            if entry.file_name().to_string_lossy().eq_ignore_ascii_case("Platforms") {
                platforms_dirs.push(path.to_path_buf());
            }
        } else if entry.file_type().is_file()
            && path
                .extension()
                .map(|ext| ext.eq_ignore_ascii_case("dep"))
                .unwrap_or(false)
        {
            dep_files.push(path.to_path_buf());
        }
    }

    ArtPayload {
        platforms_dirs,
        dep_files,
    }
}

/// Finds the first `<UpdateArt><Item>` value out of a .modinfo file.
pub fn find_update_art_name(modinfo_path: &Path) -> Option<String> {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let file = fs::File::open(modinfo_path).ok()?;
    let mut sanitized = Vec::new();
    sanitize_xml(file, &mut sanitized).ok()?;

    let mut reader = Reader::from_reader(sanitized.as_slice());
    let mut buf = Vec::new();

    let mut in_update_art = false;
    let mut in_item = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let name = e.name();
                if name.as_ref().eq_ignore_ascii_case(b"UpdateArt") {
                    in_update_art = true;
                } else if in_update_art && name.as_ref().eq_ignore_ascii_case(b"Item") {
                    in_item = true;
                }
            }
            Ok(Event::End(ref e)) => {
                let name = e.name();
                if name.as_ref().eq_ignore_ascii_case(b"UpdateArt") {
                    in_update_art = false;
                } else if name.as_ref().eq_ignore_ascii_case(b"Item") {
                    in_item = false;
                }
            }
            Ok(Event::Text(e)) if in_item => {
                let text = e.unescape().unwrap_or_default().trim().to_string();
                if !text.is_empty() {
                    // Handling if the modder wrote the .dep file name instead of module name. Both SHOULD be the same
                    if text.to_ascii_lowercase().ends_with(".dep") {
                        return Some(text[..text.len() - 4].to_string());
                    }
                    return Some(text);
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(err) => {
                log::warn!("art-dlc: failed to read UpdateArt from modinfo: {err}");
                break;
            }
        }
        buf.clear();
    }
    None
}

/// Device names Windows refuses to use as a folder name.
const RESERVED_WINDOWS_NAMES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Checks the name of the updateArt can work as a folder.
fn validate_folder_name(name: &str) -> Result<String, String> {
    let invalid = |reason: &str| -> Result<String, String> {
        Err(format!(
            "'{name}' cannot be used as a DLC folder name ({reason}). \
             The mod's UpdateArt name has to be usable as a folder name."
        ))
    };

    if name.is_empty() {
        return invalid("it is empty");
    }

    if name
        .chars()
        .any(|c| matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control())
    {
        return invalid("it contains characters that are not allowed in a folder name");
    }

    if name != name.trim() || name.ends_with('.') {
        return invalid("it starts or ends with a space or a dot");
    }

    if name == "." || name == ".." {
        return invalid("it is a relative path");
    }

    let stem = name.split('.').next().unwrap_or(name);
    if RESERVED_WINDOWS_NAMES
        .iter()
        .any(|reserved| stem.eq_ignore_ascii_case(reserved))
    {
        return invalid("it is a reserved Windows device name");
    }

    Ok(name.to_string())
}

/// Copies a directory tree, merging into an existing destination.
fn copy_dir_merged(source: &Path, destination: &Path) -> io::Result<()> {
    fs::create_dir_all(destination)?;

    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());

        if entry.file_type()?.is_dir() {
            copy_dir_merged(&entry.path(), &target)?;
        } else {
            if target.exists() {
                let mut permissions = fs::metadata(&target)?.permissions();
                permissions.set_readonly(false);
                let _ = fs::set_permissions(&target, permissions);
            }
            fs::copy(entry.path(), &target)?;
        }
    }

    Ok(())
}

fn describe_io_error(path: &Path, error: &io::Error) -> String {
    if error.kind() == io::ErrorKind::PermissionDenied {
        format!(
            "When installing art assets, permission was denied writing to '{}'. Try rerunning as administrator.",
            path.display()
        )
    } else {
        format!("Failed to write '{}': {}", path.display(), error)
    }
}


/// install art payload if exists, returns Ok(None) if not.
pub fn install_art_dlc(
    modinfo_dir: &Path,
    archive_root: &Path,
    modinfo_path: &Path,
    fallback_name: &str,
    dlc_folder: &Path,
) -> Result<Option<ArtDlcInstallInfo>, String> {
    let mut payload = find_art_payload(modinfo_dir);
    if payload.is_empty() {
        payload = find_art_payload(archive_root);
    }
    if payload.is_empty() {
        return Ok(None);
    }

    if !dlc_folder.is_dir() {
        return Err(format!(
            "The game's DLC folder '{}' does not exist. Set it in the Settings.",
            dlc_folder.display()
        ));
    }

    let folder_name = match find_update_art_name(modinfo_path) {
        Some(name) => validate_folder_name(&name)?,
        None => validate_folder_name(fallback_name.trim())?,
    };

    let target = dlc_folder.join(&folder_name);
    let marker = target.join(ART_DLC_MARKER_FILE);

    // If a modder made a UpdateArt called one of the regular DLC files, DONT do anything. Dont want to handle
    // maintenance for a list of current DLC.
    if target.exists() && !marker.exists() {
        return Err(format!(
            "When installing art, provided '{}' already exists in the DLC folder and was not installed by CivMods. \
             Skipping art installation to avoid overwriting game files.",
            folder_name
        ));
    }

    fs::create_dir_all(&target).map_err(|e| describe_io_error(&target, &e))?;

    for platforms_dir in &payload.platforms_dirs {
        let destination = target.join("Platforms");
        log::info!(
            "art-dlc: copying '{}' to '{}'",
            platforms_dir.display(),
            destination.display()
        );
        copy_dir_merged(platforms_dir, &destination)
            .map_err(|e| describe_io_error(&destination, &e))?;
    }

    for dep_file in &payload.dep_files {
        let Some(file_name) = dep_file.file_name() else {
            continue;
        };
        let destination = target.join(file_name);
        log::info!(
            "art-dlc: copying '{}' to '{}'",
            dep_file.display(),
            destination.display()
        );
        fs::copy(dep_file, &destination).map_err(|e| describe_io_error(&destination, &e))?;
    }

    fs::write(&marker, folder_name.as_bytes()).map_err(|e| describe_io_error(&marker, &e))?;

    Ok(Some(ArtDlcInstallInfo {
        folder_name,
        installed_path: target.to_string_lossy().to_string(),
        dep_files: payload.dep_files.len(),
        platforms_dirs: payload.platforms_dirs.len(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    const ART_MODINFO: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<Mod id="aaa_mansa" version="1" xmlns="ModInfo">
    <Properties><Name>Mansa Art</Name></Properties>
    <ActionGroups>
        <ActionGroup id="aaa_mansa-game" scope="game" criteria="always">
            <Actions>
                <UpdateArt><Item>aaa_mansa</Item></UpdateArt>
            </Actions>
        </ActionGroup>
    </ActionGroups>
</Mod>"#;

    fn write_payload(dir: &Path, name: &str) {
        let blps = dir.join("Platforms").join("Windows").join("BLPs");
        fs::create_dir_all(&blps).unwrap();
        fs::write(blps.join("art.blp"), b"blp").unwrap();
        fs::write(dir.join(format!("{name}.dep")), b"dep").unwrap();
    }

    /// The common layout: the art sits next to the modinfo, in the folder that
    /// gets copied into Mods.
    fn build_flat_archive(root: &Path, modinfo: &str) -> PathBuf {
        let modinfo_dir = root.join("aaa_mansa");
        fs::create_dir_all(&modinfo_dir).unwrap();
        let modinfo_path = modinfo_dir.join("aaa_mansa.modinfo");
        fs::write(&modinfo_path, modinfo).unwrap();
        write_payload(&modinfo_dir, "aaa_mansa");

        modinfo_path
    }

    fn build_split_archive(root: &Path, modinfo: &str) -> PathBuf {
        let modinfo_dir = root.join("Mods").join("aaa_mansa").join("modules");
        fs::create_dir_all(&modinfo_dir).unwrap();
        let modinfo_path = modinfo_dir.join("aaa_mansa.modinfo");
        fs::write(&modinfo_path, modinfo).unwrap();
        write_payload(&root.join("DLC").join("aaa_mansa"), "aaa_mansa");

        modinfo_path
    }

    /// Calls `install_art_dlc` the way `extract_archive` does.
    fn install(
        archive_root: &Path,
        modinfo_path: &Path,
        fallback_name: &str,
        dlc_folder: &Path,
    ) -> Result<Option<ArtDlcInstallInfo>, String> {
        install_art_dlc(
            modinfo_path.parent().unwrap(),
            archive_root,
            modinfo_path,
            fallback_name,
            dlc_folder,
        )
    }

    fn assert_installed(dlc: &Path, folder_name: &str, dep_name: &str) {
        let installed = dlc.join(folder_name);
        assert!(installed.join(format!("{dep_name}.dep")).exists());
        assert!(installed
            .join("Platforms")
            .join("Windows")
            .join("BLPs")
            .join("art.blp")
            .exists());
    }

    #[test]
    fn test_installs_payload_next_to_the_modinfo() {
        let source = tempdir().unwrap();
        let dlc = tempdir().unwrap();
        let modinfo_path = build_flat_archive(source.path(), ART_MODINFO);

        let info = install(source.path(), &modinfo_path, "civmods-fallback", dlc.path())
            .unwrap()
            .expect("expected an art payload");

        assert_eq!(info.folder_name, "aaa_mansa");
        assert_eq!(info.dep_files, 1);
        assert_eq!(info.platforms_dirs, 1);
        assert_installed(dlc.path(), "aaa_mansa", "aaa_mansa");
    }

    #[test]
    fn test_installs_payload_from_a_sibling_dlc_folder() {
        let source = tempdir().unwrap();
        let dlc = tempdir().unwrap();
        let modinfo_path = build_split_archive(source.path(), ART_MODINFO);

        let info = install(source.path(), &modinfo_path, "civmods-fallback", dlc.path())
            .unwrap()
            .expect("expected an art payload");

        assert_eq!(info.folder_name, "aaa_mansa");
        assert_installed(dlc.path(), "aaa_mansa", "aaa_mansa");
    }

    #[test]
    fn test_falls_back_to_mod_name_without_update_art() {
        let source = tempdir().unwrap();
        let dlc = tempdir().unwrap();
        let modinfo = r#"<Mod id="aaa_mansa"><Properties><Name>Mansa Art</Name></Properties></Mod>"#;
        let modinfo_path = build_flat_archive(source.path(), modinfo);

        let info = install(source.path(), &modinfo_path, "Mansa Art", dlc.path())
            .unwrap()
            .expect("expected an art payload");

        assert_eq!(info.folder_name, "Mansa Art");
        assert_installed(dlc.path(), "Mansa Art", "aaa_mansa");
    }

    #[test]
    fn test_unusable_folder_name_fails() {
        let source = tempdir().unwrap();
        let dlc = tempdir().unwrap();
        let modinfo = r#"<Mod id="x"><ActionGroups><ActionGroup id="a"><Actions>
                         <UpdateArt><Item>Mansa: Art/Vol 2</Item></UpdateArt>
                         </Actions></ActionGroup></ActionGroups></Mod>"#;
        let modinfo_path = build_flat_archive(source.path(), modinfo);

        let error = install(source.path(), &modinfo_path, "aaa_mansa", dlc.path())
            .expect_err("expected an invalid folder name to fail");
        assert!(error.contains("not allowed in a folder name"), "{error}");
        // Nothing half-installed, and no fallback to a name the game won't find
        assert_eq!(fs::read_dir(dlc.path()).unwrap().count(), 0);
    }

    #[test]
    fn test_no_payload_is_not_an_error() {
        let source = tempdir().unwrap();
        let dlc = tempdir().unwrap();
        let modinfo_dir = source.path().join("Mods").join("plain");
        fs::create_dir_all(&modinfo_dir).unwrap();
        let modinfo_path = modinfo_dir.join("plain.modinfo");
        fs::write(&modinfo_path, r#"<Mod id="plain"></Mod>"#).unwrap();

        let result = install(source.path(), &modinfo_path, "plain", dlc.path()).unwrap();
        assert!(result.is_none());
        assert_eq!(fs::read_dir(dlc.path()).unwrap().count(), 0);
    }

    #[test]
    fn test_refuses_to_overwrite_a_foreign_dlc_folder() {
        let source = tempdir().unwrap();
        let dlc = tempdir().unwrap();
        let modinfo_path = build_flat_archive(source.path(), ART_MODINFO);

        // Pretend the game already ships a module with the same name
        let existing = dlc.path().join("aaa_mansa");
        fs::create_dir_all(&existing).unwrap();
        fs::write(existing.join("aaa_mansa.dep"), b"original").unwrap();

        let error = install(source.path(), &modinfo_path, "aaa_mansa", dlc.path())
            .expect_err("expected the install to be refused");
        assert!(error.contains("not installed by CivMods"), "{error}");
        assert_eq!(
            fs::read(existing.join("aaa_mansa.dep")).unwrap(),
            b"original"
        );
    }

    #[test]
    fn test_reinstall_over_own_folder_is_allowed() {
        let source = tempdir().unwrap();
        let dlc = tempdir().unwrap();
        let modinfo_path = build_flat_archive(source.path(), ART_MODINFO);

        install(source.path(), &modinfo_path, "aaa_mansa", dlc.path()).unwrap();
        let info = install(source.path(), &modinfo_path, "aaa_mansa", dlc.path())
            .unwrap()
            .expect("expected an art payload");

        assert_eq!(info.folder_name, "aaa_mansa");
    }

    #[test]
    fn test_validate_folder_name() {
        assert_eq!(validate_folder_name("aaa_mansa").unwrap(), "aaa_mansa");
        assert_eq!(validate_folder_name("Mansa Art").unwrap(), "Mansa Art");

        for invalid in ["", "a/b", "a\\b", "a:b", "a?b", "a|b", "trailing.", " padded", "..", "CON", "nul.dep"] {
            assert!(
                validate_folder_name(invalid).is_err(),
                "expected '{invalid}' to be rejected"
            );
        }
    }

    #[test]
    fn test_update_art_item_with_dep_extension() {
        let dir = tempdir().unwrap();
        let modinfo_path = dir.path().join("x.modinfo");
        fs::write(
            &modinfo_path,
            r#"<Mod id="x"><ActionGroups><ActionGroup id="a"><Actions>
               <UpdateArt><Item>my-art.dep</Item></UpdateArt>
               </Actions></ActionGroup></ActionGroups></Mod>"#,
        )
        .unwrap();

        assert_eq!(
            find_update_art_name(&modinfo_path),
            Some("my-art".to_string())
        );
    }
}

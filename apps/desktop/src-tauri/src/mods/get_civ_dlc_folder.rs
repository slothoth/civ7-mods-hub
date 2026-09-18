//! Finds the DLC folder inside Civ VII game install. Supports Steam for now, dunno about Epic.
use dirs::home_dir;
use std::env::consts::OS;
use std::fs;
use std::path::{Path, PathBuf};

const GAME_FOLDER_NAME: &str = "Sid Meier's Civilization VII";

/// Steam records game libaries in other drives using `libraryfolders.vdf`
fn parse_steam_library_paths(vdf: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    for line in vdf.lines() {
        let line = line.trim();
        let mut parts = line.split('"').filter(|part| !part.trim().is_empty());
        let Some(key) = parts.next() else { continue };
        if !key.eq_ignore_ascii_case("path") {
            continue;
        }
        if let Some(value) = parts.next() {
            paths.push(PathBuf::from(value.replace("\\\\", "\\")));
        }
    }

    paths
}

fn steam_libraries(steam_roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut libraries: Vec<PathBuf> = steam_roots.to_vec();

    for root in steam_roots {
        let vdf_path = root.join("steamapps").join("libraryfolders.vdf");
        let Ok(contents) = fs::read_to_string(&vdf_path) else {
            continue;
        };
        for library in parse_steam_library_paths(&contents) {
            if !libraries.contains(&library) {
                libraries.push(library);
            }
        }
    }

    libraries
}

/// Game folders inside every Steam library reachable from the given Steam roots.
fn steam_game_folders(steam_roots: Vec<PathBuf>) -> Vec<PathBuf> {
    steam_libraries(&steam_roots)
        .into_iter()
        .map(|library| {
            library
                .join("steamapps")
                .join("common")
                .join(GAME_FOLDER_NAME)
        })
        .collect()
}

/// Get candidate game folders. Sort by liklihood if multiple.
fn candidate_game_folders() -> Vec<PathBuf> {
    let home = home_dir();
    let mut candidates = Vec::new();

    match OS {
        "windows" => {
            candidates.extend(steam_game_folders(vec![
                PathBuf::from(r"C:\Program Files (x86)\Steam"),
                PathBuf::from(r"C:\Program Files\Steam"),
            ]));
            candidates.push(PathBuf::from(r"C:\Program Files\Epic Games").join(GAME_FOLDER_NAME));
            candidates.push(
                PathBuf::from(r"C:\XboxGames")
                    .join(GAME_FOLDER_NAME)
                    .join("Content"),
            );
        }
        "macos" => {
            if let Some(home) = home.as_ref() {
                candidates.extend(steam_game_folders(vec![home
                    .join("Library")
                    .join("Application Support")
                    .join("Steam")]));
            }
            candidates.push(
                PathBuf::from("/Applications")
                    .join(format!("{GAME_FOLDER_NAME}.app"))
                    .join("Contents")
                    .join("Resources"),
            );
        }
        "linux" => {
            if let Some(home) = home.as_ref() {
                candidates.extend(steam_game_folders(vec![
                    home.join(".steam").join("steam"),
                    home.join(".local").join("share").join("Steam"),
                    home.join(".var")
                        .join("app")
                        .join("com.valvesoftware.Steam")
                        .join(".local")
                        .join("share")
                        .join("Steam"),
                ]));
            }
        }
        _ => {}
    }

    candidates
}

pub fn get_civ7_dlc_folder() -> Option<PathBuf> {
    candidate_game_folders()
        .into_iter()
        .map(|game_folder| game_folder.join("DLC"))
        .find(|dlc_folder| dlc_folder.is_dir())
}

/// If user selects DLC folder instead of Game folder in dialog, it still works.
pub fn normalize_dlc_folder(selected: &Path) -> Option<PathBuf> {
    if selected
        .file_name()
        .map(|name| name.eq_ignore_ascii_case("DLC"))
        .unwrap_or(false)
    {
        return Some(selected.to_path_buf());
    }

    let nested = selected.join("DLC");
    if nested.is_dir() {
        Some(nested)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_parse_steam_library_paths() {
        let vdf = r#"
        "libraryfolders"
        {
            "0"
            {
                "path"		"C:\\Program Files (x86)\\Steam"
                "label"		""
            }
            "1"
            {
                "path"		"D:\\SteamLibrary"
            }
        }
        "#;

        assert_eq!(
            parse_steam_library_paths(vdf),
            vec![
                PathBuf::from(r"C:\Program Files (x86)\Steam"),
                PathBuf::from(r"D:\SteamLibrary"),
            ]
        );
    }

    #[test]
    fn test_normalize_dlc_folder() {
        let dir = tempdir().unwrap();
        let game_folder = dir.path().join(GAME_FOLDER_NAME);
        let dlc_folder = game_folder.join("DLC");
        fs::create_dir_all(&dlc_folder).unwrap();

        assert_eq!(normalize_dlc_folder(&game_folder), Some(dlc_folder.clone()));
        assert_eq!(normalize_dlc_folder(&dlc_folder), Some(dlc_folder));
        assert_eq!(normalize_dlc_folder(dir.path()), None);
    }
}

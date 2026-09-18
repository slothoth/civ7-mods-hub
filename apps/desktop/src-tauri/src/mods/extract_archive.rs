use flate2::read::GzDecoder;
use sevenz_rust::decompress_file;
use std::fs::{self, File};
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};
use unrar::Archive;
use zip::ZipArchive;
use serde::Serialize;

use super::patch_modinfo::{patch_modinfo_xml, CivModsProperties};
use super::traversal::find_modinfo_file;
use super::art_dlc::{install_art_dlc, ArtDlcInstallInfo};

#[derive(Debug, Default, Serialize)]
pub struct ExtractModArchiveResult {
    pub art_dlc: Option<ArtDlcInstallInfo>,  // when successful art install
    pub art_dlc_error: Option<String>,       // when art files were present but not installed
}

#[tauri::command]
pub async fn extract_mod_archive(
    archive_path: &str,
    extract_path: &str,
    properties: CivModsProperties,
    dlc_folder: Option<String>,
) -> Result<ExtractModArchiveResult, String> {
    let info = extract_archive(&archive_path, &extract_path, &properties, dlc_folder.as_deref())
        .map_err(|e| format!("Failed to extract archive: {}", e))?;

    if let Err(e) = patch_modinfo_xml(info.modinfo_path.clone(), properties) {
        log::error!("Failed to patch modinfo '{}': {}", info.modinfo_path, e);
    }

    Ok(ExtractModArchiveResult {
        art_dlc: info.art_dlc,
        art_dlc_error: info.art_dlc_error,
    })
}

/// Extract ZIP files using `zip 2.x`
fn extract_zip(archive_path: &str, extract_to: &str) -> io::Result<()> {
    let file = File::open(archive_path)?;
    let mut archive = ZipArchive::new(BufReader::new(file))?;

    fs::create_dir_all(extract_to)?;

    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let out_path = Path::new(extract_to).join(file.name());

        if file.is_dir() {
            fs::create_dir_all(&out_path)?;
        } else {
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut outfile = File::create(&out_path)?;
            io::copy(&mut file, &mut outfile)?;
        }
    }

    Ok(())
}

/// Extract 7z files using `sevenz-rust`
fn extract_7z(archive_path: &str, extract_to: &str) -> io::Result<()> {
    fs::create_dir_all(extract_to)?;
    decompress_file(archive_path, extract_to)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    Ok(())
}

/// Extract RAR files using `unrar 0.5.8`
fn extract_rar(archive_path: &str, extract_to: &str) -> io::Result<()> {
    let mut archive = Archive::new(archive_path).open_for_processing().unwrap();

    while let Some(header) = archive
        .read_header()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?
    {
        println!(
            "{} bytes: {}",
            header.entry().unpacked_size,
            header.entry().filename.to_string_lossy(),
        );
        archive = if header.entry().is_file() {
            header
                .extract_with_base(extract_to)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?
        } else {
            header
                .skip()
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?
        };
    }
    Ok(())
}

fn extract_tgz(archive_path: &str, extract_to: &str) -> io::Result<()> {
    // Open the tar.gz file
    let file = File::open(archive_path)?;

    // Wrap it in a buffered reader for efficiency
    let buf_reader = BufReader::new(file);

    // Create a GzDecoder to decompress the gzip layer
    let gz_decoder = GzDecoder::new(buf_reader);

    // Create a tar Archive from the decompressed stream
    let mut archive = tar::Archive::new(gz_decoder);

    // Extract the archive to the output directory
    archive.unpack(extract_to)?;

    println!("Extracted '{}' to '{}'", archive_path, extract_to);
    Ok(())
}

pub struct ExtractArchiveInfo {
    modinfo_path: String,
    modinfo_dir: String,
    art_dlc: Option<ArtDlcInstallInfo>,
    art_dlc_error: Option<String>,
}

/// Extracts any archive format based on file extension
pub fn extract_archive(
    archive_path: &str,
    extract_to: &str,
    properties: &CivModsProperties,
    dlc_folder: Option<&str>,
) -> Result<ExtractArchiveInfo, String> {
    let ext = Path::new(archive_path)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_lowercase();

    // e.g. /path/to/extract_to__temp
    let temp_target = format!("{}__temp", extract_to);

    let result = match ext.as_str() {
        "zip" => extract_zip(archive_path, &temp_target).map_err(|e| e.to_string()),
        "7z" => extract_7z(archive_path, &temp_target).map_err(|e| e.to_string()),
        "rar" => extract_rar(archive_path, &temp_target).map_err(|e| e.to_string()),
        // We should support `.tar.gz` but the extension algorithm doesn't support it right now
        "tgz" | "gz" => extract_tgz(archive_path, &temp_target).map_err(|e| e.to_string()),
        _ => Err(format!("Unsupported file format: {}", ext)),
    };

    if let Err(e) = result {
        return Err(format!("Failed to extract archive: {}", e));
    }

    let mut modinfo_search_dir = PathBuf::from(&temp_target);
    if let Some(target_modinfo_path) = &properties.target_modinfo_path {
        log::info!("Appending variant modinfo path: {}", target_modinfo_path);
        modinfo_search_dir = modinfo_search_dir.join(target_modinfo_path);
    }

    let (modinfo_path, modinfo_xml) = find_modinfo_file(modinfo_search_dir.as_path());
    let modinfo_dir = Path::new(modinfo_path.as_deref().unwrap())
        .parent()
        .ok_or("Modinfo file not found")?;

    println!("Modinfo directory: {:?}", modinfo_dir);

    // Install any art assets.
    let mut art_dlc = None;
    let mut art_dlc_error = None;
    if let Some(dlc_folder) = dlc_folder {
        let fallback_name = modinfo_xml
            .as_ref()
            .and_then(|xml| xml.properties.name.clone())
            .or_else(|| modinfo_xml.as_ref().and_then(|xml| xml.id.clone()))
            .unwrap_or_else(|| {
                Path::new(extract_to)
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_default()
            });

        match install_art_dlc(
            modinfo_dir,
            Path::new(&temp_target),
            Path::new(modinfo_path.as_deref().unwrap()),
            &fallback_name,
            Path::new(dlc_folder),
        ) {
            Ok(Some(info)) => {
                log::info!("art-dlc: installed art files to '{}'", info.installed_path);
                art_dlc = Some(info);
            }
            Ok(None) => {}
            Err(e) => {
                log::error!("art-dlc: failed to install art files: {}", e);
                art_dlc_error = Some(e);
            }
        }
    }

    recursively_grant_write_permissions(modinfo_dir)
        .map_err(|e| format!("Failed to grant write permissions: {}", e))?;

    // Copy the modinfo_dir directory to the target directory
    let _ = fs::create_dir_all(extract_to);

    let mut copy_options = fs_extra::dir::CopyOptions::new();
    copy_options.overwrite = true;
    copy_options.content_only = true;

    fs_extra::dir::copy(modinfo_dir, extract_to, &copy_options)
        .map_err(|e| format!("Failed to copy modinfo directory: {}", e))?;

    // Remove the temp directory
    fs::remove_dir_all(&temp_target)
        .map_err(|e| format!("Failed to remove temp directory: {}", e))?;

    // Find the modinfo file and return the path
    let (updated_modinfo_path, _) = find_modinfo_file(Path::new(extract_to));

    Ok(ExtractArchiveInfo {
        modinfo_path: updated_modinfo_path.ok_or("Modinfo file not found")?,
        modinfo_dir: extract_to.to_string(),
        art_dlc,
        art_dlc_error,
    })
}

pub(crate) fn recursively_grant_write_permissions(root: &Path) -> io::Result<()> {
    let mut files = vec![root.to_path_buf()];
    while let Some(file) = files.pop() {
        let mut permissions = fs::metadata(&file)?.permissions();
        permissions.set_readonly(false);
        fs::set_permissions(&file, permissions)?;
        if file.is_dir() {
            for entry in fs::read_dir(file)? {
                files.push(entry?.path());
            }
        }
    }
    Ok(())
}

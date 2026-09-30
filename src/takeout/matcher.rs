use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;
use tracing::{debug, warn};
use super::models::{MediaFilePair, TakeoutJsonMetadata};

pub const PHOTO_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "webp", "heic", "heif", "gif", "bmp", "tiff", "tif", "dng", "cr2", "nef", "arw"
];

pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mov", "m4v", "avi", "mkv", "3gp", "wmv", "mpg", "mpeg"
];

pub fn is_media_extension(ext: &str) -> bool {
    let lower = ext.to_lowercase();
    PHOTO_EXTENSIONS.contains(&lower.as_str()) || VIDEO_EXTENSIONS.contains(&lower.as_str())
}

pub struct TakeoutScanner {
    root_dir: PathBuf,
}

impl TakeoutScanner {
    pub fn new<P: AsRef<Path>>(root_dir: P) -> Self {
        Self {
            root_dir: root_dir.as_ref().to_path_buf(),
        }
    }

    pub fn scan(&self, prefer_edited: bool) -> (Vec<MediaFilePair>, HashMap<String, Vec<String>>) {
        let mut album_collaborators: HashMap<String, Vec<String>> = HashMap::new();
        let mut media_files: Vec<PathBuf> = Vec::new();
        let mut json_files: HashMap<PathBuf, HashSet<PathBuf>> = HashMap::new(); // parent_dir -> set of json files

        // First pass: collect all media files and JSON files grouped by directory
        for entry in WalkDir::new(&self.root_dir).into_iter().filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                    let ext_lower = ext.to_lowercase();
                    if ext_lower == "json" {
                        if let Some(parent) = path.parent() {
                            json_files
                                .entry(parent.to_path_buf())
                                .or_default()
                                .insert(path.to_path_buf());
                        }

                        // Check if this is an album metadata / collaborator file
                        let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
                        if file_name == "shared_album_comments.json" || file_name == "metadata.json" {
                            if let Some(album_name) = self.infer_album_name(path) {
                                if let Ok(content) = fs::read_to_string(path) {
                                    let collabs = extract_collaborators_from_json(&content);
                                    if !collabs.is_empty() {
                                        album_collaborators.entry(album_name).or_default().extend(collabs);
                                    }
                                }
                            }
                        }
                    } else if is_media_extension(&ext_lower) {
                        media_files.push(path.to_path_buf());
                    }
                }
            }
        }

        // Deduplicate collaborator lists
        for collabs in album_collaborators.values_mut() {
            collabs.sort();
            collabs.dedup();
        }

        // Second pass: pair media files with sidecars and resolve edited pairs
        let mut pairs: Vec<MediaFilePair> = Vec::new();

        // Group media files by stem in the same directory to detect edited versions
        let mut dir_media_map: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
        for m in media_files {
            if let Some(parent) = m.parent() {
                dir_media_map.entry(parent.to_path_buf()).or_default().push(m);
            }
        }

        for (dir, files) in dir_media_map {
            let empty_set = HashSet::new();
            let dir_jsons = json_files.get(&dir).unwrap_or(&empty_set);

            // Separate edited vs unedited files
            let mut edited_map: HashMap<String, PathBuf> = HashMap::new();
            let mut original_map: HashMap<String, PathBuf> = HashMap::new();

            for f in &files {
                let file_name = f.file_name().and_then(|s| s.to_str()).unwrap_or("");
                if is_edited_filename(file_name) {
                    let base_name = strip_edited_suffix(file_name);
                    edited_map.insert(base_name, f.clone());
                } else {
                    original_map.insert(file_name.to_string(), f.clone());
                }
            }

            for f in &files {
                let file_name = f.file_name().and_then(|s| s.to_str()).unwrap_or("");
                let is_edited = is_edited_filename(file_name);

                // If user prefers edited and this is an original that has a corresponding edited version, skip original
                if prefer_edited && !is_edited {
                    let base_name = file_name.to_string();
                    if edited_map.contains_key(&base_name) {
                        debug!("Skipping original {} in favor of edited version", file_name);
                        continue;
                    }
                }

                // If user prefers original and this is an edited version, skip edited
                if !prefer_edited && is_edited {
                    let base_name = strip_edited_suffix(file_name);
                    if original_map.contains_key(&base_name) {
                        debug!("Skipping edited {} in favor of original", file_name);
                        continue;
                    }
                }

                // Find matching JSON sidecar
                let sidecar = find_matching_json(f, dir_jsons);
                let metadata = sidecar.as_ref().and_then(|p| {
                    match fs::read_to_string(p) {
                        Ok(content) => serde_json::from_str::<TakeoutJsonMetadata>(&content).ok(),
                        Err(e) => {
                            warn!("Failed to read JSON metadata at {:?}: {}", p, e);
                            None
                        }
                    }
                });

                let album_name = self.infer_album_name(f);

                pairs.push(MediaFilePair {
                    media_path: f.clone(),
                    sidecar_path: sidecar,
                    album_name,
                    is_edited_version: is_edited,
                    metadata,
                });
            }
        }

        (pairs, album_collaborators)
    }

    fn infer_album_name(&self, path: &Path) -> Option<String> {
        let parent = path.parent()?;
        let folder_name = parent.file_name()?.to_str()?;

        // If folder is the root takeout or standard year folder, it's not a custom album
        if parent == self.root_dir {
            return None;
        }

        // Google Takeout default root is often "Google Photos" or year folders like "Photos from 2021"
        if folder_name.eq_ignore_ascii_case("Google Photos")
            || folder_name.eq_ignore_ascii_case("Trash")
            || folder_name.starts_with("Photos from ")
        {
            return None;
        }

        Some(folder_name.to_string())
    }
}

pub fn is_edited_filename(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.contains("-edited.") || lower.contains("_edited.") || lower.contains(" - edited.")
}

pub fn strip_edited_suffix(name: &str) -> String {
    // e.g. "IMG_0001-edited.jpg" -> "IMG_0001.jpg"
    let re = regex::Regex::new(r"[-_ ]edited(\.[^.]+)$").unwrap();
    re.replace(name, "$1").to_string()
}

pub fn find_matching_json(media_path: &Path, dir_jsons: &HashSet<PathBuf>) -> Option<PathBuf> {
    let file_name = media_path.file_name()?.to_str()?;
    let parent = media_path.parent()?;

    // 1. Exact match: <file_name>.json (e.g. photo.jpg -> photo.jpg.json)
    let candidate1 = parent.join(format!("{}.json", file_name));
    if dir_jsons.contains(&candidate1) {
        return Some(candidate1);
    }

    // 2. Base match without media extension: <stem>.json (e.g. photo.jpg -> photo.json)
    if let Some(stem) = media_path.file_stem().and_then(|s| s.to_str()) {
        let candidate2 = parent.join(format!("{}.json", stem));
        if dir_jsons.contains(&candidate2) {
            return Some(candidate2);
        }

        // 3. Numbered duplicates handling:
        // photo(1).jpg -> photo.jpg(1).json
        let re_num = regex::Regex::new(r"^(.*?)\((\d+)\)$").unwrap();
        if let Some(caps) = re_num.captures(stem) {
            let base_stem = caps.get(1).unwrap().as_str();
            let num = caps.get(2).unwrap().as_str();
            let ext = media_path.extension().and_then(|s| s.to_str()).unwrap_or("");
            let candidate3 = parent.join(format!("{}.{}({}).json", base_stem, ext, num));
            if dir_jsons.contains(&candidate3) {
                return Some(candidate3);
            }
        }

        // 4. Truncation match: Google Takeout truncates filenames after ~47-51 chars
        let prefix = if stem.len() > 45 { &stem[..45] } else { stem };
        for json_path in dir_jsons {
            if let Some(j_stem) = json_path.file_stem().and_then(|s| s.to_str()) {
                if j_stem.starts_with(prefix) {
                    return Some(json_path.clone());
                }
            }
        }
    }

    None
}

fn extract_collaborators_from_json(json_content: &str) -> Vec<String> {
    let mut names = Vec::new();
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_content) {
        // Parse arrays or objects with "name", "author", "creator"
        if let Some(arr) = val.as_array() {
            for item in arr {
                extract_names_recursive(item, &mut names);
            }
        } else if val.is_object() {
            extract_names_recursive(&val, &mut names);
        }
    }
    names
}

fn extract_names_recursive(val: &serde_json::Value, names: &mut Vec<String>) {
    if let Some(obj) = val.as_object() {
        if let Some(name) = obj.get("displayName").or_else(|| obj.get("name")).and_then(|v| v.as_str()) {
            if !name.trim().is_empty() && !names.contains(&name.to_string()) {
                names.push(name.trim().to_string());
            }
        }
        for v in obj.values() {
            extract_names_recursive(v, names);
        }
    } else if let Some(arr) = val.as_array() {
        for v in arr {
            extract_names_recursive(v, names);
        }
    }
}

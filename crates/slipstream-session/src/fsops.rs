//! Local filesystem helpers for file browsing and transfer: listings,
//! shortcut places, and safe destination paths.

use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use slipstream_core::proto::{DirEntry, DirListing, Place};

pub fn home() -> PathBuf {
    directories::UserDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .unwrap_or_else(std::env::temp_dir)
}

/// Where received files go by default: the Desktop, else Downloads, else home.
pub fn default_upload_dir() -> PathBuf {
    let dirs = directories::UserDirs::new();
    dirs.as_ref()
        .and_then(|d| d.desktop_dir().map(Path::to_path_buf))
        .filter(|p| p.is_dir())
        .or_else(|| dirs.as_ref().and_then(|d| d.download_dir().map(Path::to_path_buf)).filter(|p| p.is_dir()))
        .unwrap_or_else(home)
}

/// Shortcut folders and drives for the side panel / places menu.
pub fn places() -> Vec<Place> {
    let mut out = Vec::new();
    let mut add = |label: &str, p: Option<&Path>| {
        if let Some(p) = p.filter(|p| p.is_dir()) {
            let path = p.display().to_string();
            if !out.iter().any(|x: &Place| x.path == path) {
                out.push(Place { label: label.into(), path });
            }
        }
    };
    let home = home();
    add("Home", Some(&home));
    if let Some(d) = directories::UserDirs::new() {
        add("Desktop", d.desktop_dir());
        add("Documents", d.document_dir());
        add("Downloads", d.download_dir());
        add("Pictures", d.picture_dir());
    }
    #[cfg(windows)]
    for letter in b'A'..=b'Z' {
        let root = format!("{}:\\", letter as char);
        if Path::new(&root).exists() {
            add(&format!("Drive {}:", letter as char), Some(Path::new(&root)));
        }
    }
    #[cfg(unix)]
    {
        add("Computer", Some(Path::new("/")));
        let user = std::env::var("USER").unwrap_or_default();
        for base in [format!("/media/{user}"), format!("/run/media/{user}"), "/Volumes".into()] {
            if let Ok(rd) = std::fs::read_dir(base) {
                for e in rd.flatten() {
                    let p = e.path();
                    add(&e.file_name().to_string_lossy(), Some(&p));
                }
            }
        }
    }
    out
}

/// Lists a folder (`None` = home). Folders first, then files, by name.
pub fn list(path: Option<&str>) -> Result<DirListing> {
    let dir = match path {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => home(),
    };
    let dir = dir.canonicalize().with_context(|| format!("cannot open {}", dir.display()))?;
    let rd = std::fs::read_dir(&dir).with_context(|| format!("cannot read {}", dir.display()))?;
    let mut entries: Vec<DirEntry> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            // Follow symlinks for type/size; skip broken ones.
            let meta = std::fs::metadata(e.path()).ok()?;
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            Some(DirEntry { name, is_dir: meta.is_dir(), size: if meta.is_dir() { 0 } else { meta.len() }, modified })
        })
        .collect();
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(DirListing {
        path: display(&dir),
        parent: dir.parent().map(display),
        sep: std::path::MAIN_SEPARATOR.to_string(),
        entries,
    })
}

/// Paths as users expect to see them (no `\\?\` prefix on Windows).
pub fn display(p: &Path) -> String {
    let s = p.display().to_string();
    s.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(s)
}

/// Turns a peer-supplied relative path into a safe one: only plain names,
/// no `..`, roots or drive prefixes, and no characters Windows forbids.
pub fn safe_relative(rel: &str) -> Result<PathBuf> {
    let mut out = PathBuf::new();
    for part in rel.split(['/', '\\']) {
        if part.is_empty() || part == "." {
            continue;
        }
        let clean: String = part
            .chars()
            .filter(|c| !c.is_control() && !matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|'))
            .collect();
        let clean = clean.trim().trim_end_matches('.').to_string();
        if clean.is_empty() || clean == ".." {
            bail!("invalid file name in {rel:?}");
        }
        if !matches!(Path::new(&clean).components().next(), Some(Component::Normal(_))) {
            bail!("invalid file name in {rel:?}");
        }
        out.push(clean);
    }
    if out.as_os_str().is_empty() {
        bail!("empty file name");
    }
    Ok(out)
}

/// `path`, or `path (1)`, `path (2)`… if it exists.
pub fn unique_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let dir = path.parent().unwrap_or(Path::new(""));
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name, String::new()),
    };
    (1..).map(|i| dir.join(format!("{stem} ({i}){ext}"))).find(|p| !p.exists()).expect("unbounded")
}

/// The temporary folder for one paste of copied files. Older ones (from a
/// day ago) are removed when a new one is made.
pub fn paste_dir(batch: u64) -> PathBuf {
    let root = std::env::temp_dir().join("Slipstream Remote paste");
    if let Ok(entries) = std::fs::read_dir(&root) {
        for e in entries.flatten() {
            let old = e
                .metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|t| t.elapsed().is_ok_and(|d| d.as_secs() > 24 * 3600));
            if old {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    root.join(format!("{batch:016x}"))
}

/// What a list of copied files amounts to: top-level items, total bytes
/// and file count (stops counting past `limit` files).
pub fn summarize(paths: &[PathBuf], limit: u64) -> (Vec<slipstream_core::proto::ClipFile>, u64, u64) {
    let (mut bytes, mut count) = (0u64, 0u64);
    let mut items = Vec::new();
    for p in paths {
        let is_dir = p.is_dir();
        let mut size = 0;
        let mut stack = vec![p.clone()];
        while let Some(q) = stack.pop() {
            if count > limit {
                break;
            }
            match std::fs::metadata(&q) {
                Ok(m) if m.is_dir() => {
                    stack.extend(std::fs::read_dir(&q).into_iter().flatten().flatten().map(|e| e.path()))
                }
                Ok(m) => {
                    size += m.len();
                    count += 1;
                }
                Err(_) => {}
            }
        }
        bytes += size;
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        items.push(slipstream_core::proto::ClipFile { path: display(p), name, is_dir, size });
    }
    (items, bytes, count)
}

/// All files under `root` (recursively) with their paths relative to the
/// parent of `root`, e.g. `Photos/2026/a.jpg`. A plain file yields itself.
pub fn walk_local(root: &Path) -> Result<Vec<(PathBuf, String, u64)>> {
    let base = root.parent().unwrap_or(Path::new(""));
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(p) = stack.pop() {
        let meta = std::fs::metadata(&p)?;
        if meta.is_dir() {
            for e in std::fs::read_dir(&p)?.flatten() {
                stack.push(e.path());
            }
        } else {
            let rel = p.strip_prefix(base).unwrap_or(&p);
            let rel = rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
            out.push((p.clone(), rel, meta.len()));
        }
    }
    Ok(out)
}

/// An upload plan includes directories so empty folders survive a drop.
#[derive(Default)]
pub struct LocalTree {
    pub files: Vec<(PathBuf, String, u64)>,
    pub directories: Vec<String>,
}

pub fn walk_local_tree(root: &Path) -> Result<LocalTree> {
    let base = root.parent().unwrap_or(Path::new(""));
    let mut out = LocalTree::default();
    let mut stack = vec![root.to_path_buf()];
    let mut visited = std::collections::HashSet::new();
    while let Some(path) = stack.pop() {
        let metadata = std::fs::metadata(&path).with_context(|| format!("reading {}", path.display()))?;
        let rel = path.strip_prefix(base)?.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
        if metadata.is_dir() {
            // Follow useful symlinks, but stop a link back into an ancestor.
            if !visited.insert(std::fs::canonicalize(&path)?) { continue; }
            out.directories.push(rel);
            let mut children = std::fs::read_dir(&path)?.map(|entry| entry.map(|e| e.path())).collect::<std::io::Result<Vec<_>>>()?;
            children.sort();
            stack.extend(children.into_iter().rev());
        } else if metadata.is_file() {
            out.files.push((path, rel, metadata.len()));
        } else {
            bail!("{} is not a regular file or folder", path.display());
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_are_contained() {
        assert_eq!(safe_relative("a/b.txt").unwrap(), PathBuf::from("a").join("b.txt"));
        assert_eq!(safe_relative("dir\\\\x.bin").unwrap(), PathBuf::from("dir").join("x.bin"));
        assert!(safe_relative("../etc/passwd").is_err());
        assert!(safe_relative("a/../../x").is_err());
        assert_eq!(safe_relative("/etc/passwd").unwrap(), PathBuf::from("etc").join("passwd"));
        assert_eq!(safe_relative("C:/Windows/x").unwrap(), PathBuf::from("C").join("Windows").join("x"));
        assert!(safe_relative("").is_err());
    }

    #[test]
    fn dropped_folders_keep_empty_directories_and_stop_symlink_cycles() {
        let root = std::env::temp_dir().join(format!("slipstream-drop-tree-{}", std::process::id()));
        std::fs::create_dir_all(root.join("empty/nested")).unwrap();
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(root.join("data/note.txt"), b"hello").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&root, root.join("data/back-to-root")).unwrap();
        let tree = walk_local_tree(&root).unwrap();
        let name = root.file_name().unwrap().to_str().unwrap();
        assert_eq!(tree.directories, [name.to_owned(), format!("{name}/data"), format!("{name}/empty"), format!("{name}/empty/nested")]);
        assert_eq!(tree.files.len(), 1);
        assert_eq!(tree.files[0].1, format!("{name}/data/note.txt"));
        assert_eq!(tree.files[0].2, 5);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn listing_and_walk() {
        let dir = std::env::temp_dir().join(format!("slipstream-fsops-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("b.txt"), b"bb").unwrap();
        std::fs::write(dir.join("sub/a.txt"), b"a").unwrap();
        let l = list(Some(dir.to_str().unwrap())).unwrap();
        assert_eq!(l.entries[0].name, "sub");
        assert!(l.entries[0].is_dir);
        assert_eq!(l.entries[1].size, 2);
        let mut files: Vec<String> = walk_local(&dir).unwrap().into_iter().map(|f| f.1).collect();
        files.sort();
        let top = dir.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(files, vec![format!("{top}/b.txt"), format!("{top}/sub/a.txt")]);
        assert_eq!(unique_path(&dir.join("b.txt")), dir.join("b (1).txt"));
        std::fs::remove_dir_all(dir).ok();
    }
}

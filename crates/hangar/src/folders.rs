//! Folder browsing for the app's folder picker.
//!
//! A web page cannot learn a folder's real path from the browser, so the
//! daemon lists folders for the signed-in app. Only folder names are listed,
//! never files or contents.

use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// The most entries one listing returns.
pub const MAX_ENTRIES: usize = 300;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Entry {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Listing {
    /// The folder whose children are listed; `None` at the top level
    /// (home and drives).
    pub dir: Option<String>,
    /// The requested path is an existing folder (so it can be used as is).
    pub exists: bool,
    pub parent: Option<String>,
    /// Child folders of `dir` (filtered by the last path segment when the
    /// path does not exist yet), or the top-level places.
    pub entries: Vec<Entry>,
    pub truncated: bool,
}

/// Lists what the picker should show for a typed `path`:
/// * empty: home and every drive;
/// * an existing folder: its child folders;
/// * anything else: the child folders of its parent whose names start with
///   the last segment (case-insensitively), like a shell completing a path.
pub fn list(path: &str, home: &Path) -> Listing {
    let typed = path.trim();
    if typed.is_empty() {
        return Listing { dir: None, exists: false, parent: None, entries: places(home), truncated: false };
    }
    let p = expand(typed, home);
    if p.is_dir() {
        let (entries, truncated) = children(&p, "");
        return Listing { dir: Some(show(&p)), exists: true, parent: p.parent().map(show), entries, truncated };
    }
    let (Some(parent), Some(name)) = (p.parent().filter(|d| !d.as_os_str().is_empty() && d.is_dir()), p.file_name()) else {
        return Listing { dir: None, exists: false, parent: None, entries: Vec::new(), truncated: false };
    };
    let (entries, truncated) = children(parent, &name.to_string_lossy());
    Listing { dir: Some(show(parent)), exists: false, parent: parent.parent().map(show), entries, truncated }
}

/// Creates one new folder whose parent exists. Returns its path.
pub fn create(path: &str, home: &Path) -> Result<String> {
    let p = expand(path.trim(), home);
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).context("give the new folder a name")?;
    if !valid_name(&name) {
        bail!("{name:?} is not a valid folder name");
    }
    let parent = p.parent().filter(|d| d.is_dir()).with_context(|| format!("{} does not exist", p.parent().map(show).unwrap_or_default()))?;
    if p.exists() {
        bail!("{} already exists", show(&p));
    }
    std::fs::create_dir(parent.join(&name)).with_context(|| format!("creating {}", show(&p)))?;
    Ok(show(&p))
}

fn valid_name(name: &str) -> bool {
    let bad = |c: char| c.is_control() || (cfg!(windows) && r#"<>:"/\|?*"#.contains(c)) || c == '/';
    !name.is_empty() && name != "." && name != ".." && !name.chars().any(bad) && !name.ends_with(' ') && !(cfg!(windows) && name.ends_with('.'))
}

/// `~` means the home folder; on Windows `C:` means `C:\`.
fn expand(typed: &str, home: &Path) -> PathBuf {
    if typed == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = typed.strip_prefix("~/").or_else(|| typed.strip_prefix("~\\")) {
        return home.join(rest);
    }
    if cfg!(windows) && typed.len() == 2 && typed.ends_with(':') {
        return PathBuf::from(format!("{typed}\\"));
    }
    PathBuf::from(typed)
}

fn show(p: &Path) -> String {
    p.display().to_string()
}

/// Child folders of `dir` whose names start with `prefix`, hidden ones only
/// when asked for by name.
fn children(dir: &Path, prefix: &str) -> (Vec<Entry>, bool) {
    let want = prefix.to_lowercase();
    let Ok(read) = std::fs::read_dir(dir) else { return (Vec::new(), false) };
    let mut out: Vec<Entry> = read
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if !name.to_lowercase().starts_with(&want) || (is_hidden(&e, &name) && !prefix.starts_with('.')) {
                return None;
            }
            // Follows links, so a linked folder counts as a folder.
            e.path().is_dir().then(|| Entry { path: show(&e.path()), name })
        })
        .collect();
    out.sort_by_key(|e| e.name.to_lowercase());
    let truncated = out.len() > MAX_ENTRIES;
    out.truncate(MAX_ENTRIES);
    (out, truncated)
}

fn is_hidden(entry: &std::fs::DirEntry, name: &str) -> bool {
    if name.starts_with('.') {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const HIDDEN: u32 = 0x2;
        const SYSTEM: u32 = 0x4;
        if let Ok(meta) = entry.metadata() {
            return meta.file_attributes() & (HIDDEN | SYSTEM) != 0;
        }
    }
    let _ = entry;
    false
}

/// Home, then every drive (Windows) or the filesystem root.
fn places(home: &Path) -> Vec<Entry> {
    let mut out = vec![Entry { name: "Home".into(), path: show(home) }];
    #[cfg(windows)]
    {
        // A bitmask of drive letters; asking it never touches the drives.
        let mask = unsafe { windows_sys::Win32::Storage::FileSystem::GetLogicalDrives() };
        for (i, letter) in ('A'..='Z').enumerate() {
            if mask & (1 << i) != 0 {
                out.push(Entry { name: format!("{letter}:"), path: format!("{letter}:\\") });
            }
        }
    }
    #[cfg(not(windows))]
    out.push(Entry { name: "/".into(), path: "/".into() });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> PathBuf {
        let d = std::env::temp_dir().join(format!("hangar-folders-{}-{}", std::process::id(), crate::util::random_hex(3)));
        for sub in ["Alpha", "apps", "beta", ".git", "beta/inner"] {
            std::fs::create_dir_all(d.join(sub)).unwrap();
        }
        std::fs::write(d.join("a-file.txt"), "not a folder").unwrap();
        d
    }

    fn names(l: &Listing) -> Vec<&str> {
        l.entries.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn an_existing_folder_lists_its_visible_child_folders_only() {
        let d = tree();
        let l = list(&d.display().to_string(), Path::new("/home"));
        assert!(l.exists);
        assert_eq!(l.dir.as_deref(), Some(d.display().to_string().as_str()));
        assert_eq!(names(&l), ["Alpha", "apps", "beta"], "sorted, no files, no hidden folders");
        assert_eq!(l.entries[2].path, d.join("beta").display().to_string());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_partial_name_completes_against_its_parent() {
        let d = tree();
        let l = list(&d.join("a").display().to_string(), Path::new("/home"));
        assert!(!l.exists);
        assert_eq!(names(&l), ["Alpha", "apps"], "case-insensitive prefix");
        assert_eq!(names(&list(&d.join(".g").display().to_string(), Path::new("/home"))), [".git"], "hidden folders when asked for");
        assert!(list(&d.join("nope/deeper").display().to_string(), Path::new("/home")).entries.is_empty());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn the_top_level_is_home_and_drives() {
        let l = list("  ", Path::new("/home/me"));
        assert_eq!(l.dir, None);
        assert_eq!(l.entries[0], Entry { name: "Home".into(), path: "/home/me".into() });
        assert!(l.entries.len() >= 2, "at least one drive or the root");
    }

    #[test]
    fn tilde_means_home() {
        let d = tree();
        let l = list("~", &d);
        assert!(l.exists);
        assert_eq!(names(&l), ["Alpha", "apps", "beta"]);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn create_makes_one_new_folder_and_refuses_the_rest() {
        let d = tree();
        let made = create(&d.join("new-app").display().to_string(), Path::new("/home")).unwrap();
        assert!(Path::new(&made).is_dir());
        assert!(create(&made, Path::new("/home")).unwrap_err().to_string().contains("already exists"));
        assert!(create(&d.join("missing").join("child").display().to_string(), Path::new("/home")).is_err(), "parents are not created");
        assert!(create(&d.join("..").display().to_string(), Path::new("/home")).is_err());
        assert!(!valid_name("a:b") || !cfg!(windows));
        assert!(!valid_name("x\ny"));
        let _ = std::fs::remove_dir_all(d);
    }
}

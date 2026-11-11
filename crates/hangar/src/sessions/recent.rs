//! Recent folders for the new-session picker: where hangar sessions ran and
//! where Claude Code was used, most recent first.

use serde_json::Value;
use std::cmp::Reverse;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// How many of Claude Code's most recent folders are read.
const CLAUDE_FOLDERS: usize = 30;

/// Claude Code keeps its transcripts in `~/.claude/projects`, next to
/// `~/.claude.json`.
pub fn transcripts_dir(claude_json: &Path) -> PathBuf {
    claude_json.parent().unwrap_or(Path::new(".")).join(".claude").join("projects")
}

/// Folders Claude Code has run in, newest first, dated by their newest
/// transcript. The transcript folder names are lossy (every symbol became
/// `-`), so the real path comes from inside the newest transcript.
pub fn claude_folders(transcripts: &Path) -> Vec<(u64, String)> {
    let Ok(read) = std::fs::read_dir(transcripts) else { return Vec::new() };
    let mut newest: Vec<(u64, PathBuf)> = read
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| {
            std::fs::read_dir(e.path())
                .ok()?
                .flatten()
                .filter(|f| f.path().extension().is_some_and(|x| x == "jsonl"))
                .filter_map(|f| Some((modified_ms(&f.metadata().ok()?)?, f.path())))
                .max_by_key(|f| f.0)
        })
        .collect();
    newest.sort_by_key(|n| Reverse(n.0));
    newest.into_iter().take(CLAUDE_FOLDERS).filter_map(|(when, file)| Some((when, cwd_of(&file)?))).collect()
}

fn modified_ms(meta: &std::fs::Metadata) -> Option<u64> {
    Some(meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_millis() as u64)
}

/// The folder a transcript was recorded in (its first lines carry `cwd`).
fn cwd_of(transcript: &Path) -> Option<String> {
    let file = std::fs::File::open(transcript).ok()?;
    BufReader::new(file.take(256 * 1024))
        .lines()
        .take(50)
        .map_while(Result::ok)
        .find_map(|line| serde_json::from_str::<Value>(&line).ok()?.get("cwd")?.as_str().map(String::from))
}

/// The first `max` distinct folders that still exist, leaving out
/// throwaway ones under the temp folder.
pub fn pick(candidates: impl IntoIterator<Item = String>, max: usize, temp: &Path) -> Vec<String> {
    let key = |p: &str| p.replace('\\', "/").trim_end_matches('/').to_lowercase();
    let temp = key(&temp.display().to_string());
    let mut out: Vec<String> = Vec::new();
    for c in candidates {
        if out.len() >= max {
            break;
        }
        let k = key(&c);
        let in_temp = k == temp || k.strip_prefix(&temp).is_some_and(|rest| rest.starts_with('/'));
        if in_temp || out.iter().any(|o| key(o) == k) || !Path::new(&c).is_dir() {
            continue;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hangar-recent-{tag}-{}-{}", std::process::id(), crate::util::random_hex(3)));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn claude_folders_come_from_the_newest_transcripts_with_their_real_paths() {
        let root = tmp("claude");
        let write = |dir: &str, file: &str, cwd: &str, age_s: u64| {
            let d = root.join(dir);
            std::fs::create_dir_all(&d).unwrap();
            let path = d.join(file);
            let first = serde_json::json!({"type": "summary", "summary": "x"});
            let second = serde_json::json!({"type": "user", "cwd": cwd, "message": {"content": "hi"}});
            std::fs::write(&path, format!("{first}\n{second}\n")).unwrap();
            let when = std::time::SystemTime::now() - std::time::Duration::from_secs(age_s);
            std::fs::File::options().write(true).open(&path).unwrap().set_modified(when).unwrap();
        };
        write("C--code-old", "a.jsonl", r"C:\code\old", 500);
        write("C--code-my-app", "a.jsonl", r"C:\code\my-app", 400);
        write("C--code-my-app", "b.jsonl", r"C:\code\my-app", 10);
        write("C--code-mid", "a.jsonl", r"C:\code\mid", 100);
        std::fs::create_dir_all(root.join("empty")).unwrap();
        let got: Vec<String> = claude_folders(&root).into_iter().map(|(_, c)| c).collect();
        assert_eq!(got, [r"C:\code\my-app", r"C:\code\mid", r"C:\code\old"], "dated by the newest transcript");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn pick_keeps_existing_distinct_folders_outside_temp() {
        let root = tmp("pick");
        let (a, b) = (root.join("a"), root.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let s = |p: &Path| p.display().to_string();
        let other_temp = tmp("other");
        let got = pick([s(&a), s(&a).to_uppercase(), s(&root.join("gone")), s(&b), s(&other_temp)], 5, &other_temp);
        assert_eq!(got, [s(&a), s(&b)]);
        assert_eq!(pick([s(&a), s(&b)], 1, Path::new("/nowhere")), [s(&a)]);
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(other_temp);
    }
}

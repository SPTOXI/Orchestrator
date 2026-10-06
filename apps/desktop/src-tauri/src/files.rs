//! The app's own files (ADR-0022): written so that an update, a crash or a
//! full disk never leaves one half written, and never written over when
//! the app could not read them.

use chrono::Utc;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Writes `bytes` to `path` through a temporary file in the same folder and
/// a rename: the file is either the old one or the new one, never half of
/// each.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let dir = path
        .parent()
        .ok_or_else(|| format!("{} has no folder", path.display()))?;
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let name = path
        .file_name()
        .ok_or_else(|| format!("{} has no name", path.display()))?
        .to_string_lossy();
    let tmp = dir.join(format!(".{name}.tmp-{}", std::process::id()));
    let written = std::fs::File::create(&tmp).and_then(|mut file| {
        file.write_all(bytes)?;
        file.sync_all()
    });
    if let Err(err) = written.and_then(|()| std::fs::rename(&tmp, path)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("cannot write {}: {err}", path.display()));
    }
    Ok(())
}

/// Keeps a copy of a file the app could not use before anything writes
/// over it: `<name>.unreadable-<when>`, next to it. Returns the copy (or an
/// earlier one with the same content); `None` when there is no file.
pub fn keep_unreadable(path: &Path) -> Result<Option<PathBuf>, String> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("cannot read {}: {err}", path.display())),
    };
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let prefix = format!("{name}.unreadable-");
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let kept = entry.path();
            if entry.file_name().to_string_lossy().starts_with(&prefix)
                && std::fs::read(&kept).is_ok_and(|old| old == bytes)
            {
                return Ok(Some(kept));
            }
        }
    }
    let stamp = Utc::now().format("%Y%m%d-%H%M%S");
    let mut copy = dir.join(format!("{prefix}{stamp}"));
    let mut n = 2;
    while copy.exists() {
        copy = dir.join(format!("{prefix}{stamp}-{n}"));
        n += 1;
    }
    write_atomic(&copy, &bytes)?;
    Ok(Some(copy))
}

/// When loading `path` gave `warning` (the app went on without the file, or
/// without part of it), keeps a copy of the file and says where it is.
pub fn guard(path: &Path, warning: Option<String>) -> Option<String> {
    let warning = warning?;
    Some(match keep_unreadable(path) {
        Ok(Some(copy)) => format!(
            "{warning} — o conteúdo original foi guardado em {}",
            copy.display()
        ),
        Ok(None) => warning,
        Err(err) => format!("{warning} (não foi possível guardar uma cópia: {err})"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_writes_replace_the_whole_file_and_leave_no_temporary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("connections.json");
        write_atomic(&path, b"{\"a\": 1}").unwrap();
        write_atomic(&path, b"{}").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{}");
        let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, ["connections.json"]);
    }

    #[test]
    fn an_unreadable_file_is_kept_once_before_it_is_written_over() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        assert_eq!(keep_unreadable(&path).unwrap(), None);
        assert_eq!(guard(&path, None), None);

        std::fs::write(&path, "{ the user's servers, from a newer version").unwrap();
        let warning = guard(&path, Some("mcp.json inválido".into())).unwrap();
        let copy = keep_unreadable(&path).unwrap().unwrap();
        assert!(warning.contains(&copy.display().to_string()), "{warning}");
        assert_eq!(
            std::fs::read_to_string(&copy).unwrap(),
            "{ the user's servers, from a newer version"
        );
        // Every start warns again, but the same content is kept only once.
        guard(&path, Some("mcp.json inválido".into()));
        let kept = std::fs::read_dir(dir.path())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("mcp.json.unreadable-")
            })
            .count();
        assert_eq!(kept, 1);
        // The app's next save writes over the original; the copy stays.
        write_atomic(&path, b"{\"servers\": []}").unwrap();
        assert!(copy.exists());
    }
}

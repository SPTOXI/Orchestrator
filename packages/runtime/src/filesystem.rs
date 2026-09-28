//! Filesystem tools: `filesystem.list`, `.read`, `.write`, `.move`, `.delete`.
//!
//! Functions here are synchronous; the dispatcher runs them on the blocking
//! thread pool.

use crate::platform::io_error;
use base64::Engine as _;
use chrono::{DateTime, Utc};
use orchestrator_core::{ToolError, ToolErrorKind};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;

/// Content encoding for `read`/`write`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Encoding {
    #[default]
    Utf8,
    Base64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    File,
    Directory,
    /// Anything else (sockets, devices, broken symlinks…).
    Other,
}

// ----------------------------------------------------------------- list ---

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListArgs {
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirEntryInfo {
    pub name: String,
    pub path: String,
    /// Kind of the entry, following symlinks.
    pub kind: EntryKind,
    pub is_symlink: bool,
    pub size: u64,
    pub modified_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListOutput {
    pub path: String,
    /// Directories first, then files, each sorted case-insensitively.
    pub entries: Vec<DirEntryInfo>,
}

pub fn list(path: &Path) -> Result<ListOutput, ToolError> {
    let read_dir = fs::read_dir(path).map_err(|e| io_error("cannot list", path, e))?;
    let mut entries = Vec::new();
    for entry in read_dir {
        let entry = entry.map_err(|e| io_error("cannot list", path, e))?;
        let entry_path = entry.path();
        let is_symlink = entry.file_type().map(|t| t.is_symlink()).unwrap_or(false);
        let metadata = fs::metadata(&entry_path).ok();
        let kind = match &metadata {
            Some(m) if m.is_dir() => EntryKind::Directory,
            Some(m) if m.is_file() => EntryKind::File,
            _ => EntryKind::Other,
        };
        entries.push(DirEntryInfo {
            name: entry.file_name().to_string_lossy().into_owned(),
            path: entry_path.display().to_string(),
            kind,
            is_symlink,
            size: metadata.as_ref().map(|m| m.len()).unwrap_or(0),
            modified_at: metadata
                .and_then(|m| m.modified().ok())
                .map(DateTime::<Utc>::from),
        });
    }
    entries.sort_by(|a, b| {
        let a_dir = a.kind != EntryKind::Directory;
        let b_dir = b.kind != EntryKind::Directory;
        a_dir
            .cmp(&b_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(ListOutput {
        path: path.display().to_string(),
        entries,
    })
}

// ----------------------------------------------------------------- read ---

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadArgs {
    pub path: String,
    /// Requested encoding. `utf8` falls back to `base64` for binary content.
    #[serde(default)]
    pub encoding: Encoding,
    /// Read at most this many bytes (whole file when absent).
    #[serde(default)]
    pub max_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadOutput {
    pub path: String,
    pub content: String,
    /// Encoding actually used for `content`.
    pub encoding: Encoding,
    /// Size of the file on disk.
    pub size: u64,
    /// True when only the first `maxBytes` bytes were returned.
    pub truncated: bool,
}

pub fn read(
    path: &Path,
    encoding: Encoding,
    max_bytes: Option<u64>,
) -> Result<ReadOutput, ToolError> {
    let metadata = fs::metadata(path).map_err(|e| io_error("cannot read", path, e))?;
    if metadata.is_dir() {
        return Err(ToolError::invalid_args(format!(
            "{} is a directory; use filesystem.list",
            path.display()
        )));
    }
    let size = metadata.len();
    let mut file = fs::File::open(path).map_err(|e| io_error("cannot read", path, e))?;
    let mut bytes = Vec::new();
    match max_bytes {
        Some(max) => (&mut file).take(max).read_to_end(&mut bytes),
        None => file.read_to_end(&mut bytes),
    }
    .map_err(|e| io_error("cannot read", path, e))?;
    let truncated = (bytes.len() as u64) < size;

    let (content, encoding) = match encoding {
        Encoding::Base64 => (
            base64::engine::general_purpose::STANDARD.encode(&bytes),
            Encoding::Base64,
        ),
        Encoding::Utf8 => match utf8_text(&bytes, truncated) {
            Some(text) => (text, Encoding::Utf8),
            None => (
                base64::engine::general_purpose::STANDARD.encode(&bytes),
                Encoding::Base64,
            ),
        },
    };

    Ok(ReadOutput {
        path: path.display().to_string(),
        content,
        encoding,
        size,
        truncated,
    })
}

/// Returns the bytes as text if they are UTF-8. When the read was truncated,
/// an incomplete character at the very end is dropped instead of making the
/// whole file look binary.
fn utf8_text(bytes: &[u8], truncated: bool) -> Option<String> {
    match std::str::from_utf8(bytes) {
        Ok(text) => Some(text.to_owned()),
        Err(err) if truncated && err.error_len().is_none() => {
            Some(String::from_utf8_lossy(&bytes[..err.valid_up_to()]).into_owned())
        }
        Err(_) => None,
    }
}

// ---------------------------------------------------------------- write ---

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WriteArgs {
    pub path: String,
    pub content: String,
    #[serde(default)]
    pub encoding: Encoding,
    /// Create missing parent directories (default: true).
    #[serde(default = "default_true")]
    pub create_dirs: bool,
    /// Append instead of replacing the file (default: false).
    #[serde(default)]
    pub append: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteOutput {
    pub path: String,
    pub bytes_written: u64,
    /// True when the file did not exist before.
    pub created: bool,
}

pub fn write(path: &Path, args: &WriteArgs) -> Result<WriteOutput, ToolError> {
    let bytes = match args.encoding {
        Encoding::Utf8 => args.content.as_bytes().to_vec(),
        Encoding::Base64 => base64::engine::general_purpose::STANDARD
            .decode(args.content.as_bytes())
            .map_err(|e| ToolError::invalid_args(format!("invalid base64 content: {e}")))?,
    };
    if path.is_dir() {
        return Err(ToolError::invalid_args(format!(
            "{} is a directory",
            path.display()
        )));
    }
    if args.create_dirs {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)
                .map_err(|e| io_error("cannot create directory", parent, e))?;
        }
    }
    let created = !path.exists();
    let mut file = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(args.append)
        .truncate(!args.append)
        .open(path)
        .map_err(|e| io_error("cannot write", path, e))?;
    file.write_all(&bytes)
        .and_then(|_| file.flush())
        .map_err(|e| io_error("cannot write", path, e))?;
    Ok(WriteOutput {
        path: path.display().to_string(),
        bytes_written: bytes.len() as u64,
        created,
    })
}

// ----------------------------------------------------------------- move ---

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MoveArgs {
    pub from: String,
    pub to: String,
    /// Replace an existing destination (default: false).
    #[serde(default)]
    pub overwrite: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveOutput {
    pub from: String,
    pub to: String,
    /// True when an existing destination was replaced.
    pub replaced: bool,
}

pub fn move_path(from: &Path, to: &Path, overwrite: bool) -> Result<MoveOutput, ToolError> {
    fs::symlink_metadata(from).map_err(|e| io_error("cannot move", from, e))?;
    let replaced = fs::symlink_metadata(to).is_ok();
    if replaced {
        if !overwrite {
            return Err(ToolError::new(
                ToolErrorKind::AlreadyExists,
                format!(
                    "destination exists: {} (pass overwrite: true to replace it)",
                    to.display()
                ),
            ));
        }
        remove_any(to).map_err(|e| io_error("cannot replace", to, e))?;
    }
    if let Some(parent) = to.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|e| io_error("cannot create directory", parent, e))?;
    }
    match fs::rename(from, to) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::CrossesDevices => {
            copy_recursive(from, to).map_err(|e| io_error("cannot copy", from, e))?;
            remove_any(from).map_err(|e| io_error("cannot remove", from, e))?;
        }
        Err(err) => return Err(io_error("cannot move", from, err)),
    }
    Ok(MoveOutput {
        from: from.display().to_string(),
        to: to.display().to_string(),
        replaced,
    })
}

fn copy_recursive(from: &Path, to: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(from)?;
    if metadata.is_dir() {
        fs::create_dir_all(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(from, to).map(|_| ())
    }
}

fn remove_any(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        fs::remove_dir_all(path)
    } else {
        remove_file_or_link(path)
    }
}

fn remove_file_or_link(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        // Directory symlinks on Windows must be removed with remove_dir.
        Err(err) if cfg!(windows) && path.is_dir() => fs::remove_dir(path).or(Err(err)),
        other => other,
    }
}

// --------------------------------------------------------------- delete ---

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeleteArgs {
    pub path: String,
    /// Required to delete a non-empty directory (default: false).
    #[serde(default)]
    pub recursive: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteOutput {
    pub path: String,
    pub kind: EntryKind,
}

pub fn delete(path: &Path, recursive: bool) -> Result<DeleteOutput, ToolError> {
    let metadata = fs::symlink_metadata(path).map_err(|e| io_error("cannot delete", path, e))?;
    let kind = if metadata.file_type().is_symlink() {
        remove_file_or_link(path).map_err(|e| io_error("cannot delete", path, e))?;
        EntryKind::Other
    } else if metadata.is_dir() {
        let result = if recursive {
            fs::remove_dir_all(path)
        } else {
            fs::remove_dir(path)
        };
        result.map_err(|e| {
            if !recursive && e.kind() == io::ErrorKind::DirectoryNotEmpty {
                ToolError::invalid_args(format!(
                    "directory not empty: {} (pass recursive: true to delete it)",
                    path.display()
                ))
            } else {
                io_error("cannot delete", path, e)
            }
        })?;
        EntryKind::Directory
    } else {
        fs::remove_file(path).map_err(|e| io_error("cannot delete", path, e))?;
        EntryKind::File
    };
    Ok(DeleteOutput {
        path: path.display().to_string(),
        kind,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_args(path: &Path, content: &str) -> WriteArgs {
        WriteArgs {
            path: path.display().to_string(),
            content: content.into(),
            encoding: Encoding::Utf8,
            create_dirs: true,
            append: false,
        }
    }

    #[test]
    fn write_read_roundtrip_creates_parents() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a/b/c.txt");
        let out = write(&file, &write_args(&file, "olá mundo")).unwrap();
        assert!(out.created);
        assert_eq!(out.bytes_written, "olá mundo".len() as u64);

        let read_back = read(&file, Encoding::Utf8, None).unwrap();
        assert_eq!(read_back.content, "olá mundo");
        assert_eq!(read_back.encoding, Encoding::Utf8);
        assert!(!read_back.truncated);

        let again = write(&file, &write_args(&file, "x")).unwrap();
        assert!(!again.created);
    }

    #[test]
    fn append_mode_appends() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("log.txt");
        write(&file, &write_args(&file, "a")).unwrap();
        let mut args = write_args(&file, "b");
        args.append = true;
        write(&file, &args).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "ab");
    }

    #[test]
    fn binary_content_falls_back_to_base64() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("bin");
        fs::write(&file, [0xff, 0x00, 0xfe]).unwrap();
        let out = read(&file, Encoding::Utf8, None).unwrap();
        assert_eq!(out.encoding, Encoding::Base64);
        assert_eq!(out.content, "/wD+");
    }

    #[test]
    fn base64_write_decodes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("bin");
        let mut args = write_args(&file, "/wD+");
        args.encoding = Encoding::Base64;
        write(&file, &args).unwrap();
        assert_eq!(fs::read(&file).unwrap(), vec![0xff, 0x00, 0xfe]);
    }

    #[test]
    fn truncated_read_does_not_split_characters() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("t.txt");
        fs::write(&file, "aé").unwrap(); // 3 bytes
        let out = read(&file, Encoding::Utf8, Some(2)).unwrap();
        assert_eq!(out.encoding, Encoding::Utf8);
        assert_eq!(out.content, "a");
        assert!(out.truncated);
        assert_eq!(out.size, 3);
    }

    #[test]
    fn list_sorts_directories_first() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("b.txt"), "").unwrap();
        fs::write(dir.path().join("A.txt"), "").unwrap();
        fs::create_dir(dir.path().join("zdir")).unwrap();
        let out = list(dir.path()).unwrap();
        let names: Vec<_> = out.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["zdir", "A.txt", "b.txt"]);
        assert_eq!(out.entries[0].kind, EntryKind::Directory);
    }

    #[test]
    fn list_missing_directory_is_not_found() {
        let err = list(Path::new("/definitely/not/here/42")).unwrap_err();
        assert_eq!(err.kind, ToolErrorKind::NotFound);
    }

    #[test]
    fn move_refuses_to_overwrite_unless_asked() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        fs::write(&a, "A").unwrap();
        fs::write(&b, "B").unwrap();
        let err = move_path(&a, &b, false).unwrap_err();
        assert_eq!(err.kind, ToolErrorKind::AlreadyExists);

        let out = move_path(&a, &b, true).unwrap();
        assert!(out.replaced);
        assert!(!a.exists());
        assert_eq!(fs::read_to_string(&b).unwrap(), "A");
    }

    #[test]
    fn move_directory_into_new_parent() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("f"), "1").unwrap();
        let dest = dir.path().join("x/y/dest");
        move_path(&src, &dest, false).unwrap();
        assert_eq!(fs::read_to_string(dest.join("f")).unwrap(), "1");
    }

    #[test]
    fn delete_non_empty_directory_requires_recursive() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("f"), "1").unwrap();

        let err = delete(&sub, false).unwrap_err();
        assert_eq!(err.kind, ToolErrorKind::InvalidArgs);
        assert!(sub.exists());

        let out = delete(&sub, true).unwrap();
        assert_eq!(out.kind, EntryKind::Directory);
        assert!(!sub.exists());
    }

    #[test]
    fn delete_file() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("f");
        fs::write(&f, "1").unwrap();
        assert_eq!(delete(&f, false).unwrap().kind, EntryKind::File);
        assert!(!f.exists());
        assert_eq!(delete(&f, false).unwrap_err().kind, ToolErrorKind::NotFound);
    }
}

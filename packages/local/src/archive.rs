//! Unpacking the engine's release files (ADR-0025): `.zip` on Windows,
//! `.tar.gz` on macOS and Linux. Entries cannot leave the destination.

use std::fs::File;
use std::path::{Path, PathBuf};

pub fn extract(archive: &Path, dest: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest)
        .map_err(|e| format!("não foi possível criar {}: {e}", dest.display()))?;
    let name = archive
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if name.ends_with(".zip") {
        extract_zip(archive, dest)
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        extract_tar_gz(archive, dest)
    } else {
        Err(format!("formato de pacote desconhecido: {name}"))
    }
}

fn extract_zip(archive: &Path, dest: &Path) -> Result<(), String> {
    let file = File::open(archive).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("pacote zip inválido: {e}"))?;
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| format!("pacote zip inválido: {e}"))?;
        let Some(relative) = entry.enclosed_name() else {
            return Err(format!(
                "o pacote tem um caminho inseguro: {}",
                entry.name()
            ));
        };
        let target = dest.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut out = File::create(&target)
            .map_err(|e| format!("não foi possível gravar {}: {e}", target.display()))?;
        std::io::copy(&mut entry, &mut out).map_err(|e| format!("pacote zip corrompido: {e}"))?;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            let _ =
                std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode & 0o777));
        }
    }
    Ok(())
}

fn extract_tar_gz(archive: &Path, dest: &Path) -> Result<(), String> {
    let file = File::open(archive).map_err(|e| e.to_string())?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
    tar.set_preserve_permissions(true);
    for entry in tar
        .entries()
        .map_err(|e| format!("pacote tar inválido: {e}"))?
    {
        let mut entry = entry.map_err(|e| format!("pacote tar corrompido: {e}"))?;
        // `unpack_in` refuses entries that would land outside `dest`.
        let inside = entry
            .unpack_in(dest)
            .map_err(|e| format!("pacote tar corrompido: {e}"))?;
        if !inside {
            return Err("o pacote tem um caminho inseguro".into());
        }
    }
    Ok(())
}

/// The engine's executable under `dir`, at any depth.
pub fn find_program(dir: &Path, name: &str) -> Option<PathBuf> {
    let file = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    };
    walk(dir, &mut |path| {
        path.file_name().is_some_and(|n| n == file.as_str())
    })
    .into_iter()
    .min_by_key(|p| p.components().count())
}

/// Folders under `dir` that hold shared libraries (`.dll`, `.so`,
/// `.dylib`): the engine needs them on its library path.
pub fn library_dirs(dir: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = walk(dir, &mut |path| {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        name.ends_with(".dll") || name.ends_with(".dylib") || name.contains(".so")
    })
    .into_iter()
    .filter_map(|p| p.parent().map(Path::to_path_buf))
    .collect();
    dirs.sort();
    dirs.dedup();
    dirs
}

fn walk(dir: &Path, keep: &mut dyn FnMut(&Path) -> bool) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(t) if t.is_dir() => stack.push(path),
                Ok(_) if keep(&path) => found.push(path),
                _ => {}
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn unpacks_zip_and_tar_and_finds_the_server() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("e.zip");
        {
            let mut zip = zip::ZipWriter::new(File::create(&zip_path).unwrap());
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            zip.start_file("llama-server.exe", options).unwrap();
            zip.write_all(b"exe").unwrap();
            zip.start_file("ggml-cpu.dll", options).unwrap();
            zip.write_all(b"dll").unwrap();
            zip.finish().unwrap();
        }
        let out = dir.path().join("zip");
        extract(&zip_path, &out).unwrap();
        assert_eq!(std::fs::read(out.join("llama-server.exe")).unwrap(), b"exe");
        assert_eq!(library_dirs(&out), vec![out.clone()]);

        let tar_path = dir.path().join("e.tar.gz");
        {
            let gz = flate2::write::GzEncoder::new(
                File::create(&tar_path).unwrap(),
                flate2::Compression::fast(),
            );
            let mut tar = tar::Builder::new(gz);
            let mut header = tar::Header::new_gnu();
            header.set_size(3);
            header.set_mode(0o755);
            header.set_cksum();
            tar.append_data(&mut header, "llama-b1/llama-server", &b"bin"[..])
                .unwrap();
            let mut header = tar::Header::new_gnu();
            header.set_size(3);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, "llama-b1/libllama.so", &b"lib"[..])
                .unwrap();
            tar.into_inner().unwrap().finish().unwrap();
        }
        let out = dir.path().join("tar");
        extract(&tar_path, &out).unwrap();
        assert_eq!(
            std::fs::read(out.join("llama-b1/llama-server")).unwrap(),
            b"bin"
        );
        if !cfg!(windows) {
            assert_eq!(
                find_program(&out, "llama-server"),
                Some(out.join("llama-b1/llama-server"))
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(out.join("llama-b1/llama-server"))
                    .unwrap()
                    .permissions()
                    .mode();
                assert_eq!(mode & 0o111, 0o111);
            }
        }
        assert_eq!(library_dirs(&out), vec![out.join("llama-b1")]);
    }

    #[test]
    fn refuses_unknown_formats() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("e.rar");
        std::fs::write(&path, b"x").unwrap();
        assert!(extract(&path, &dir.path().join("o"))
            .unwrap_err()
            .contains("desconhecido"));
    }
}

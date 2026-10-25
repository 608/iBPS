use crate::patch::{self, Error, Format};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

pub fn read(path: &Path) -> Result<Vec<u8>, Error> {
    let result = (|| -> Result<Vec<u8>, Error> {
        let mut file = File::open(path).map_err(|e| e.to_string())?;
        let size = usize::try_from(file.metadata().map_err(|e| e.to_string())?.len())
            .map_err(|_| "ファイルが大きすぎます")?;
        let mut data = Vec::new();
        data.try_reserve_exact(size)
            .map_err(|_| "必要なメモリを確保できません")?;
        file.read_to_end(&mut data).map_err(|e| e.to_string())?;
        Ok(data)
    })();
    result.map_err(|e| format!("{}を読み込めません: {e}", path.display()))
}

pub fn default_apply_path(patch: &Path, input: &Path) -> PathBuf {
    patch.with_extension(input.extension().unwrap_or_default())
}

pub fn same_path(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

pub fn apply_files(
    patch_path: &Path,
    input_path: &Path,
    output: Option<&Path>,
) -> Result<Vec<String>, Error> {
    let default = default_apply_path(patch_path, input_path);
    let out = output.unwrap_or(&default);
    let no_replace = output.is_none() && !same_path(out, input_path);
    if no_replace && out.try_exists().map_err(|e| e.to_string())? {
        return Err(format!(
            "出力先{}はすでに存在します。上書きする場合は出力先を明示してください",
            out.display()
        ));
    }
    let applied = patch::apply(&read(patch_path)?, &read(input_path)?)?;
    write_atomic(out, &applied.data, no_replace)?;
    Ok(applied.warnings)
}

pub fn create_files(
    format: Format,
    source: &Path,
    target: &Path,
    output: &Path,
) -> Result<Vec<String>, Error> {
    if same_path(source, target) {
        return Err("変更前と変更後に同じパスは指定できません".into());
    }
    let created = patch::create(format, &read(source)?, &read(target)?)?;
    write_atomic(output, &created.data, false)?;
    Ok(created.warnings)
}

pub fn write_atomic(path: &Path, data: &[u8], no_replace: bool) -> Result<(), Error> {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let (temp, mut file) = loop {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let temp = parent.join(format!(".ibps-{}-{id}.tmp", std::process::id()));
        match OpenOptions::new().write(true).create_new(true).open(&temp) {
            Ok(file) => break (temp, file),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("一時ファイルを作成できません: {e}")),
        }
    };
    let result = (|| {
        file.write_all(data)?;
        file.sync_all()?;
        drop(file);
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            use windows_sys::Win32::Storage::FileSystem::{
                MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
            };
            let from: Vec<_> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
            let to: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            let flags = MOVEFILE_WRITE_THROUGH
                | if no_replace {
                    0
                } else {
                    MOVEFILE_REPLACE_EXISTING
                };
            if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), flags) } == 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
        #[cfg(not(windows))]
        if no_replace {
            // Never overwrite a file created concurrently at the default output path.
            fs::hard_link(&temp, path)?;
            fs::remove_file(&temp)?;
        } else {
            fs::rename(&temp, path)?;
        }
        Ok::<_, std::io::Error>(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.map_err(|e| format!("{}を書き込めません: {e}", path.display()))
}

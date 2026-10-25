use crate::patch::Format;
use std::{
    ffi::OsString,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::PathBuf,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS},
    System::Registry::{
        HKEY_CURRENT_USER, KEY_SET_VALUE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ,
        RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegCloseKey, RegCreateKeyExW, RegGetValueW,
        RegSetValueExW,
    },
};

const KEY: &str = "Software\\ibps\\Settings";

#[derive(Clone, Debug, PartialEq)]
pub struct GuiSettings {
    pub create: bool,
    pub format: Format,
    pub source: PathBuf,
    pub target: PathBuf,
    pub output: PathBuf,
    pub apply_patch: PathBuf,
    pub create_patch: PathBuf,
}

impl Default for GuiSettings {
    fn default() -> Self {
        Self {
            create: false,
            format: Format::Bps,
            source: PathBuf::new(),
            target: PathBuf::new(),
            output: PathBuf::new(),
            apply_patch: PathBuf::new(),
            create_patch: PathBuf::new(),
        }
    }
}

pub fn load() -> Result<GuiSettings, String> {
    load_from(KEY)
}
pub fn save(settings: &GuiSettings) -> Result<(), String> {
    save_to(KEY, settings)
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
fn error(code: u32) -> String {
    format!(
        "GUIの設定を読み書きできません: {}",
        std::io::Error::from_raw_os_error(code as i32)
    )
}

fn read_path(key: &[u16], name: &str) -> Result<PathBuf, String> {
    let mut value = vec![0u16; 32_768];
    let mut size = (value.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            wide(name).as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            value.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(PathBuf::new());
    }
    if status != ERROR_SUCCESS {
        return Err(error(status));
    }
    if !size.is_multiple_of(2) || size < 2 {
        return Err("保存されたファイルパスのフォーマットが不正です".into());
    }
    value.truncate(size as usize / 2);
    if value.pop() != Some(0) || value.contains(&0) {
        return Err("保存されたファイルパスのフォーマットが不正です".into());
    }
    Ok(OsString::from_wide(&value).into())
}

fn read_number(key: &[u16], name: &str) -> Result<u32, String> {
    let mut value = 0u32;
    let mut size = 4;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            wide(name).as_ptr(),
            RRF_RT_REG_DWORD,
            null_mut(),
            (&mut value as *mut u32).cast(),
            &mut size,
        )
    };
    match status {
        ERROR_FILE_NOT_FOUND => Ok(0),
        ERROR_SUCCESS => Ok(value),
        _ => Err(error(status)),
    }
}

fn load_from(key: &str) -> Result<GuiSettings, String> {
    let key = wide(key);
    Ok(GuiSettings {
        create: read_number(&key, "Create")? == 1,
        format: match read_number(&key, "Format")? {
            1 => Format::Ips,
            2 => Format::Ips32,
            3 => Format::Ups,
            _ => Format::Bps,
        },
        source: read_path(&key, "Source")?,
        target: read_path(&key, "Target")?,
        output: read_path(&key, "Output")?,
        apply_patch: read_path(&key, "ApplyPatch")?,
        create_patch: read_path(&key, "CreatePatch")?,
    })
}

fn save_to(path: &str, settings: &GuiSettings) -> Result<(), String> {
    let mut key = null_mut();
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            wide(path).as_ptr(),
            0,
            null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            null(),
            &mut key,
            null_mut(),
        )
    };
    if status != ERROR_SUCCESS {
        return Err(error(status));
    }
    let result = (|| {
        for (name, path) in [
            ("Source", &settings.source),
            ("Target", &settings.target),
            ("Output", &settings.output),
            ("ApplyPatch", &settings.apply_patch),
            ("CreatePatch", &settings.create_patch),
        ] {
            let value: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            if value.len() > 32_768 || value[..value.len() - 1].contains(&0) {
                return Err("ファイルパスが長すぎるか、使用できない文字を含んでいます".into());
            }
            let status = unsafe {
                RegSetValueExW(
                    key,
                    wide(name).as_ptr(),
                    0,
                    REG_SZ,
                    value.as_ptr().cast(),
                    (value.len() * 2) as u32,
                )
            };
            if status != ERROR_SUCCESS {
                return Err(error(status));
            }
        }
        let format = match settings.format {
            Format::Bps => 0,
            Format::Ips => 1,
            Format::Ips32 => 2,
            Format::Ups => 3,
        };
        for (name, value) in [("Create", u32::from(settings.create)), ("Format", format)] {
            let status = unsafe {
                RegSetValueExW(
                    key,
                    wide(name).as_ptr(),
                    0,
                    REG_DWORD,
                    value.to_le_bytes().as_ptr(),
                    4,
                )
            };
            if status != ERROR_SUCCESS {
                return Err(error(status));
            }
        }
        Ok(())
    })();
    unsafe { RegCloseKey(key) };
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use windows_sys::Win32::System::Registry::RegDeleteTreeW;
    struct TestKey(String);
    impl TestKey {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            Self(format!(
                "Software\\ibps\\Tests\\{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ))
        }
    }
    impl Drop for TestKey {
        fn drop(&mut self) {
            unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, wide(&self.0).as_ptr()) };
        }
    }
    #[test]
    fn missing_settings_use_empty_defaults() {
        assert_eq!(
            load_from(&TestKey::new().0).unwrap(),
            GuiSettings::default()
        );
    }
    #[test]
    fn native_paths_modes_and_formats_survive_reloading() {
        let key = TestKey::new();
        let mut settings = GuiSettings {
            create: true,
            format: Format::Ips32,
            source: "C:\\日本語 フォルダ\\変更元.gba".into(),
            target: "D:\\変更後.gba".into(),
            output: "D:\\適用結果.gba".into(),
            apply_patch: "D:\\適用.ips".into(),
            create_patch: PathBuf::from(OsString::from_wide(&[0x61, 0xd800, 0x62])),
        };
        for format in [Format::Bps, Format::Ips, Format::Ips32, Format::Ups] {
            settings.format = format;
            save_to(&key.0, &settings).unwrap();
            assert_eq!(load_from(&key.0).unwrap(), settings);
        }
        settings.create = false;
        settings.source = PathBuf::new();
        save_to(&key.0, &settings).unwrap();
        assert_eq!(load_from(&key.0).unwrap(), settings);
    }
}

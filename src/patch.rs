use crate::{bps, crc32::crc32, ips, ups};
use std::{borrow::Cow, fmt};

pub type Error = String;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Format {
    Ips,
    Ips32,
    Ups,
    Bps,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Ips | Self::Ips32 => "ips",
            Self::Ups => "ups",
            Self::Bps => "bps",
        }
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Ips => "IPS",
            Self::Ips32 => "IPS32",
            Self::Ups => "UPS",
            Self::Bps => "BPS",
        })
    }
}

#[derive(Debug)]
pub struct Applied {
    pub data: Vec<u8>,
    pub warnings: Vec<String>,
}
#[derive(Debug)]
pub struct Created {
    pub data: Vec<u8>,
    pub warnings: Vec<String>,
}
#[derive(Debug)]
pub enum Info {
    Ips {
        format: Format,
        records: usize,
        minimum_size: u64,
        truncate_size: Option<u64>,
    },
    Checksummed {
        format: Format,
        source_size: u64,
        target_size: u64,
        source_crc: u32,
        target_crc: u32,
    },
}

impl fmt::Display for Info {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ips {
                format,
                records,
                minimum_size,
                truncate_size,
            } => {
                writeln!(
                    f,
                    "フォーマット: {format}\nレコード数: {records}\n出力の最小サイズ: {minimum_size}バイト"
                )?;
                if let Some(size) = truncate_size {
                    writeln!(f, "トリム後のサイズ: {size}バイト")?;
                }
                Ok(())
            }
            Self::Checksummed {
                format,
                source_size,
                target_size,
                source_crc,
                target_crc,
            } => {
                writeln!(
                    f,
                    "フォーマット: {format}\n想定サイズ（変更前）: {source_size}バイト\n出力サイズ: {target_size}バイト\n変更前CRC32: {source_crc:08X}\n変更後CRC32: {target_crc:08X}"
                )
            }
        }
    }
}

pub fn identify(patch: &[u8]) -> Option<Format> {
    if patch.starts_with(b"PATCH") {
        Some(Format::Ips)
    } else if patch.starts_with(b"IPS32") {
        Some(Format::Ips32)
    } else if patch.starts_with(b"UPS1") {
        Some(Format::Ups)
    } else if patch.starts_with(b"BPS1") {
        Some(Format::Bps)
    } else {
        None
    }
}

pub fn apply(patch: &[u8], input: &[u8]) -> Result<Applied, Error> {
    match identify(patch).ok_or("対応していないパッチフォーマットです")? {
        Format::Ips => ips::apply(patch, input, false),
        Format::Ips32 => ips::apply(patch, input, true),
        Format::Ups => ups::apply(patch, input),
        Format::Bps => bps::apply(patch, input),
    }
}

pub fn create(format: Format, source: &[u8], target: &[u8]) -> Result<Created, Error> {
    let data = match format {
        Format::Ips => ips::create(source, target, false),
        Format::Ips32 => ips::create(source, target, true),
        Format::Ups => ups::create(source, target),
        Format::Bps => bps::create(source, target),
    }?;
    if apply(&data, source)?.data != target {
        return Err("内部エラー: 作成したパッチの検証結果が変更後ファイルと一致しません".into());
    }
    let mut warnings = Vec::new();
    if source == target {
        warnings.push("変更前ファイルと変更後ファイルの内容が同一です".into());
    }
    if source.len() > target.len() {
        warnings.push("変更後ファイルの方が小さいため、入力の指定順を確認してください".into());
    }
    Ok(Created { data, warnings })
}

pub fn info(patch: &[u8]) -> Result<Info, Error> {
    match identify(patch).ok_or("対応していないパッチフォーマットです")? {
        Format::Ips => ips::info(patch, false),
        Format::Ips32 => ips::info(patch, true),
        Format::Ups => ups::info(patch),
        Format::Bps => bps::info(patch),
    }
}

pub fn buffer(size: usize, fill: u8) -> Result<Vec<u8>, Error> {
    let mut data = Vec::new();
    data.try_reserve_exact(size)
        .map_err(|_| "必要なメモリを確保できません")?;
    data.resize(size, fill);
    Ok(data)
}

pub fn footer(patch: &[u8], magic: &[u8]) -> Result<(usize, u32, u32), Error> {
    if !patch.starts_with(magic) || patch.len() < magic.len() + 14 {
        return Err("パッチのヘッダまたは長さが不正です".into());
    }
    let end = patch.len() - 12;
    let read = |at| u32::from_le_bytes(patch[at..at + 4].try_into().unwrap());
    if crc32(&patch[..patch.len() - 4]) != read(patch.len() - 4) {
        return Err("パッチのCRC32が一致しません。パッチが破損しています".into());
    }
    Ok((end, read(end), read(end + 4)))
}

pub fn append_checksums(out: &mut Vec<u8>, source: &[u8], target: &[u8]) {
    out.extend_from_slice(&crc32(source).to_le_bytes());
    out.extend_from_slice(&crc32(target).to_le_bytes());
    out.extend_from_slice(&crc32(out).to_le_bytes());
}

pub fn checked_input(
    input: &[u8],
    source_size: u64,
    source_crc: u32,
    target_size: u64,
    target_crc: u32,
    reverse: bool,
) -> Result<(Cow<'_, [u8]>, bool), Error> {
    let actual_crc = crc32(input);
    if input.len() as u64 == source_size && actual_crc == source_crc {
        return Ok((Cow::Borrowed(input), false));
    }
    if input.len() as u64 == target_size && actual_crc == target_crc {
        if reverse {
            return Ok((Cow::Borrowed(input), true));
        }
        return Err("指定されたファイルはすでにパッチ適用済みです".into());
    }
    if (input.len() as u64) < source_size {
        let size =
            usize::try_from(source_size).map_err(|_| "想定サイズが扱える範囲を超えています")?;
        let mut padded = buffer(size, 0xff)?;
        padded[..input.len()].copy_from_slice(input);
        if crc32(&padded) == source_crc {
            return Ok((Cow::Owned(padded), false));
        }
        padded[input.len()..].fill(0);
        if crc32(&padded) == source_crc {
            return Ok((Cow::Owned(padded), false));
        }
    }
    Err(format!(
        "対象ファイルが一致しません。想定サイズ: {source_size}バイト、入力サイズ: {}バイト、変更前CRC32: {source_crc:08X}、入力CRC32: {actual_crc:08X}",
        input.len()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn input_selection_prioritizes_exact_matches() {
        let source = [1, 2, 0xff, 0xff];
        let target = [1, 2];
        assert!(
            checked_input(&target, 4, crc32(&source), 2, crc32(&target), false)
                .unwrap_err()
                .contains("適用済み")
        );
        assert!(
            checked_input(&target, 4, crc32(&source), 2, crc32(&target), true)
                .unwrap()
                .1
        );
        assert!(
            !checked_input(&source, 4, crc32(&source), 4, crc32(&source), true)
                .unwrap()
                .1
        );
        for fill in [0, 255] {
            let source = [1, 2, fill, fill];
            let (data, reversed) = checked_input(&target, 4, crc32(&source), 5, 0, false).unwrap();
            assert_eq!(&*data, source);
            assert!(!reversed);
        }
    }
    #[test]
    fn impossible_allocation_is_an_error() {
        assert!(buffer(usize::MAX, 0).is_err());
    }
}

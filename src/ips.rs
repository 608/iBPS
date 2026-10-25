use crate::patch::{Applied, Error, Format, Info, buffer};

const IPS_HEADER: &[u8] = b"PATCH";
const IPS32_HEADER: &[u8] = b"IPS32";
const IPS_END: u64 = 0x45_4f_46;
const IPS32_END: u64 = 0x45_45_4f_46;
const MAX_RECORD_LEN: usize = u16::MAX as usize;

struct Spec {
    header: &'static [u8],
    offset_width: usize,
    end: u64,
    format: Format,
}

fn spec(wide: bool) -> Spec {
    if wide {
        Spec {
            header: IPS32_HEADER,
            offset_width: 4,
            end: IPS32_END,
            format: Format::Ips32,
        }
    } else {
        Spec {
            header: IPS_HEADER,
            offset_width: 3,
            end: IPS_END,
            format: Format::Ips,
        }
    }
}

enum ParsedData<'a> {
    Literal(&'a [u8]),
    Rle { len: usize, value: u8 },
}

struct ParsedRecord<'a> {
    offset: usize,
    data: ParsedData<'a>,
}

impl ParsedRecord<'_> {
    fn len(&self) -> usize {
        match self.data {
            ParsedData::Literal(data) => data.len(),
            ParsedData::Rle { len, .. } => len,
        }
    }
}

struct ParsedPatch<'a> {
    records: Vec<ParsedRecord<'a>>,
    minimum_size: usize,
    truncate_size: Option<usize>,
}

fn read_be(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .fold(0, |value, &byte| (value << 8) | u64::from(byte))
}

fn parse(patch: &[u8], wide: bool) -> Result<ParsedPatch<'_>, Error> {
    let spec = spec(wide);
    if !patch.starts_with(spec.header) {
        return Err(format!("{}ヘッダがありません。", spec.format));
    }

    let mut cursor = spec.header.len();
    let mut records = Vec::new();
    records
        .try_reserve(patch.len() / (spec.offset_width + 3))
        .map_err(|_| "IPSレコード一覧のメモリを確保できません。".to_string())?;
    let mut minimum_size = 0usize;

    loop {
        let offset_end = cursor
            .checked_add(spec.offset_width)
            .ok_or_else(|| "IPSパッチ内の位置がオーバーフローしました。".to_string())?;
        let offset_bytes = patch
            .get(cursor..offset_end)
            .ok_or_else(|| "IPSパッチに終端記号がありません。".to_string())?;
        cursor = offset_end;
        let raw_offset = read_be(offset_bytes);
        if raw_offset == spec.end {
            break;
        }
        let offset = usize::try_from(raw_offset)
            .map_err(|_| "IPSレコードの書き込み位置を扱えません。".to_string())?;

        let size_end = cursor
            .checked_add(2)
            .ok_or_else(|| "IPSパッチ内の位置がオーバーフローしました。".to_string())?;
        let size = patch
            .get(cursor..size_end)
            .map(read_be)
            .ok_or_else(|| "IPSレコードのサイズが途中で切れています。".to_string())?
            as usize;
        cursor = size_end;

        let data = if size != 0 {
            let data_end = cursor
                .checked_add(size)
                .ok_or_else(|| "IPSレコードのデータ位置がオーバーフローしました。".to_string())?;
            let data = patch
                .get(cursor..data_end)
                .ok_or_else(|| "IPSレコードのデータが途中で切れています。".to_string())?;
            cursor = data_end;
            ParsedData::Literal(data)
        } else {
            let rle_end = cursor
                .checked_add(3)
                .ok_or_else(|| "IPS RLEレコードの位置がオーバーフローしました。".to_string())?;
            let rle = patch
                .get(cursor..rle_end)
                .ok_or_else(|| "IPS RLEレコードが途中で切れています。".to_string())?;
            cursor = rle_end;
            let len = read_be(&rle[..2]) as usize;
            if len == 0 {
                return Err("IPS RLEレコードの長さが0です。".to_string());
            }
            ParsedData::Rle { len, value: rle[2] }
        };

        let record = ParsedRecord { offset, data };
        let record_end = record
            .offset
            .checked_add(record.len())
            .ok_or_else(|| "IPSレコードの書き込み終端がオーバーフローしました。".to_string())?;
        minimum_size = minimum_size.max(record_end);
        records.push(record);
    }

    let remaining = patch.len() - cursor;
    let truncate_size = if wide {
        if remaining != 0 {
            return Err("IPS32の終端記号の後に余分なデータがあります。".to_string());
        }
        None
    } else {
        match remaining {
            0 => None,
            3 => Some(read_be(&patch[cursor..]) as usize),
            _ => return Err("IPSの終端記号の後に不正なデータがあります。".to_string()),
        }
    };

    Ok(ParsedPatch {
        records,
        minimum_size,
        truncate_size,
    })
}

pub fn apply(patch: &[u8], input: &[u8], wide: bool) -> Result<Applied, Error> {
    let parsed = parse(patch, wide)?;
    let mut data = buffer(input.len().max(parsed.minimum_size), 0xff)?;
    data[..input.len()].copy_from_slice(input);

    for record in parsed.records {
        let end = record.offset + record.len();
        match record.data {
            ParsedData::Literal(bytes) => data[record.offset..end].copy_from_slice(bytes),
            ParsedData::Rle { value, .. } => data[record.offset..end].fill(value),
        }
    }

    let mut warnings = Vec::new();
    if let Some(size) = parsed.truncate_size {
        if input.len() <= size {
            warnings.push(
                "トリム後のサイズが変更前ファイルのサイズ以上です。対象ファイルが一致していない可能性があります。"
                    .to_string(),
            );
        }
        if size < data.len() {
            data.truncate(size);
        }
    }
    if data == input {
        warnings.push(
            "パッチ適用後も内容が変化しませんでした。すでに適用済みの可能性があります。"
                .to_string(),
        );
    }

    Ok(Applied { data, warnings })
}

#[derive(Clone, Copy)]
enum CreatedRecord {
    Literal {
        offset: usize,
        len: usize,
    },
    Rle {
        offset: usize,
        len: usize,
        value: u8,
    },
}

impl CreatedRecord {
    fn offset(self) -> usize {
        match self {
            Self::Literal { offset, .. } | Self::Rle { offset, .. } => offset,
        }
    }
}

fn push_record(records: &mut Vec<CreatedRecord>, record: CreatedRecord) -> Result<(), Error> {
    records
        .try_reserve(1)
        .map_err(|_| "IPSレコード一覧のメモリを確保できません。".to_string())?;
    records.push(record);
    Ok(())
}

fn push_literal(
    records: &mut Vec<CreatedRecord>,
    mut offset: usize,
    mut len: usize,
) -> Result<(), Error> {
    while len != 0 {
        let chunk = len.min(MAX_RECORD_LEN);
        push_record(records, CreatedRecord::Literal { offset, len: chunk })?;
        offset += chunk;
        len -= chunk;
    }
    Ok(())
}

fn push_rle(
    records: &mut Vec<CreatedRecord>,
    mut offset: usize,
    mut len: usize,
    value: u8,
) -> Result<(), Error> {
    while len != 0 {
        let chunk = len.min(MAX_RECORD_LEN);
        push_record(
            records,
            CreatedRecord::Rle {
                offset,
                len: chunk,
                value,
            },
        )?;
        offset += chunk;
        len -= chunk;
    }
    Ok(())
}

fn encode_block(
    records: &mut Vec<CreatedRecord>,
    target: &[u8],
    start: usize,
    end: usize,
) -> Result<(), Error> {
    let block = &target[start..end];
    if block.len() >= 4 && block.iter().all(|&byte| byte == block[0]) {
        return push_rle(records, start, block.len(), block[0]);
    }

    let mut run_start = start;
    let mut literal_start = start;
    while run_start < end {
        let value = target[run_start];
        let mut run_end = run_start + 1;
        while run_end < end && target[run_end] == value {
            run_end += 1;
        }
        let threshold = if run_end == end { 9 } else { 14 };
        if run_end - run_start >= threshold {
            push_literal(records, literal_start, run_start - literal_start)?;
            push_rle(records, run_start, run_end - run_start, value)?;
            literal_start = run_end;
        }
        run_start = run_end;
    }
    push_literal(records, literal_start, end - literal_start)
}

fn avoid_end_marker(
    records: Vec<CreatedRecord>,
    marker: usize,
) -> Result<Vec<CreatedRecord>, Error> {
    let mut fixed = Vec::new();
    fixed
        .try_reserve(records.len())
        .map_err(|_| "IPSレコード一覧のメモリを確保できません。".to_string())?;

    for record in records {
        if record.offset() != marker {
            push_record(&mut fixed, record)?;
            continue;
        }

        match record {
            CreatedRecord::Literal { offset, len } => {
                if len == MAX_RECORD_LEN {
                    push_record(
                        &mut fixed,
                        CreatedRecord::Literal {
                            offset: offset - 1,
                            len: MAX_RECORD_LEN,
                        },
                    )?;
                    push_record(
                        &mut fixed,
                        CreatedRecord::Literal {
                            offset: offset + MAX_RECORD_LEN - 1,
                            len: 1,
                        },
                    )?;
                } else {
                    push_record(
                        &mut fixed,
                        CreatedRecord::Literal {
                            offset: offset - 1,
                            len: len + 1,
                        },
                    )?;
                }
            }
            CreatedRecord::Rle { offset, len, value } => {
                push_record(
                    &mut fixed,
                    CreatedRecord::Literal {
                        offset: offset - 1,
                        len: 2,
                    },
                )?;
                let remaining = len - 1;
                if remaining >= 4 {
                    push_record(
                        &mut fixed,
                        CreatedRecord::Rle {
                            offset: offset + 1,
                            len: remaining,
                            value,
                        },
                    )?;
                } else if remaining != 0 {
                    push_record(
                        &mut fixed,
                        CreatedRecord::Literal {
                            offset: offset + 1,
                            len: remaining,
                        },
                    )?;
                }
            }
        }
    }
    Ok(fixed)
}

fn append_offset(output: &mut Vec<u8>, offset: usize, width: usize) {
    let bytes = (offset as u64).to_be_bytes();
    output.extend_from_slice(&bytes[bytes.len() - width..]);
}

fn validate_sizes(source_len: u64, target_len: u64, wide: bool) -> Result<(), Error> {
    if wide {
        if target_len < source_len {
            return Err("IPS32は変更後ファイルを小さくできません。".to_string());
        }
        if target_len >= 0x1_0000_0000 {
            return Err("IPS32は4GBを超えるデータを扱うことができません。".to_string());
        }
    } else {
        if target_len < source_len && target_len >= 0x100_0000 {
            return Err("トリム後のサイズに16MB以上を指定できません。".to_string());
        }
        if target_len > source_len && target_len > 0x100_0000 {
            return Err("IPSは16MBを超えるデータを扱うことができません。".to_string());
        }
    }
    Ok(())
}

pub fn create(source: &[u8], target: &[u8], wide: bool) -> Result<Vec<u8>, Error> {
    validate_sizes(source.len() as u64, target.len() as u64, wide)?;

    let mut blocks = Vec::new();
    let mut position = 0usize;
    while position < target.len() {
        while position < target.len() && source.get(position) == Some(&target[position]) {
            position += 1;
        }
        if position == target.len() {
            break;
        }

        let start = position;
        let mut last_change = position;
        position += 1;
        while position < target.len() {
            if source.get(position) != Some(&target[position]) {
                last_change = position;
            } else if position - last_change > 5 {
                break;
            }
            position += 1;
        }
        blocks
            .try_reserve(1)
            .map_err(|_| "IPS変更ブロック一覧のメモリを確保できません。".to_string())?;
        blocks.push((start, last_change + 1));
    }

    let mut records = Vec::new();
    for (start, end) in blocks {
        encode_block(&mut records, target, start, end)?;
    }

    let format = spec(wide);
    records = avoid_end_marker(records, format.end as usize)?;
    if !wide && records.iter().any(|record| record.offset() > 0xff_ffff) {
        return Err("IPSの書き込み位置が16MBの範囲を超えています。".to_string());
    }

    let mut output_size = format.header.len() + format.offset_width;
    if !wide && target.len() < source.len() {
        output_size += 3;
    }
    for record in &records {
        let record_size = format.offset_width
            + 2
            + match record {
                CreatedRecord::Literal { len, .. } => *len,
                CreatedRecord::Rle { .. } => 3,
            };
        output_size = output_size
            .checked_add(record_size)
            .ok_or_else(|| "IPSパッチのサイズがオーバーフローしました。".to_string())?;
    }

    let mut patch = Vec::new();
    patch
        .try_reserve(output_size)
        .map_err(|_| "IPSパッチ出力用のメモリを確保できません。".to_string())?;
    patch.extend_from_slice(format.header);
    for record in records {
        append_offset(&mut patch, record.offset(), format.offset_width);
        match record {
            CreatedRecord::Literal { offset, len } => {
                patch.extend_from_slice(&(len as u16).to_be_bytes());
                patch.extend_from_slice(&target[offset..offset + len]);
            }
            CreatedRecord::Rle { len, value, .. } => {
                patch.extend_from_slice(&0u16.to_be_bytes());
                patch.extend_from_slice(&(len as u16).to_be_bytes());
                patch.push(value);
            }
        }
    }
    append_offset(&mut patch, format.end as usize, format.offset_width);
    if !wide && target.len() < source.len() {
        append_offset(&mut patch, target.len(), 3);
    }

    Ok(patch)
}

pub fn info(patch: &[u8], wide: bool) -> Result<Info, Error> {
    let format = spec(wide);
    let parsed = parse(patch, wide)?;
    Ok(Info::Ips {
        format: format.format,
        records: parsed.records.len(),
        minimum_size: parsed.minimum_size as u64,
        truncate_size: parsed.truncate_size.map(|size| size as u64),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ips_round_trip_with_growth_rle_and_truncation() {
        let source = b"0123456789abcdefghijklmnopqrstuvwxyz";
        let mut grown = source.to_vec();
        grown[2..6].copy_from_slice(b"WXYZ");
        grown.extend_from_slice(&[0x55; 20]);
        let patch = create(source, &grown, false).unwrap();
        assert_eq!(apply(&patch, source, false).unwrap().data, grown);

        let shrunk = b"0123456789";
        let patch = create(source, shrunk, false).unwrap();
        assert_eq!(apply(&patch, source, false).unwrap().data, shrunk);
        match info(&patch, false).unwrap() {
            Info::Ips {
                format,
                records,
                minimum_size,
                truncate_size,
            } => {
                assert_eq!(format, Format::Ips);
                assert_eq!(records, 0);
                assert_eq!(minimum_size, 0);
                assert_eq!(truncate_size, Some(shrunk.len() as u64));
            }
            _ => panic!("IPS情報ではありません"),
        }
    }

    #[test]
    fn ips32_round_trip_and_rejects_shrinking() {
        let source = b"abcdef";
        let target = b"abZZZZZZZZZZZZZZZZcdef-more";
        let patch = create(source, target, true).unwrap();
        assert_eq!(apply(&patch, source, true).unwrap().data, target);
        assert!(create(source, b"abc", true).is_err());
    }

    #[test]
    fn merges_short_gaps_and_splits_long_records() {
        let source = vec![0u8; 70_010];
        let mut target = source.clone();
        target[0] = 1;
        target[6] = 2;
        target[7..70_010].fill(3);
        let patch = create(&source, &target, false).unwrap();
        let parsed = parse(&patch, false).unwrap();
        assert!(parsed.records.len() >= 3);
        assert_eq!(apply(&patch, &source, false).unwrap().data, target);
    }

    #[test]
    fn avoids_ips_end_marker_for_literal_and_rle_records() {
        let marker = IPS_END as usize;
        let mut source = vec![0u8; marker + 32];
        let mut target = source.clone();
        target[marker] = 1;
        let patch = create(&source, &target, false).unwrap();
        assert_eq!(apply(&patch, &source, false).unwrap().data, target);

        source[marker..marker + 16].fill(2);
        target = source.clone();
        target[marker..marker + 16].fill(7);
        let patch = create(&source, &target, false).unwrap();
        assert_eq!(apply(&patch, &source, false).unwrap().data, target);
    }

    #[test]
    fn end_marker_fix_handles_max_length_and_ips32_without_large_files() {
        let marker = IPS_END as usize;
        let fixed = avoid_end_marker(
            vec![CreatedRecord::Literal {
                offset: marker,
                len: MAX_RECORD_LEN,
            }],
            marker,
        )
        .unwrap();
        assert!(matches!(
            fixed.as_slice(),
            [
                CreatedRecord::Literal { offset, len },
                CreatedRecord::Literal {
                    offset: tail_offset,
                    len: 1
                }
            ] if *offset == marker - 1
                && *len == MAX_RECORD_LEN
                && *tail_offset == marker + MAX_RECORD_LEN - 1
        ));

        let marker = IPS32_END as usize;
        let fixed = avoid_end_marker(
            vec![CreatedRecord::Rle {
                offset: marker,
                len: 10,
                value: 0xa5,
            }],
            marker,
        )
        .unwrap();
        assert!(matches!(
            fixed.as_slice(),
            [
                CreatedRecord::Literal { offset, len: 2 },
                CreatedRecord::Rle {
                    offset: rle_offset,
                    len: 9,
                    value: 0xa5
                }
            ] if *offset == marker - 1 && *rle_offset == marker + 1
        ));
    }

    #[test]
    fn validates_ips_and_ips32_size_boundaries() {
        assert!(validate_sizes(0, 0x100_0000, false).is_ok());
        assert!(validate_sizes(0, 0x100_0001, false).is_err());
        assert!(validate_sizes(0x100_0001, 0x100_0000, false).is_err());
        assert!(validate_sizes(0x100_0000, 0xff_ffff, false).is_ok());

        assert!(validate_sizes(0, 0xffff_ffff, true).is_ok());
        assert!(validate_sizes(0, 0x1_0000_0000, true).is_err());
        assert!(validate_sizes(2, 1, true).is_err());
    }

    #[test]
    fn rejects_malformed_patches_and_warns_when_unchanged() {
        assert!(apply(b"PATCH", b"", false).is_err());
        assert!(apply(b"PATCHEOFx", b"", false).is_err());
        assert!(apply(b"IPS32EEOFx", b"", true).is_err());
        assert!(apply(b"PATCH\0\0\0\0\0\0\0\0", b"", false).is_err());

        let applied = apply(b"PATCHEOF", b"same", false).unwrap();
        assert_eq!(applied.data, b"same");
        assert_eq!(applied.warnings.len(), 1);
    }
}

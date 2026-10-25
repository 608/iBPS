use crate::crc32::crc32;
use crate::patch::{Applied, Error, Format, Info, append_checksums, buffer, checked_input, footer};
use crate::varint::{decode, encode};

const MAGIC: &[u8; 4] = b"UPS1";

fn header(patch: &[u8], body_end: usize) -> Result<(u64, u64, usize), Error> {
    let body = patch
        .get(..body_end)
        .ok_or_else(|| "UPSパッチの構造が不正です".to_string())?;
    let mut pos = MAGIC.len();
    let source_size = decode(body, &mut pos)?;
    let target_size = decode(body, &mut pos)?;
    Ok((source_size, target_size, pos))
}

pub fn apply(patch: &[u8], input: &[u8]) -> Result<Applied, Error> {
    let (body_end, source_crc, target_crc) = footer(patch, MAGIC)?;
    let (source_size, target_size, mut patch_pos) = header(patch, body_end)?;
    let (input, reverse) = checked_input(
        input,
        source_size,
        source_crc,
        target_size,
        target_crc,
        true,
    )?;

    let (output_size, expected_crc) = if reverse {
        (source_size, source_crc)
    } else {
        (target_size, target_crc)
    };
    let output_len = usize::try_from(output_size)
        .map_err(|_| format!("出力サイズが大きすぎます: {output_size}バイト"))?;
    let mut output = buffer(output_len, 0)?;
    let copied = input.len().min(output.len());
    output[..copied].copy_from_slice(&input[..copied]);

    // Keep the UPS file position separate from memory indices because it may advance beyond the output.
    let mut logical_pos = 0u64;
    while patch_pos < body_end {
        let skip = decode(&patch[..body_end], &mut patch_pos)?;
        logical_pos = logical_pos
            .checked_add(skip)
            .ok_or_else(|| "UPSパッチのファイル位置が範囲を超えています".to_string())?;

        loop {
            let xor = *patch
                .get(patch_pos)
                .filter(|_| patch_pos < body_end)
                .ok_or_else(|| "UPSパッチのXORブロックが終端されていません".to_string())?;
            patch_pos += 1;

            if logical_pos < output_size {
                let index = usize::try_from(logical_pos)
                    .map_err(|_| "UPSパッチのファイル位置が大きすぎます".to_string())?;
                let source = input.get(index).copied().unwrap_or(0);
                output[index] = source ^ xor;
            }

            logical_pos = logical_pos
                .checked_add(1)
                .ok_or_else(|| "UPSパッチのファイル位置が範囲を超えています".to_string())?;
            if xor == 0 {
                break;
            }
        }
    }

    let actual_crc = crc32(&output);
    if actual_crc != expected_crc {
        return Err(format!(
            "適用結果のCRC32が一致しません（期待値: {expected_crc:08X}、実際: {actual_crc:08X}）"
        ));
    }

    Ok(Applied {
        data: output,
        warnings: Vec::new(),
    })
}

pub fn create(source: &[u8], target: &[u8]) -> Result<Vec<u8>, Error> {
    let source_size = u64::try_from(source.len())
        .map_err(|_| "変更前ファイルのサイズが大きすぎます".to_string())?;
    let target_size = u64::try_from(target.len())
        .map_err(|_| "変更後ファイルのサイズが大きすぎます".to_string())?;
    let end = source.len().max(target.len());
    let capacity = end
        .checked_mul(2)
        .and_then(|size| size.checked_add(36))
        .ok_or_else(|| "UPSパッチのサイズが大きすぎます".to_string())?;
    let mut out = Vec::new();
    out.try_reserve(capacity)
        .map_err(|_| "UPSパッチ用のメモリを確保できません".to_string())?;
    out.extend_from_slice(MAGIC);
    encode(source_size, &mut out);
    encode(target_size, &mut out);

    let mut relative = 0usize;
    let mut n = 0usize;
    while n < end {
        let source_byte = source.get(n).copied().unwrap_or(0);
        let target_byte = target.get(n).copied().unwrap_or(0);
        if source_byte == target_byte {
            n += 1;
            continue;
        }

        encode(
            u64::try_from(n - relative)
                .map_err(|_| "UPSパッチの相対位置が大きすぎます".to_string())?,
            &mut out,
        );
        loop {
            let value = source.get(n).copied().unwrap_or(0) ^ target.get(n).copied().unwrap_or(0);
            out.push(value);
            n += 1;
            if value == 0 {
                break;
            }
            if n >= end {
                out.push(0);
                n = n
                    .checked_add(1)
                    .ok_or_else(|| "UPSパッチのファイル位置が範囲を超えています".to_string())?;
                break;
            }
        }
        relative = n;
    }

    append_checksums(&mut out, source, target);
    Ok(out)
}

pub fn info(patch: &[u8]) -> Result<Info, Error> {
    let (body_end, source_crc, target_crc) = footer(patch, MAGIC)?;
    let (source_size, target_size, _) = header(patch, body_end)?;
    Ok(Info::Checksummed {
        format: Format::Ups,
        source_size,
        target_size,
        source_crc,
        target_crc,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_equal_growing_and_shrinking_files() {
        for (source, target) in [
            (&b"abcdef"[..], &b"abXdef"[..]),
            (&b"short"[..], &b"short and longer"[..]),
            (&b"longer source"[..], &b"short"[..]),
        ] {
            let patch = create(source, target).unwrap();
            assert_eq!(apply(&patch, source).unwrap().data, target);
            assert_eq!(apply(&patch, target).unwrap().data, source);
        }
    }

    #[test]
    fn accepts_xor_blocks_beyond_the_physical_output() {
        let mut patch = b"UPS1".to_vec();
        encode(0, &mut patch);
        encode(0, &mut patch);
        encode(3, &mut patch);
        patch.extend_from_slice(&[0x5a, 0]);
        append_checksums(&mut patch, b"", b"");

        assert_eq!(apply(&patch, b"").unwrap().data, b"");
    }

    #[test]
    fn accepts_ff_and_zero_padded_sources() {
        for fill in [0xff, 0] {
            let source = [1, 2, fill, fill];
            let target = [1, 9, fill, fill];
            let patch = create(&source, &target).unwrap();
            assert_eq!(apply(&patch, &source[..2]).unwrap().data, target);
        }
    }

    #[test]
    fn rejects_an_unterminated_xor_block() {
        let mut patch = b"UPS1".to_vec();
        encode(0, &mut patch);
        encode(0, &mut patch);
        encode(0, &mut patch);
        patch.push(1);
        append_checksums(&mut patch, b"", b"");

        assert!(apply(&patch, b"").unwrap_err().contains("終端"));
    }

    #[test]
    fn rejects_a_wrong_target_crc() {
        let mut patch = create(b"source", b"target").unwrap();
        let len = patch.len();
        patch[len - 8] ^= 1;
        let patch_crc = crc32(&patch[..len - 4]);
        patch[len - 4..].copy_from_slice(&patch_crc.to_le_bytes());

        assert!(
            apply(&patch, b"source")
                .unwrap_err()
                .contains("適用結果のCRC32")
        );
    }

    #[test]
    fn rejects_a_logical_position_overflow() {
        let mut patch = b"UPS1".to_vec();
        encode(0, &mut patch);
        encode(0, &mut patch);
        encode(u64::MAX - 1, &mut patch);
        patch.push(0);
        encode(1, &mut patch);
        patch.push(0);
        append_checksums(&mut patch, b"", b"");

        assert!(apply(&patch, b"").unwrap_err().contains("範囲"));
    }

    #[test]
    fn rejects_a_truncated_header() {
        let mut patch = b"UPS1\0\0".to_vec();
        append_checksums(&mut patch, b"", b"");

        assert!(apply(&patch, b"").unwrap_err().contains("可変長整数"));
    }

    #[test]
    fn reports_header_information() {
        let source = b"source";
        let target = b"target data";
        let patch = create(source, target).unwrap();

        match info(&patch).unwrap() {
            Info::Checksummed {
                format: Format::Ups,
                source_size,
                target_size,
                source_crc,
                target_crc,
            } => {
                assert_eq!(source_size, source.len() as u64);
                assert_eq!(target_size, target.len() as u64);
                assert_eq!(source_crc, crc32(source));
                assert_eq!(target_crc, crc32(target));
            }
            _ => panic!("UPS情報ではありません"),
        }
    }
}

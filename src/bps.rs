use crate::crc32::crc32;
use crate::patch::{Applied, Error, Format, Info, append_checksums, buffer, checked_input, footer};
use crate::varint::{decode, encode};

const MAGIC: &[u8; 4] = b"BPS1";

struct Header {
    source_size: usize,
    target_size: usize,
    commands: usize,
}

fn header(patch: &[u8], commands_end: usize) -> Result<Header, Error> {
    let commands = &patch[..commands_end];
    let mut pos = MAGIC.len();
    let source_size = usize::try_from(decode(commands, &mut pos)?)
        .map_err(|_| "BPSの変更前サイズがこの環境で扱える範囲を超えています".to_owned())?;
    let target_size = usize::try_from(decode(commands, &mut pos)?)
        .map_err(|_| "BPSの変更後サイズがこの環境で扱える範囲を超えています".to_owned())?;
    let metadata_size = usize::try_from(decode(commands, &mut pos)?)
        .map_err(|_| "BPSのメタデータサイズがこの環境で扱える範囲を超えています".to_owned())?;
    pos = pos
        .checked_add(metadata_size)
        .filter(|&end| end <= commands_end)
        .ok_or_else(|| "BPSのメタデータがパッチの範囲を超えています".to_owned())?;
    Ok(Header {
        source_size,
        target_size,
        commands: pos,
    })
}

fn move_cursor(cursor: usize, value: u64) -> Result<usize, Error> {
    let distance = usize::try_from(value >> 1)
        .map_err(|_| "BPSの相対位置がこの環境で扱える範囲を超えています".to_owned())?;
    if value & 1 != 0 {
        cursor.checked_sub(distance)
    } else {
        cursor.checked_add(distance)
    }
    .ok_or_else(|| "BPSの相対位置が範囲外です".to_owned())
}

pub fn apply(patch: &[u8], input: &[u8]) -> Result<Applied, Error> {
    let (commands_end, source_crc, target_crc) = footer(patch, MAGIC)?;
    let header = header(patch, commands_end)?;
    let (source, reversed) = checked_input(
        input,
        header.source_size as u64,
        source_crc,
        header.target_size as u64,
        target_crc,
        false,
    )?;
    debug_assert!(!reversed);

    let mut output = buffer(header.target_size, 0)?;
    let mut pos = header.commands;
    let mut outpos = 0usize;
    let mut source_cursor = 0usize;
    let mut target_cursor = 0usize;
    let commands = &patch[..commands_end];

    while pos < commands_end {
        let command = decode(commands, &mut pos)?;
        let action = command & 3;
        let length = usize::try_from(
            (command >> 2)
                .checked_add(1)
                .ok_or_else(|| "BPSのコマンド長がu64の範囲を超えています".to_owned())?,
        )
        .map_err(|_| "BPSのコマンド長がこの環境で扱える範囲を超えています".to_owned())?;
        let out_end = outpos
            .checked_add(length)
            .filter(|&end| end <= header.target_size)
            .ok_or_else(|| "BPSのコマンドが変更後ファイルの範囲を超えています".to_owned())?;

        match action {
            0 => {
                let source_end = outpos
                    .checked_add(length)
                    .filter(|&end| end <= header.source_size)
                    .ok_or_else(|| {
                        "BPSのSourceReadが変更前ファイルの範囲を超えています".to_owned()
                    })?;
                output[outpos..out_end].copy_from_slice(&source[outpos..source_end]);
            }
            1 => {
                let patch_end = pos
                    .checked_add(length)
                    .filter(|&end| end <= commands_end)
                    .ok_or_else(|| {
                        "BPSのTargetReadがコマンド領域の範囲を超えています".to_owned()
                    })?;
                output[outpos..out_end].copy_from_slice(&patch[pos..patch_end]);
                pos = patch_end;
            }
            2 => {
                source_cursor = move_cursor(source_cursor, decode(commands, &mut pos)?)?;
                let source_end = source_cursor
                    .checked_add(length)
                    .filter(|&end| end <= header.source_size)
                    .ok_or_else(|| {
                        "BPSのSourceCopyが変更前ファイルの範囲を超えています".to_owned()
                    })?;
                output[outpos..out_end].copy_from_slice(&source[source_cursor..source_end]);
                source_cursor = source_end;
            }
            3 => {
                target_cursor = move_cursor(target_cursor, decode(commands, &mut pos)?)?;
                if target_cursor >= outpos {
                    return Err("BPSのTargetCopyが未出力領域を参照しています".to_owned());
                }
                target_cursor
                    .checked_add(length)
                    .filter(|&end| end <= header.target_size)
                    .ok_or_else(|| {
                        "BPSのTargetCopyが変更後ファイルの範囲を超えています".to_owned()
                    })?;
                for destination in outpos..out_end {
                    output[destination] = output[target_cursor];
                    target_cursor += 1;
                }
            }
            _ => unreachable!(),
        }
        outpos = out_end;
    }

    if pos != commands_end || outpos != header.target_size {
        return Err("BPSのコマンド領域または変更後サイズが一致しません".to_owned());
    }
    if crc32(&output) != target_crc {
        return Err("BPS適用後のCRC32が一致しません".to_owned());
    }
    Ok(Applied {
        data: output,
        warnings: Vec::new(),
    })
}

pub fn info(patch: &[u8]) -> Result<Info, Error> {
    let (commands_end, source_crc, target_crc) = footer(patch, MAGIC)?;
    let header = header(patch, commands_end)?;
    Ok(Info::Checksummed {
        format: Format::Bps,
        source_size: header.source_size as u64,
        target_size: header.target_size as u64,
        source_crc,
        target_crc,
    })
}

#[derive(Clone, Copy)]
enum Match {
    Source(usize),
    Target(usize),
}

fn common_prefix(data: &[u8], a: usize, b: usize, limit: usize) -> usize {
    let maximum = limit.min(data.len() - a).min(data.len() - b);
    let mut length = 0;
    while length < maximum && data[a + length] == data[b + length] {
        length += 1;
    }
    length
}

fn best_match(
    data: &[u8],
    suffixes: &[i32],
    rank: &[i32],
    target_size: usize,
    outpos: usize,
) -> Option<(Match, usize)> {
    let own_rank = rank[outpos] as usize;
    let valid = |position: usize| position < outpos || position >= target_size;

    let mut before = None;
    for &position in suffixes[..own_rank].iter().rev() {
        let position = position as usize;
        if valid(position) {
            before = Some(position);
            break;
        }
    }
    let mut after = None;
    for &position in &suffixes[own_rank + 1..] {
        let position = position as usize;
        if valid(position) {
            after = Some(position);
            break;
        }
    }

    let search_length = target_size - outpos;
    let position = match (before, after) {
        (Some(a), Some(b)) => {
            let shared = common_prefix(data, a, b, usize::MAX);
            if shared >= search_length
                || a.checked_add(shared)
                    .is_some_and(|p| p < data.len() && data[outpos + shared] == data[p])
            {
                a
            } else {
                b
            }
        }
        (Some(position), None) | (None, Some(position)) => position,
        (None, None) => return None,
    };
    let length = common_prefix(data, outpos, position, search_length);
    let location = if position < target_size {
        Match::Target(position)
    } else {
        Match::Source(position - target_size)
    };
    Some((location, length))
}

fn varint_cost(distance: usize) -> usize {
    if distance < 128 {
        1
    } else if distance < 128usize.pow(2) {
        2
    } else if distance < 128usize.pow(3) {
        3
    } else if distance < 128usize.pow(4) {
        4
    } else {
        5
    }
}

fn distance(a: usize, b: usize) -> usize {
    a.abs_diff(b)
}

fn relative(from: usize, to: usize) -> u64 {
    ((distance(from, to) as u64) << 1) | u64::from(to < from)
}

fn command(action: u64, length: usize, output: &mut Vec<u8>) {
    encode((((length - 1) as u64) << 2) | action, output);
}

fn flush_target_read(start: &mut Option<usize>, end: usize, target: &[u8], output: &mut Vec<u8>) {
    if let Some(begin) = start.take() {
        command(1, end - begin, output);
        output.extend_from_slice(&target[begin..end]);
    }
}

pub fn create(source: &[u8], target: &[u8]) -> Result<Vec<u8>, Error> {
    let total = source
        .len()
        .checked_add(target.len())
        .filter(|&size| size < i32::MAX as usize)
        .ok_or_else(|| "BPS作成では変更前と変更後の合計サイズを2GB未満にしてください".to_owned())?;

    let mut data = Vec::new();
    data.try_reserve_exact(total)
        .map_err(|_| "BPS作成用バッファのメモリを確保できません".to_owned())?;
    data.extend_from_slice(target);
    data.extend_from_slice(source);

    let mut suffixes = Vec::new();
    suffixes
        .try_reserve_exact(total)
        .map_err(|_| "BPS接尾辞配列のメモリを確保できません".to_owned())?;
    suffixes.resize(total, 0i32);
    if total != 0 {
        divsufsort::sort_in_place(&data, &mut suffixes);
    }

    let mut rank = Vec::new();
    rank.try_reserve_exact(total)
        .map_err(|_| "BPS逆引き配列のメモリを確保できません".to_owned())?;
    rank.resize(total, 0i32);
    for (order, &position) in suffixes.iter().enumerate() {
        rank[position as usize] = order as i32;
    }

    let capacity = target
        .len()
        .checked_mul(2)
        .and_then(|size| size.checked_add(64))
        .ok_or_else(|| "BPSパッチのサイズがこの環境で扱える範囲を超えています".to_owned())?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(capacity)
        .map_err(|_| "BPSパッチ用バッファのメモリを確保できません".to_owned())?;
    output.extend_from_slice(MAGIC);
    encode(source.len() as u64, &mut output);
    encode(target.len() as u64, &mut output);
    encode(0, &mut output);

    let mut outpos = 0usize;
    let mut source_cursor = 0usize;
    let mut target_cursor = 0usize;
    let mut pending = None;
    while outpos < target.len() {
        let (location, length) = best_match(&data, &suffixes, &rank, target.len(), outpos)
            .unwrap_or((Match::Target(0), 0));
        let cost = match location {
            Match::Source(position) if position == outpos => 1,
            Match::Source(position) => 1 + varint_cost(distance(position, source_cursor)),
            Match::Target(position) => 1 + varint_cost(distance(position, target_cursor)),
        };
        let minimum = 1 + cost + usize::from(pending.is_some()) + usize::from(length == 1);
        if length < minimum {
            pending.get_or_insert(outpos);
            outpos += 1;
            continue;
        }

        flush_target_read(&mut pending, outpos, target, &mut output);
        match location {
            Match::Source(position) if position == outpos => command(0, length, &mut output),
            Match::Source(position) => {
                command(2, length, &mut output);
                encode(relative(source_cursor, position), &mut output);
                source_cursor = position + length;
            }
            Match::Target(position) => {
                command(3, length, &mut output);
                encode(relative(target_cursor, position), &mut output);
                target_cursor = position + length;
            }
        }
        outpos += length;
    }
    flush_target_read(&mut pending, outpos, target, &mut output);
    append_checksums(&mut output, source, target);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_patch(source: &[u8], target: &[u8], metadata: &[u8], commands: &[u8]) -> Vec<u8> {
        let mut patch = MAGIC.to_vec();
        encode(source.len() as u64, &mut patch);
        encode(target.len() as u64, &mut patch);
        encode(metadata.len() as u64, &mut patch);
        patch.extend_from_slice(metadata);
        patch.extend_from_slice(commands);
        append_checksums(&mut patch, source, target);
        patch
    }

    #[test]
    fn round_trips_size_relationships_and_repetition() {
        for (source, target) in [
            (&b"abcdefghij"[..], &b"abcXYZghij"[..]),
            (&b"short"[..], &b"short and longer short and longer"[..]),
            (&b"this input is longer"[..], &b"tiny"[..]),
            (&b""[..], &b"aaaaaaaaaaaaaaaa"[..]),
            (&b"same"[..], &b"same"[..]),
            (&b"nonempty"[..], &b""[..]),
        ] {
            let patch = create(source, target).unwrap();
            assert_eq!(apply(&patch, source).unwrap().data, target);
        }
    }

    #[test]
    fn rejects_corruption_and_already_patched_input() {
        let patch = create(b"source", b"target").unwrap();
        assert!(apply(&patch, b"target").is_err());
        let mut corrupt = patch;
        corrupt[4] ^= 1;
        assert!(apply(&corrupt, b"source").is_err());
    }

    #[test]
    fn overlapping_target_copy_is_applied_one_byte_at_a_time() {
        let mut commands = Vec::new();
        command(1, 2, &mut commands);
        commands.extend_from_slice(b"ab");
        command(3, 14, &mut commands);
        encode(0, &mut commands);
        let patch = raw_patch(b"", b"abababababababab", b"", &commands);
        assert_eq!(apply(&patch, b"").unwrap().data, b"abababababababab");
    }

    #[test]
    fn signed_source_copy_can_move_backward() {
        let mut commands = Vec::new();
        command(2, 2, &mut commands);
        encode(relative(0, 4), &mut commands);
        command(2, 2, &mut commands);
        encode(relative(6, 0), &mut commands);
        let patch = raw_patch(b"abcdef", b"efab", b"", &commands);
        assert_eq!(apply(&patch, b"abcdef").unwrap().data, b"efab");
    }

    #[test]
    fn match_threshold_accounts_for_a_pending_target_read() {
        let commands = |source: &[u8], target: &[u8]| {
            let patch = create(source, target).unwrap();
            let (end, _, _) = footer(&patch, MAGIC).unwrap();
            let start = header(&patch, end).unwrap().commands;
            patch[start..end].to_vec()
        };

        assert_eq!(commands(b"abX", b"abY"), [0x84, 0x81, b'Y']);
        assert_eq!(commands(b"ZabcX", b"YabcQ"), [0x81, b'Y', 0x88, 0x81, b'Q']);
        assert_eq!(commands(b"ZabX", b"YabQ"), [0x8d, b'Y', b'a', b'b', b'Q']);
    }

    #[test]
    fn rejects_each_command_kind_outside_its_allowed_range() {
        let mut source_read = Vec::new();
        command(0, 2, &mut source_read);
        assert!(apply(&raw_patch(b"a", b"aa", b"", &source_read), b"a").is_err());

        let mut target_read = Vec::new();
        command(1, 2, &mut target_read);
        target_read.push(b'a');
        assert!(apply(&raw_patch(b"", b"aa", b"", &target_read), b"").is_err());

        let mut source_copy = Vec::new();
        command(2, 1, &mut source_copy);
        encode(3, &mut source_copy);
        assert!(apply(&raw_patch(b"a", b"a", b"", &source_copy), b"a").is_err());

        let mut target_copy = Vec::new();
        command(3, 1, &mut target_copy);
        encode(0, &mut target_copy);
        assert!(apply(&raw_patch(b"", b"a", b"", &target_copy), b"").is_err());
    }

    #[test]
    fn rejects_metadata_and_sizes_outside_the_patch_or_platform_range() {
        let mut metadata = MAGIC.to_vec();
        encode(0, &mut metadata);
        encode(0, &mut metadata);
        encode(100, &mut metadata);
        append_checksums(&mut metadata, b"", b"");
        assert!(apply(&metadata, b"").is_err());

        let mut huge = MAGIC.to_vec();
        encode(0, &mut huge);
        encode(u64::MAX, &mut huge);
        encode(0, &mut huge);
        append_checksums(&mut huge, b"", b"");
        assert!(apply(&huge, b"").is_err());
    }
}

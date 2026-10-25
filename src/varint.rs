pub fn encode(mut value: u64, out: &mut Vec<u8>) {
    loop {
        let byte = (value & 127) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte | 128);
            return;
        }
        out.push(byte);
        value -= 1;
    }
}

pub fn decode(data: &[u8], pos: &mut usize) -> Result<u64, String> {
    let mut value = 0u64;
    let mut shift = 1u64;
    loop {
        let byte = *data.get(*pos).ok_or("可変長整数が途中で終了しています")?;
        *pos += 1;
        value = u64::from(byte & 127)
            .checked_mul(shift)
            .and_then(|part| value.checked_add(part))
            .ok_or("可変長整数がu64の範囲を超えています")?;
        if byte & 128 != 0 {
            return Ok(value);
        }
        shift = shift
            .checked_mul(128)
            .ok_or("可変長整数がu64の範囲を超えています")?;
        value = value
            .checked_add(shift)
            .ok_or("可変長整数がu64の範囲を超えています")?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn boundaries() {
        for value in [0, 1, 127, 128, 255, 16511, 16512, u32::MAX as u64, u64::MAX] {
            let mut bytes = Vec::new();
            encode(value, &mut bytes);
            let mut pos = 0;
            assert_eq!(decode(&bytes, &mut pos).unwrap(), value);
            assert_eq!(pos, bytes.len());
        }
        assert!(decode(&[0; 20], &mut 0).is_err());
        assert!(decode(&[1], &mut 0).is_err());
        let mut bytes = Vec::new();
        encode(128, &mut bytes);
        assert_eq!(bytes, [0, 128]);
    }
}

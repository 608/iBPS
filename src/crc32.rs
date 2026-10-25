const TABLE: [u32; 256] = {
    let mut table = [0; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut bit = 0;
        while bit < 8 {
            c = (c >> 1) ^ if c & 1 != 0 { 0xEDB88320 } else { 0 };
            bit += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
};

pub fn crc32(data: &[u8]) -> u32 {
    !data.iter().fold(!0u32, |crc, &byte| {
        (crc >> 8) ^ TABLE[((crc ^ byte as u32) & 255) as usize]
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ieee_vectors() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"123456789"), 0xcbf43926);
    }
}

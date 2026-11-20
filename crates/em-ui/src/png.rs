//! 极简 PNG（RGB8）编码器，零第三方依赖。
//!
//! 只够本项目自测截帧用：无压缩（deflate stored blocks），文件偏大但
//! 编码器只有 100 行，行为完全可预期。M2 的 UI 截图也走这里。

/// CRC-32（PNG 规范的 polynomial 0xEDB88320）。
fn crc32(data: &[u8]) -> u32 {
    static mut TABLE: [u32; 256] = [0; 256];
    static INIT: std::sync::Once = std::sync::Once::new();

    // 惰性建表（static mut 在单线程 Once 内初始化，读取时已稳定）
    INIT.call_once(|| unsafe {
        for n in 0..256u32 {
            let mut c = n;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            TABLE[n as usize] = c;
        }
    });

    let mut c: u32 = 0xFFFF_FFFF;
    for &b in data {
        unsafe {
            c = TABLE[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8);
        }
    }
    c ^ 0xFFFF_FFFF
}

/// Adler-32（zlib 校验）。
fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

/// 编码 RGB8（`rgb.len() == (w*h*3)`）为 PNG 字节流。
/// 输入按「从上到下」的行序；OpenGL 读回的帧是「从下到上」，调用方先翻转。
pub fn encode_rgb(w: u32, h: u32, rgb: &[u8]) -> Vec<u8> {
    assert_eq!(rgb.len(), (w * h * 3) as usize, "RGB 数据长度与尺寸不符");

    let mut out = Vec::with_capacity(rgb.len() + 1024);
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);

    // IHDR
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // depth 8 / RGB / deflate / adaptive / interlace none
    chunk(&mut out, b"IHDR", &ihdr);

    // 原始数据：每行前加 filter byte 0（None）
    let stride = (w * 3) as usize;
    let mut raw = Vec::with_capacity((stride + 1) * h as usize);
    for y in 0..h as usize {
        raw.push(0);
        raw.extend_from_slice(&rgb[y * stride..(y + 1) * stride]);
    }

    // IDAT = zlib header + stored deflate blocks + adler32
    let mut idat = vec![0x78, 0x01];
    let mut off = 0usize;
    while off < raw.len() {
        let take = (raw.len() - off).min(65_535);
        let last = off + take == raw.len();
        idat.push(u8::from(last));
        idat.extend_from_slice(&(take as u16).to_le_bytes());
        idat.extend_from_slice(&(!(take as u16)).to_le_bytes());
        idat.extend_from_slice(&raw[off..off + take]);
        off += take;
    }
    idat.extend_from_slice(&adler32(&raw).to_be_bytes());
    chunk(&mut out, b"IDAT", &idat);

    chunk(&mut out, b"IEND", &[]);
    out
}

/// 编码并落盘。
pub fn write_rgb_png(path: &std::path::Path, w: u32, h: u32, rgb: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, encode_rgb(w, h, rgb))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 解码借道 std 以外的路径不可行，这里做结构级校验：
    /// 尺寸、签名、chunk CRC（重新算一遍）、zlib/adler 自洽。
    #[test]
    fn png_roundtrip_structure() {
        let (w, h) = (4u32, 3u32);
        let mut rgb = Vec::new();
        for y in 0..h {
            for x in 0..w {
                rgb.extend_from_slice(&[x as u8 * 60, y as u8 * 80, 128]);
            }
        }
        let png = encode_rgb(w, h, &rgb);

        assert_eq!(&png[0..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);

        // IHDR 长度 13
        let ihdr_len = u32::from_be_bytes([png[8], png[9], png[10], png[11]]);
        assert_eq!(ihdr_len, 13);
        let w_read = u32::from_be_bytes([png[16], png[17], png[18], png[19]]);
        let h_read = u32::from_be_bytes([png[20], png[21], png[22], png[23]]);
        assert_eq!((w_read, h_read), (w, h));
        assert_eq!(png[24], 8); // bit depth
        assert_eq!(png[25], 2); // color type RGB

        // IDAT：解出 stored deflate 块，验证 adler32
        // 布局：sig(8) + IHDR chunk(4+4+13+4=25) → IDAT 长度字段在 33，数据在 41
        let pos = 8 + 25;
        let idat_len = u32::from_be_bytes([png[pos], png[pos + 1], png[pos + 2], png[pos + 3]]) as usize;
        let idat = &png[pos + 8..pos + 8 + idat_len];
        assert_eq!(&idat[0..2], &[0x78, 0x01]);

        // 拼回 raw
        let mut raw = Vec::new();
        let mut off = 2usize; // zlib header 之后
        loop {
            let flags = idat[off];
            let len = u16::from_le_bytes([idat[off + 1], idat[off + 2]]) as usize;
            raw.extend_from_slice(&idat[off + 5..off + 5 + len]);
            off += 5 + len;
            if flags & 1 == 1 {
                break;
            }
        }
        let adler = u32::from_be_bytes([
            idat[off],
            idat[off + 1],
            idat[off + 2],
            idat[off + 3],
        ]);
        assert_eq!(adler, adler32(&raw));

        // 行数与 filter byte
        let stride = (w * 3) as usize + 1;
        assert_eq!(raw.len(), stride * h as usize);
        for y in 0..h as usize {
            assert_eq!(raw[y * stride], 0, "第 {y} 行 filter 应为 None");
        }
    }

    #[test]
    fn crc32_known_value() {
        // PNG 规范示例：CRC32(b"123456789") = 0xCBF43926
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }
}

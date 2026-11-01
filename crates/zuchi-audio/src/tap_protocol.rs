//! 補助プログラムの stdout の形式。定義は `native/macos/AudioTap.swift` の冒頭を参照。

use std::io::{self, Read};

const MAGIC: &[u8; 4] = b"ZTAP";
const HEADER_LEN: usize = 16;
/// 壊れたヘッダで大きな確保をしないための上限
const MAX_BODY: usize = 16 * 1024 * 1024;

#[derive(Debug, PartialEq)]
pub struct Packet {
    pub sample_rate: u32,
    pub channels: usize,
    pub samples: Vec<f32>,
}

/// 次のパケットを読む。終端に達したら `Ok(None)`。
pub fn read_packet(reader: &mut impl Read) -> io::Result<Option<Packet>> {
    let mut header = [0u8; HEADER_LEN];
    match reader.read_exact(&mut header) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    if &header[..4] != MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "パケットの先頭が不正です",
        ));
    }
    let field = |at: usize| u32::from_le_bytes(header[at..at + 4].try_into().expect("4 バイト"));
    let body_len = field(12) as usize;
    if body_len > MAX_BODY || !body_len.is_multiple_of(4) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "パケットの長さが不正です",
        ));
    }

    let mut body = vec![0u8; body_len];
    reader.read_exact(&mut body)?;
    let samples = body
        .as_chunks::<4>()
        .0
        .iter()
        .map(|&b| f32::from_le_bytes(b))
        .collect();
    Ok(Some(Packet {
        sample_rate: field(4),
        channels: field(8) as usize,
        samples,
    }))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn encode(rate: u32, channels: u32, samples: &[f32]) -> Vec<u8> {
        let mut bytes = MAGIC.to_vec();
        bytes.extend(rate.to_le_bytes());
        bytes.extend(channels.to_le_bytes());
        bytes.extend((samples.len() as u32 * 4).to_le_bytes());
        samples.iter().for_each(|s| bytes.extend(s.to_le_bytes()));
        bytes
    }

    #[test]
    fn 連続したパケットを読む() {
        let mut bytes = encode(48_000, 2, &[]);
        bytes.extend(encode(44_100, 2, &[0.5, -0.5]));
        let mut reader = Cursor::new(bytes);

        let ready = read_packet(&mut reader).unwrap().unwrap();
        assert_eq!((ready.sample_rate, ready.samples.len()), (48_000, 0));
        let audio = read_packet(&mut reader).unwrap().unwrap();
        assert_eq!(audio.sample_rate, 44_100);
        assert_eq!(audio.channels, 2);
        assert_eq!(audio.samples, vec![0.5, -0.5]);
        assert_eq!(read_packet(&mut reader).unwrap(), None);
    }

    #[test]
    fn 少しずつ届いても読める() {
        struct OneByte(Cursor<Vec<u8>>);
        impl Read for OneByte {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                let end = buffer.len().min(1);
                self.0.read(&mut buffer[..end])
            }
        }
        let mut reader = OneByte(Cursor::new(encode(48_000, 2, &[0.25, 0.75])));
        let packet = read_packet(&mut reader).unwrap().unwrap();
        assert_eq!(packet.samples, vec![0.25, 0.75]);
    }

    #[test]
    fn 不正な先頭を拒否する() {
        let mut bytes = encode(48_000, 2, &[]);
        bytes[0] = b'X';
        let result = read_packet(&mut Cursor::new(bytes));
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn 不正な長さを拒否する() {
        let mut bytes = encode(48_000, 2, &[]);
        bytes[12] = 3;
        let result = read_packet(&mut Cursor::new(bytes));
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
    }
}

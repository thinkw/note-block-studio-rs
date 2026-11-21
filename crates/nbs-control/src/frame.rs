//! 行分隔 JSON 帧。一条消息一行，禁止内嵌裸换行，读写两侧都要设上限，
//! 否则对端一个畸形大包就能把这边内存吃掉。

use std::io::{self, BufRead, Write};

use serde::de::DeserializeOwned;
use serde::Serialize;

/// 单帧上限：32 MiB。导出报告、几千格的音符列表都远小于它，超了就是坏数据。
pub const MAX_FRAME_BYTES: usize = 32 * 1024 * 1024;

fn too_large() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("frame larger than {MAX_FRAME_BYTES} bytes"),
    )
}

/// 写一行 JSON。换行由本函数补，调用方不用管。
pub fn write_json_line<W, T>(writer: &mut W, value: &T) -> io::Result<()>
where
    W: Write,
    T: Serialize,
{
    serde_json::to_writer(&mut *writer, value).map_err(io::Error::other)?;
    writer.write_all(b"\n")
}

/// 读一行 JSON。`Ok(None)` = 对端干净收尾（读到 EOF 且没有残留字节）。
///
/// 这里刻意不缓冲超过一行的数据：同一块内存里可能还有下一条消息，
/// 所以调用方要么全程用同一个 `BufRead`，要么别指望「读完一条还剩多少」。
pub fn read_json_line<R, T>(reader: &mut R) -> io::Result<Option<T>>
where
    R: BufRead,
    T: DeserializeOwned,
{
    let mut buf = Vec::new();
    loop {
        let hit = reader.read_until(b'\n', &mut buf)?;
        if hit == 0 {
            if buf.iter().all(|b| b.is_ascii_whitespace()) {
                return Ok(None);
            }
            // 没有换行的尾巴：仍然尝试解析，坏数据就报格式错而不是静默丢掉。
            return parse_frame(&buf).map(Some);
        }
        if buf.len() > MAX_FRAME_BYTES {
            return Err(too_large());
        }
        if buf.last() == Some(&b'\n') {
            buf.truncate(buf.len() - 1);
            if buf.last() == Some(&b'\r') {
                buf.truncate(buf.len() - 1);
            }
            if buf.is_empty() {
                // 空行：对端在保活或者我们多读了一个换行，跳过继续。
                buf.clear();
                continue;
            }
            return parse_frame(&buf).map(Some);
        }
    }
}

fn parse_frame<T: DeserializeOwned>(buf: &[u8]) -> io::Result<T> {
    serde_json::from_slice(buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

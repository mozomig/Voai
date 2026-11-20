//! Recognition worker protocols shared by the two isolated native engines.
pub mod file;
use std::io::{self, BufReader, BufWriter, Read, Write};
use voai_l10n::{tf, tr};

pub const RATE: usize = 16_000;
pub const MIN_SAMPLES: usize = RATE / 8;
pub const MAX_SAMPLES: usize = RATE * 60 * 5;
pub const PROTOCOL: &[u8; 8] = b"VOAIS001";
const MAX_REPLY: usize = 1 << 20;

fn read_u32(r: &mut impl Read) -> io::Result<u32> {
    let mut bytes = [0; 4];
    r.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn write_reply(w: &mut impl Write, reply: Result<&str, &str>) -> io::Result<()> {
    let (code, text) = match reply {
        Ok(text) => (0, text),
        Err(text) => (1, text),
    };
    if text.len() > MAX_REPLY {
        return Err(io::Error::other(tr("speech.reply.large")));
    }
    w.write_all(&[code])?;
    w.write_all(&(text.len() as u32).to_le_bytes())?;
    w.write_all(text.as_bytes())?;
    w.flush()
}

pub fn read_reply(r: &mut impl Read) -> Result<String, String> {
    fn read(r: &mut impl Read) -> io::Result<(u8, String)> {
        let mut status = [0];
        r.read_exact(&mut status)?;
        let size = read_u32(r)? as usize;
        if size > MAX_REPLY {
            return Err(io::Error::other(tr("speech.reply.large")));
        }
        let mut bytes = vec![0; size];
        r.read_exact(&mut bytes)?;
        let text = String::from_utf8(bytes).map_err(io::Error::other)?;
        Ok((status[0], text))
    }
    match read(r) {
        Ok((0, text)) => Ok(text),
        Ok((1, error)) => Err(error),
        Ok(_) => Err(tr("speech.reply.invalid").into()),
        Err(e) => Err(tf!("speech.worker.lost", e = e)),
    }
}

pub fn serve<D>(load: impl FnOnce() -> Result<D, String>) -> io::Result<()>
where
    D: FnMut(&[f32], bool) -> Result<String, String>,
{
    let stdin = io::stdin().lock();
    let stdout = io::stdout().lock();
    let mut input = BufReader::new(stdin);
    let mut output = BufWriter::new(stdout);
    let mut protocol = [0; 8];
    input.read_exact(&mut protocol)?;
    if &protocol != PROTOCOL {
        return Err(io::Error::other(tr("speech.protocol")));
    }

    let mut decode = match load() {
        Ok(decode) => decode,
        Err(error) => return write_reply(&mut output, Err(&error)),
    };
    write_reply(&mut output, Ok(""))?;
    loop {
        let mut kind = [0u8];
        match input.read_exact(&mut kind) {
            Ok(()) if kind[0] < 2 => {}
            Ok(()) => return Err(io::Error::other(tr("speech.command"))),
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e),
        }
        let size = read_u32(&mut input)? as usize;
        if size > MAX_SAMPLES {
            return Err(io::Error::other(tr("speech.limit.samples")));
        }
        // The parent and helper use native little-endian f32 on Apple Silicon.
        let mut samples = vec![0.0f32; size];
        input.read_exact(bytemuck::cast_slice_mut(&mut samples))?;
        let result = decode(&samples, kind[0] == 1);
        write_reply(&mut output, result.as_deref().map_err(String::as_str))?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bad_reply_length_is_rejected_before_allocation() {
        let bytes = [0, 255, 255, 255, 255];
        assert!(read_reply(&mut &bytes[..]).is_err());
    }
    #[test]
    fn preserves_error_and_unicode_text() {
        for expected in [Ok("Сегодня пятница 🎙"), Err("Нет модели")] {
            let mut bytes = vec![];
            write_reply(&mut bytes, expected).unwrap();
            assert_eq!(
                read_reply(&mut bytes.as_slice())
                    .as_deref()
                    .map_err(String::as_str),
                expected
            );
        }
    }
}

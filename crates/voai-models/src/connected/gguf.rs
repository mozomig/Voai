//! Bound native metadata allocations before handing user-supplied weights to
//! llama.cpp. Only single-file models can be tracked by one file identity.
use super::{ModelKind, read_u32, read_u64};
use anyhow::{Context, Result, bail, ensure};
use std::{
    fs::File,
    io::{BufReader, Read, Seek},
};
use voai_l10n::tr;

pub(super) fn check(r: &mut BufReader<&File>, bytes: u64, kind: ModelKind) -> Result<()> {
    ensure!(matches!(read_u32(r)?, 2 | 3), tr("gguf.version"));
    let tensors = read_u64(r)?;
    let pairs = read_u64(r)?;
    ensure!(
        (1..=65_536).contains(&tensors)
            && (1..=65_536).contains(&pairs)
            && tensors * 32 + pairs * 13 <= bytes,
        tr("gguf.header")
    );
    let mut speech_architecture = false;
    for _ in 0..pairs {
        let key_len = read_u64(r)?;
        ensure!((1..=1024).contains(&key_len), tr("gguf.parameter"));
        let mut key = vec![0; key_len as usize];
        r.read_exact(&mut key)?;
        let value_type = read_u32(r)?;
        if key == b"general.architecture" {
            ensure!(value_type == 8, tr("gguf.parameter"));
            let len = read_u64(r)?;
            ensure!((1..=128).contains(&len), tr("gguf.parameter"));
            let mut arch = vec![0; len as usize];
            r.read_exact(&mut arch)?;
            speech_architecture = matches!(arch.as_slice(), b"gigaam" | b"parakeet");
        } else if key == b"split.count" {
            ensure!(value_type == 2, tr("gguf.split.type"));
            let mut split = [0; 2];
            r.read_exact(&mut split)?;
            ensure!(u16::from_le_bytes(split) <= 1, tr("gguf.multipart"));
        } else {
            let (element, count) = if value_type == 9 {
                (read_u32(r)?, read_u64(r)?)
            } else {
                (value_type, 1)
            };
            ensure!(count <= 4_194_304, tr("gguf.array"));
            // Numeric GGUF scalars and vectors have a fixed encoded width.
            let width = match element {
                0 | 1 | 7 => 1,
                2 | 3 => 2,
                4..=6 => 4,
                10..=12 => 8,
                8 => 0,
                _ => bail!(tr("gguf.type")),
            };
            if width == 0 {
                ensure!(count <= 1_048_576, tr("gguf.vocab"));
                for _ in 0..count {
                    let len = read_u64(r)?;
                    ensure!(len <= 16 * 1024 * 1024, tr("gguf.string"));
                    r.seek_relative(len as i64)?;
                }
            } else {
                r.seek_relative((count * width) as i64)?;
            }
        }
        let pos = r.stream_position().context(tr("gguf.read"))?;
        ensure!(
            pos <= 256 * 1024 * 1024 && pos + tensors * 24 < bytes,
            tr("gguf.metadata")
        );
    }
    ensure!(
        speech_architecture == (kind == ModelKind::Speech),
        tr("model.gguf.kind")
    );
    Ok(())
}

//! Validate legacy whisper.cpp GGML tensor records without reading their
//! weight payloads. This format has no tensor directory or payload lengths.
use super::read_u32;
use anyhow::{Result, bail, ensure};
use std::{
    collections::HashSet,
    fs::File,
    io::{BufReader, Read, Seek},
};

// (elements per block, encoded bytes per block), from whisper.cpp's pinned
// ggml-common.h. Type numbers are file-format identifiers, not model ftypes.
fn layout(t: u32) -> Result<(u64, u64)> {
    Ok(match t {
        0 => (1, 4),
        1 | 30 => (1, 2),
        2 | 20 => (32, 18),
        3 => (32, 20),
        6 => (32, 22),
        7 => (32, 24),
        8 => (32, 34),
        10 => (256, 84),
        11 => (256, 110),
        12 => (256, 144),
        13 => (256, 176),
        14 => (256, 210),
        16 => (256, 66),
        17 => (256, 74),
        18 => (256, 98),
        19 => (256, 50),
        21 => (256, 110),
        22 => (256, 82),
        23 => (256, 136),
        29 => (256, 56),
        39 => (32, 17),
        _ => bail!("Неподдерживаемый тип весов whisper.cpp"),
    })
}

pub(super) fn check_weights(
    r: &mut BufReader<&File>,
    file_bytes: u64,
    wtype: u32,
    width: u32,
    vocab: u32,
    audio_layers: u32,
    text_layers: u32,
) -> Result<()> {
    // Model ftype encodes the quantizer version in the thousands and uses
    // distinct identifiers for its primary tensor storage type.
    let t = match wtype % 1000 {
        0..=3 | 10..=14 => wtype % 1000,
        7 => 8,
        8 => 6,
        9 => 7,
        15..=22 => wtype % 1000 + 1,
        23 => 29,
        24 => 30,
        25 => 39,
        _ => bail!("Неподдерживаемая квантизация whisper.cpp"),
    };
    let (elements, bytes_per_block) = layout(t)?;
    let width = u64::from(width);
    let weights =
        width * u64::from(vocab) + width * width * u64::from(12 * audio_layers + 16 * text_layers);
    let start = r.stream_position()?;
    ensure!(
        start < file_bytes && weights / elements * bytes_per_block <= file_bytes - start,
        "Размер весов не соответствует параметрам Whisper"
    );

    let mut names = HashSet::new();
    while r.stream_position()? < file_bytes {
        let dimensions = read_u32(r)?;
        let name_bytes = read_u32(r)?;
        let tensor_type = read_u32(r)?;
        ensure!(
            (1..=4).contains(&dimensions) && (1..=256).contains(&name_bytes) && names.len() < 4096,
            "Некорректное описание тензора Whisper"
        );
        let (block, size) = layout(tensor_type)?;
        let mut elements = 1u64;
        for axis in 0..dimensions {
            let dim = u64::from(read_u32(r)?);
            ensure!(
                dim > 0
                    && dim <= i32::MAX as u64 / elements
                    && (axis != 0 || dim.is_multiple_of(block)),
                "Некорректный размер тензора Whisper"
            );
            elements *= dim;
        }
        let mut name = vec![0; name_bytes as usize];
        r.read_exact(&mut name)?;
        ensure!(
            std::str::from_utf8(&name).is_ok() && !name.contains(&0) && names.insert(name),
            "Некорректное или повторное имя тензора Whisper"
        );
        let payload = elements / block * size;
        ensure!(
            r.stream_position()? + payload <= file_bytes,
            "Файл весов Whisper обрезан"
        );
        r.seek_relative(payload as i64)?;
    }
    ensure!(!names.is_empty(), "В файле отсутствуют веса Whisper");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_truncated_final_tensor() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("weights");
        let mut data = Vec::new();
        for n in [1u32, 1, 0, 32] {
            data.extend(n.to_le_bytes());
        }
        data.push(b'w');
        data.resize(data.len() + 32 * 4, 0);
        let check = |data: &[u8]| {
            std::fs::write(&path, data).unwrap();
            let f = File::open(&path).unwrap();
            check_weights(&mut BufReader::new(&f), data.len() as u64, 0, 1, 1, 1, 1)
        };
        assert!(check(&data).is_ok());
        data.pop();
        assert!(check(&data).is_err());
    }
}

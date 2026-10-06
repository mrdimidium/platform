// SPDX-FileCopyrightText: 2026 Nikolay Govorov
// SPDX-License-Identifier: MPL-2.0

use std::io::{self, Write as _};

const MIN_SOURCE_BYTES: usize = 256;
const MIN_SAVINGS_BYTES: usize = 64;
const MAX_RELATIVE_PERCENT: usize = 90;

pub struct Variants {
    pub gzip: Option<Vec<u8>>,
    pub brotli: Option<Vec<u8>>,
}

pub fn compress(bytes: &[u8]) -> io::Result<Variants> {
    if bytes.len() < MIN_SOURCE_BYTES {
        return Ok(Variants {
            gzip: None,
            brotli: None,
        });
    }

    let gzip = gzip(bytes)?;
    let gzip = worthwhile(&gzip, bytes).then_some(gzip);

    let brotli = brotli(bytes)?;
    let brotli_fallback = gzip.as_deref().unwrap_or(bytes);
    let brotli = worthwhile(&brotli, brotli_fallback).then_some(brotli);

    Ok(Variants { gzip, brotli })
}

fn worthwhile(candidate: &[u8], fallback: &[u8]) -> bool {
    candidate.len().saturating_add(MIN_SAVINGS_BYTES) <= fallback.len()
        && candidate.len().saturating_mul(100)
            <= fallback.len().saturating_mul(MAX_RELATIVE_PERCENT)
}

fn gzip(bytes: &[u8]) -> io::Result<Vec<u8>> {
    let mut encoder = flate2::GzBuilder::new()
        .mtime(0)
        .write(Vec::new(), flate2::Compression::best());
    encoder.write_all(bytes)?;
    encoder.finish()
}

fn brotli(bytes: &[u8]) -> io::Result<Vec<u8>> {
    let mut output = Vec::new();
    {
        let mut encoder = brotli::CompressorWriter::new(&mut output, 4096, 11, 22);
        encoder.write_all(bytes)?;
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use std::{fmt::Write as _, io::Read as _};

    use super::*;

    #[test]
    fn keeps_only_material_improvements_over_the_fallback() {
        let mut input = String::new();
        for index in 0..1_024 {
            write!(
                input,
                "<p class=\"item-{}\">repeated markup {index}</p>",
                index % 31
            )
            .unwrap();
        }
        let input = input.into_bytes();
        let variants = compress(&input).unwrap();
        let gzip = variants.gzip.unwrap();
        let brotli = variants.brotli.unwrap();

        assert!(worthwhile(&gzip, &input));
        assert!(worthwhile(&brotli, &gzip));

        let mut decoded = Vec::new();
        flate2::read::GzDecoder::new(gzip.as_slice())
            .read_to_end(&mut decoded)
            .unwrap();
        assert_eq!(decoded, input);

        let mut decoded = Vec::new();
        brotli::Decompressor::new(brotli.as_slice(), 4096)
            .read_to_end(&mut decoded)
            .unwrap();
        assert_eq!(decoded, input);
    }

    #[test]
    fn rejects_tiny_and_marginal_variants() {
        let tiny = compress(b"User-agent: *\n").unwrap();
        assert!(tiny.gzip.is_none());
        assert!(tiny.brotli.is_none());

        assert!(!worthwhile(&[0; 901], &[0; 1_000]));
        assert!(worthwhile(&[0; 900], &[0; 1_000]));
        assert!(!worthwhile(&[0; 100], &[0; 163]));
        assert!(worthwhile(&[0; 99], &[0; 163]));
    }

    #[test]
    fn gzip_output_is_reproducible() {
        let bytes = b"deterministic gzip content".repeat(64);
        assert_eq!(gzip(&bytes).unwrap(), gzip(&bytes).unwrap());
    }
}

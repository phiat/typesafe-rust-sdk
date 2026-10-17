//! Compressing request bodies.

use crate::error::Error;

/// Bodies shorter than this are sent as they are, as the official SDKs do:
/// compressing them saves nothing worth the work.
pub(crate) const MIN_COMPRESSED_BODY: usize = 1024;

/// How to compress request bodies, which the API accepts as `gzip` or `zstd`.
///
/// Compression is off unless asked for, with [`Config::compression`](crate::Config::compression)
/// for every call or [`CallOptions::compression`](crate::CallOptions::compression)
/// for one. It pays on large states, such as a long document or a chat log. A body
/// under 1 KiB is sent as it is.
///
/// Each codec is a cargo feature, so a build only carries what it uses:
///
/// - `gzip` adds [`Compression::Gzip`], in pure Rust
/// - `zstd` adds [`Compression::Zstd`], which compresses faster and smaller but
///   needs a C compiler to build. It is what the official SDKs use.
///
/// Requests with images are compressed too, though PNG and JPEG data won't shrink.
///
/// ```
/// use typesafe::{CallOptions, Compression, Config};
///
/// # #[cfg(feature = "zstd")]
/// # {
/// let config = Config::from_env().compression(Compression::Zstd);
/// // and off again for one call:
/// let options = CallOptions::new().compression(Compression::Off);
/// # }
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Compression {
    /// Send bodies as they are.
    #[default]
    Off,
    /// `Content-Encoding: gzip`.
    #[cfg(feature = "gzip")]
    Gzip,
    /// `Content-Encoding: zstd`, at zstd's default level.
    #[cfg(feature = "zstd")]
    Zstd,
}

#[cfg(any(feature = "gzip", feature = "zstd"))]
fn failed(e: std::io::Error) -> Error {
    Error::invalid_request(format!("the request body could not be compressed: {e}"))
}

impl Compression {
    /// The body to send, and the `content-encoding` that describes it. A body
    /// under [`MIN_COMPRESSED_BODY`] comes back unchanged, with no encoding.
    pub(crate) fn encode(self, body: Vec<u8>) -> Result<(Vec<u8>, Option<&'static str>), Error> {
        if body.len() < MIN_COMPRESSED_BODY {
            return Ok((body, None));
        }
        match self {
            Compression::Off => Ok((body, None)),
            #[cfg(feature = "gzip")]
            Compression::Gzip => {
                use std::io::Write;
                let mut encoder =
                    flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
                encoder.write_all(&body).map_err(failed)?;
                Ok((encoder.finish().map_err(failed)?, Some("gzip")))
            }
            #[cfg(feature = "zstd")]
            Compression::Zstd => {
                let compressed = zstd::encode_all(&body[..], 0).map_err(failed)?;
                Ok((compressed, Some("zstd")))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn big() -> Vec<u8> {
        b"the same sentence, over and over. ".repeat(100)
    }

    #[test]
    fn off_and_small_bodies_pass_through() {
        assert_eq!(Compression::Off.encode(big()).unwrap(), (big(), None));
        let small = vec![b'x'; MIN_COMPRESSED_BODY - 1];
        for codec in codecs() {
            assert_eq!(codec.encode(small.clone()).unwrap(), (small.clone(), None));
        }
    }

    fn codecs() -> Vec<Compression> {
        let codecs: &[Compression] = &[
            #[cfg(feature = "gzip")]
            Compression::Gzip,
            #[cfg(feature = "zstd")]
            Compression::Zstd,
        ];
        codecs.to_vec()
    }

    #[cfg(feature = "gzip")]
    #[test]
    fn gzip_round_trips() {
        use std::io::Read;
        let (compressed, encoding) = Compression::Gzip.encode(big()).unwrap();
        assert_eq!(encoding, Some("gzip"));
        assert!(compressed.len() < big().len() / 10);
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(&compressed[..])
            .read_to_end(&mut out)
            .unwrap();
        assert_eq!(out, big());
    }

    #[cfg(feature = "zstd")]
    #[test]
    fn zstd_round_trips() {
        let (compressed, encoding) = Compression::Zstd.encode(big()).unwrap();
        assert_eq!(encoding, Some("zstd"));
        assert!(compressed.len() < big().len() / 10);
        assert_eq!(zstd::decode_all(&compressed[..]).unwrap(), big());
    }

    #[cfg(feature = "zstd")]
    #[test]
    fn zstd_stays_inside_the_apis_window() {
        // The API takes zstd windows up to 8 MiB. Level 0 is zstd's default
        // (3), whose window is 2 MiB, so even a large body is accepted.
        let big = (0..4 * 1024 * 1024u32)
            .flat_map(|i| i.to_le_bytes())
            .collect::<Vec<u8>>();
        let (compressed, _) = Compression::Zstd.encode(big.clone()).unwrap();
        let mut decoder = zstd::Decoder::new(&compressed[..]).unwrap();
        decoder.window_log_max(23).unwrap(); // 2^23 = 8 MiB
        let mut out = Vec::new();
        std::io::Read::read_to_end(&mut decoder, &mut out).unwrap();
        assert_eq!(out, big);
    }
}

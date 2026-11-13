//! Optional Opus decoding, enabled with the `opus` cargo feature.

// Some items are only used when the feature is enabled.
#![allow(dead_code)]

pub const RATE: u32 = 48_000;
pub const CHANNELS: usize = 2;
/// Samples per channel in one 20 ms frame.
pub const FRAME_SAMPLES: usize = 960;
/// The largest frame libopus can return (120 ms).
pub const MAX_FRAME_SAMPLES: usize = 5760;

pub struct Decoder {
    #[cfg(feature = "opus")]
    inner: opus::Decoder,
    channels: usize,
}

impl Decoder {
    pub fn new(channels: usize) -> anyhow::Result<Self> {
        #[cfg(feature = "opus")]
        {
            let inner = opus::Decoder::new(RATE, opus::Channels::Stereo)
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            Ok(Decoder { inner, channels })
        }

        #[cfg(not(feature = "opus"))]
        {
            let _ = channels;
            anyhow::bail!("server built without opus support (rebuild with --features opus)")
        }
    }

    /// Decode one packet, or conceal one lost 20 ms frame when `packet` is None.
    /// Returns samples per channel.
    pub fn decode(&mut self, packet: Option<&[u8]>, out: &mut [i16]) -> anyhow::Result<usize> {
        #[cfg(feature = "opus")]
        {
            let samples = match packet {
                Some(packet) => self
                    .inner
                    .decode(packet, out, false)
                    .map_err(|e| anyhow::anyhow!("{}", e))?,
                None => self
                    .inner
                    .decode(&[], &mut out[..FRAME_SAMPLES * self.channels], false)
                    .map_err(|e| anyhow::anyhow!("{}", e))?,
            };
            Ok(samples)
        }

        #[cfg(not(feature = "opus"))]
        {
            let _ = (packet, out);
            unreachable!("decoder cannot be constructed without the opus feature")
        }
    }
}

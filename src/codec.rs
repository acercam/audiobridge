use audiopus::coder::{Decoder, Encoder};
use audiopus::{Application, Bitrate, Channels, SampleRate};

use crate::protocol::{FRAME_SAMPLES, OPUS_BITRATE};

pub struct OpusCodec {
    encoder: Encoder,
    decoder: Decoder,
    encode_buf: Vec<u8>,
}

impl OpusCodec {
    pub fn new() -> Result<Self, audiopus::Error> {
        let sample_rate = SampleRate::Hz24000;
        let channels = Channels::Mono;
        let mut encoder = Encoder::new(sample_rate, channels, Application::Audio)?;
        encoder.set_bitrate(Bitrate::BitsPerSecond(OPUS_BITRATE))?;
        encoder.set_vbr(true)?;
        let decoder = Decoder::new(sample_rate, channels)?;
        Ok(Self {
            encoder,
            decoder,
            encode_buf: vec![0u8; 4000],
        })
    }

    pub fn encode(&mut self, pcm: &[i16]) -> Result<&[u8], audiopus::Error> {
        let len = self.encoder.encode(pcm, &mut self.encode_buf)?;
        Ok(&self.encode_buf[..len])
    }

    pub fn decode(&mut self, data: &[u8], out: &mut [i16]) -> Result<usize, audiopus::Error> {
        self.decoder.decode(Some(data), out, false)
    }
}

pub fn f32_to_i16(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|&s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
        .collect()
}

pub fn i16_to_f32(samples: &[i16]) -> Vec<f32> {
    samples
        .iter()
        .map(|&s| s as f32 / i16::MAX as f32)
        .collect()
}

pub fn resample_linear(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate {
        return input.to_vec();
    }
    let ratio = from_rate as f64 / to_rate as f64;
    let out_len = ((input.len() as f64) / ratio).floor() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src = i as f64 * ratio;
        let idx = src.floor() as usize;
        let frac = (src - idx as f64) as f32;
        let a = input.get(idx).copied().unwrap_or(0.0);
        let b = input.get(idx + 1).copied().unwrap_or(a);
        out.push(a + (b - a) * frac);
    }
    out
}

pub const TARGET_FRAME_SAMPLES: usize = FRAME_SAMPLES;

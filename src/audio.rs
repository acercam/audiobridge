use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, Stream, StreamConfig};

use crate::codec::{f32_to_i16, i16_to_f32, resample_linear, TARGET_FRAME_SAMPLES};
use crate::protocol::SAMPLE_RATE;

pub fn list_devices() -> Result<(), Box<dyn std::error::Error>> {
    let host = cpal::default_host();
    println!(
        "Default input:  {:?}",
        host.default_input_device().map(|d| d.name())
    );
    println!(
        "Default output: {:?}",
        host.default_output_device().map(|d| d.name())
    );
    println!();
    println!("Input devices:");
    for (i, dev) in host.input_devices()?.enumerate() {
        println!("  [{i}] {}", dev.name()?);
    }
    println!();
    println!("Output devices:");
    for (i, dev) in host.output_devices()?.enumerate() {
        println!("  [{i}] {}", dev.name()?);
    }
    Ok(())
}

pub fn resolve_device(name: Option<&str>, input: bool) -> Result<Device, String> {
    let host = cpal::default_host();
    if let Some(query) = name {
        let devices: Vec<Device> = if input {
            host.input_devices()
                .map_err(|e| e.to_string())?
                .collect()
        } else {
            host.output_devices()
                .map_err(|e| e.to_string())?
                .collect()
        };
        devices
            .into_iter()
            .find(|d| {
                d.name()
                    .map(|n| n.to_lowercase().contains(&query.to_lowercase()))
                    .unwrap_or(false)
            })
            .ok_or_else(|| {
                format!(
                    "No {} device matching '{query}'",
                    if input { "input" } else { "output" }
                )
            })
    } else if input {
        host.default_input_device()
            .ok_or_else(|| "No default input device".to_string())
    } else {
        host.default_output_device()
            .ok_or_else(|| "No default output device".to_string())
    }
}

/// Thread-safe audio buffers shared with network threads.
#[derive(Clone)]
pub struct AudioBuffers {
    capture: Arc<Mutex<Vec<i16>>>,
    playback: Arc<Mutex<Vec<i16>>>,
}

impl AudioBuffers {
    pub fn drain_capture_frame(&self) -> Option<Vec<i16>> {
        let mut buf = self.capture.lock().ok()?;
        if buf.len() >= TARGET_FRAME_SAMPLES {
            Some(buf.drain(..TARGET_FRAME_SAMPLES).collect())
        } else {
            None
        }
    }

    pub fn push_playback_frame(&self, frame: &[i16]) {
        if let Ok(mut buf) = self.playback.lock() {
            buf.extend_from_slice(frame);
            let max = TARGET_FRAME_SAMPLES * 25;
            if buf.len() > max {
                let extra = buf.len() - max;
                buf.drain(..extra);
            }
        }
    }

    pub fn clear_playback(&self) {
        if let Ok(mut buf) = self.playback.lock() {
            buf.clear();
        }
    }
}

/// Keeps cpal streams alive on the creating thread (not Send on macOS).
pub struct AudioGuard {
    _input_stream: Stream,
    _output_stream: Stream,
}

pub fn start_audio(device_name: Option<&str>) -> Result<(AudioGuard, AudioBuffers), String> {
    let input_dev = resolve_device(device_name, true)?;
    let output_dev = resolve_device(device_name, false)?;

    let in_cfg = preferred_config(&input_dev, true)?;
    let out_cfg = preferred_config(&output_dev, false)?;
    let in_rate = in_cfg.sample_rate.0;
    let out_rate = out_cfg.sample_rate.0;

    let capture = Arc::new(Mutex::new(Vec::<i16>::new()));
    let playback = Arc::new(Mutex::new(Vec::<i16>::new()));
    let buffers = AudioBuffers {
        capture: Arc::clone(&capture),
        playback: Arc::clone(&playback),
    };

    let cap_buf = Arc::clone(&capture);
    let input_stream = build_input_stream(&input_dev, &in_cfg, move |data: &[f32]| {
        let mut f32_samples = data.to_vec();
        if in_rate != SAMPLE_RATE {
            f32_samples = resample_linear(&f32_samples, in_rate, SAMPLE_RATE);
        }
        let pcm = f32_to_i16(&f32_samples);
        if let Ok(mut buf) = cap_buf.lock() {
            buf.extend_from_slice(&pcm);
        }
    })?;

    let play_buf = Arc::clone(&playback);
    let out_channels = out_cfg.channels as usize;
    let output_stream = build_output_stream(&output_dev, &out_cfg, move |data: &mut [f32]| {
        let frames = data.len() / out_channels;
        let needed = if out_rate == SAMPLE_RATE {
            frames
        } else {
            ((frames as f64 * SAMPLE_RATE as f64) / out_rate as f64).ceil() as usize
        };

        let mut mono = vec![0i16; needed];
        if let Ok(mut buf) = play_buf.lock() {
            let take = needed.min(buf.len());
            mono[..take].copy_from_slice(&buf[..take]);
            buf.drain(..take);
        }

        let mut f32_mono = i16_to_f32(&mono);
        if out_rate != SAMPLE_RATE {
            f32_mono = resample_linear(&f32_mono, SAMPLE_RATE, out_rate);
        }

        for frame in 0..frames {
            let sample = f32_mono.get(frame).copied().unwrap_or(0.0);
            for ch in 0..out_channels {
                data[frame * out_channels + ch] = sample;
            }
        }
    })?;

    input_stream.play().map_err(|e| e.to_string())?;
    output_stream.play().map_err(|e| e.to_string())?;

    let guard = AudioGuard {
        _input_stream: input_stream,
        _output_stream: output_stream,
    };

    Ok((guard, buffers))
}

fn preferred_config(device: &Device, input: bool) -> Result<StreamConfig, String> {
    let default = if input {
        device.default_input_config()
    } else {
        device.default_output_config()
    }
    .map_err(|e| e.to_string())?;

    Ok(StreamConfig {
        channels: 1,
        sample_rate: default.sample_rate(),
        buffer_size: cpal::BufferSize::Default,
    })
}

fn build_input_stream(
    device: &Device,
    config: &StreamConfig,
    mut data_fn: impl FnMut(&[f32]) + Send + 'static,
) -> Result<Stream, String> {
    let format = device
        .default_input_config()
        .map(|c| c.sample_format())
        .unwrap_or(SampleFormat::F32);

    match format {
        SampleFormat::F32 => device
            .build_input_stream(
                config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| data_fn(data),
                |e| eprintln!("input stream error: {e}"),
                None,
            )
            .map_err(|e| e.to_string()),
        SampleFormat::I16 => device
            .build_input_stream(
                config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    let f32_data: Vec<f32> = data
                        .iter()
                        .map(|&s| s as f32 / i16::MAX as f32)
                        .collect();
                    data_fn(&f32_data);
                },
                |e| eprintln!("input stream error: {e}"),
                None,
            )
            .map_err(|e| e.to_string()),
        other => Err(format!("Unsupported input sample format: {other:?}")),
    }
}

fn build_output_stream(
    device: &Device,
    config: &StreamConfig,
    mut data_fn: impl FnMut(&mut [f32]) + Send + 'static,
) -> Result<Stream, String> {
    let format = device
        .default_output_config()
        .map(|c| c.sample_format())
        .unwrap_or(SampleFormat::F32);

    match format {
        SampleFormat::F32 => device
            .build_output_stream(
                config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| data_fn(data),
                |e| eprintln!("output stream error: {e}"),
                None,
            )
            .map_err(|e| e.to_string()),
        SampleFormat::I16 => device
            .build_output_stream(
                config,
                move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                    let mut f32_data: Vec<f32> = data
                        .iter()
                        .map(|&s| s as f32 / i16::MAX as f32)
                        .collect();
                    data_fn(&mut f32_data);
                    for (out, f) in data.iter_mut().zip(f32_data.iter()) {
                        *out = (f.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                    }
                },
                |e| eprintln!("output stream error: {e}"),
                None,
            )
            .map_err(|e| e.to_string()),
        other => Err(format!("Unsupported output sample format: {other:?}")),
    }
}

pub fn resolve_device_name(cli: Option<String>) -> Option<String> {
    cli
}

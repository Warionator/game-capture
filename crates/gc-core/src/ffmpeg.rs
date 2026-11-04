//! Construction of FFmpeg command lines and parsing of `ffprobe` output.
//!
//! The capture pipeline is a single long-running FFmpeg process:
//!
//! ```text
//!  ddagrab (DXGI Desktop Duplication, D3D11 textures)
//!    └─[optional scale_d3d11 / vpp_amf / scale_qsv on the GPU]
//!        └─ h264|hevc|av1 _nvenc|_amf|_qsv   (D3D11 frames in, no CPU copy)
//!  named pipes  \\.\pipe\gc-<id>-<n>  (f32le 48 kHz stereo from WASAPI)
//!    └─ [afftdn] ─ asplit ─┬─ isolated track n
//!                          └─ amix → Master track
//!  → MPEG-TS on stdout → replay ring buffer / session file (Rust side)
//! ```
//!
//! Keeping this as plain argument construction makes every pipeline variant
//! unit-testable without a GPU.

use crate::config::{Codec, EncoderPreference, FrameRate, RateControl, Resolution};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

pub const SAMPLE_RATE: u32 = 48_000;
/// Stereo; the pipe inputs declare `-ch_layout stereo`.
pub const CHANNELS: u32 = 2;
/// Keyframe interval; also the granularity of the replay buffer.
pub const KEYFRAME_SECS: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Backend {
    Nvenc,
    Amf,
    Qsv,
    Software,
}

impl Backend {
    pub const ALL: [Backend; 4] = [
        Backend::Nvenc,
        Backend::Amf,
        Backend::Qsv,
        Backend::Software,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Backend::Nvenc => "NVIDIA NVENC",
            Backend::Amf => "AMD AMF",
            Backend::Qsv => "Intel QuickSync",
            Backend::Software => "Software (CPU)",
        }
    }

    pub fn is_hardware(self) -> bool {
        self != Backend::Software
    }
}

/// FFmpeg encoder name for a backend/codec pair.
pub fn encoder_name(backend: Backend, codec: Codec) -> &'static str {
    use Backend::*;
    use Codec::*;
    match (backend, codec) {
        (Nvenc, H264) => "h264_nvenc",
        (Nvenc, Hevc) => "hevc_nvenc",
        (Nvenc, Av1) => "av1_nvenc",
        (Amf, H264) => "h264_amf",
        (Amf, Hevc) => "hevc_amf",
        (Amf, Av1) => "av1_amf",
        (Qsv, H264) => "h264_qsv",
        (Qsv, Hevc) => "hevc_qsv",
        (Qsv, Av1) => "av1_qsv",
        (Software, H264) => "libx264",
        (Software, Hevc) => "libx265",
        (Software, Av1) => "libsvtav1",
    }
}

/// Backends to try, in order, for a preference. An explicit choice still
/// falls back to software so capture never silently stops working.
pub fn backend_candidates(pref: EncoderPreference) -> Vec<Backend> {
    match pref {
        EncoderPreference::Auto => Backend::ALL.to_vec(),
        EncoderPreference::Nvenc => vec![Backend::Nvenc, Backend::Software],
        EncoderPreference::Amf => vec![Backend::Amf, Backend::Software],
        EncoderPreference::Qsv => vec![Backend::Qsv, Backend::Software],
        EncoderPreference::Software => vec![Backend::Software],
    }
}

/// Parses `ffmpeg -hide_banner -encoders` output into encoder names.
pub fn parse_encoder_list(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let flags = it.next()?;
            let name = it.next()?;
            // Flag column looks like "V....D"; skip the legend lines.
            (flags.len() == 6 && flags.starts_with(['V', 'A', 'S']) && name != "=")
                .then(|| name.to_string())
        })
        .collect()
}

/// Parses `ffmpeg -hide_banner -filters` output into filter names.
pub fn parse_filter_list(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let flags = it.next()?;
            let name = it.next()?;
            let io = it.next()?;
            (flags.len() == 3 && io.contains("->")).then(|| name.to_string())
        })
        .collect()
}

/// GPU-side scaler available for the zero-copy path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuScaler {
    /// `scale_d3d11` (FFmpeg ≥ 8): D3D11 video processor, also converts
    /// BGRA → NV12, usable by NVENC, AMF and (after hwmap) QSV.
    D3d11,
    /// `vpp_amf` (FFmpeg ≥ 7.1).
    Amf,
    /// `scale_qsv` after mapping into a QSV device.
    Qsv,
}

/// What FFmpeg build features are available (probed once at startup).
#[derive(Debug, Clone, Default)]
pub struct FfmpegCaps {
    pub encoders: Vec<String>,
    pub filters: Vec<String>,
}

impl FfmpegCaps {
    pub fn has_encoder(&self, name: &str) -> bool {
        self.encoders.iter().any(|e| e == name)
    }
    pub fn has_filter(&self, name: &str) -> bool {
        self.filters.iter().any(|f| f == name)
    }
    /// Best GPU scaler for a backend, if any.
    pub fn gpu_scaler(&self, backend: Backend) -> Option<GpuScaler> {
        match backend {
            Backend::Software => None,
            Backend::Amf if self.has_filter("vpp_amf") => Some(GpuScaler::Amf),
            Backend::Qsv if self.has_filter("scale_qsv") => Some(GpuScaler::Qsv),
            _ if self.has_filter("scale_d3d11") => Some(GpuScaler::D3d11),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum VideoSource {
    /// DXGI Desktop Duplication of one monitor via the `ddagrab` filter source.
    Ddagrab {
        /// DXGI adapter the monitor is attached to (`None` = default adapter).
        /// Hybrid-GPU laptops often drive the panel from the iGPU.
        adapter_idx: Option<u32>,
        /// Output index *within* that adapter.
        output_idx: u32,
        draw_mouse: bool,
        /// Capture a sub-rectangle (e.g. a windowed game) instead of the whole
        /// output: `(x, y, w, h)` relative to the output.
        region: Option<(i32, i32, u32, u32)>,
    },
    /// Windows Graphics Capture of a single window via `gfxcapture`
    /// (FFmpeg ≥ 8). Only that window's pixels are captured, so overlapping
    /// windows and notifications never appear. Output is scaled to
    /// `source_size` if the window is resized.
    Window {
        hwnd: u64,
        draw_mouse: bool,
        adapter_idx: Option<u32>,
    },
    /// Synthetic pattern; used by tests and on non-Windows development hosts.
    TestPattern,
}

impl VideoSource {
    fn is_gpu(&self) -> bool {
        matches!(
            self,
            VideoSource::Ddagrab { .. } | VideoSource::Window { .. }
        )
    }

    fn adapter(&self) -> Option<u32> {
        match self {
            VideoSource::Ddagrab { adapter_idx, .. } | VideoSource::Window { adapter_idx, .. } => {
                *adapter_idx
            }
            VideoSource::TestPattern => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioInput {
    /// URL FFmpeg opens, e.g. `\\.\pipe\gc-1234-0`.
    pub url: String,
    pub in_mix: bool,
    /// `afftdn` noise-reduction strength in dB.
    pub denoise: Option<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Output {
    /// MPEG-TS on stdout (consumed by the replay ring / session writer).
    StdoutTs,
    /// Direct file output (format inferred from extension).
    File(String),
}

/// The "Game not in focus" picture shown instead of the capture while the
/// game is in the background. Switched at runtime with [`focus_command`].
#[derive(Debug, Clone, PartialEq)]
pub struct FocusScreen {
    /// TTF for the caption (`drawtext`); `None` = no caption, plain screen.
    pub font_file: Option<String>,
}

/// Background colour of the focus screen (matches the app UI).
pub const FOCUS_SCREEN_COLOR: &str = "0x0b0d12";

/// Bytes to write to FFmpeg's stdin to show (`true`) or hide the focus
/// screen. Uses FFmpeg's interactive `c` command, which sends `map` to the
/// `streamselect` filter in the running graph.
pub fn focus_command(show_screen: bool) -> String {
    format!("cstreamselect -1 map {}\n", u8::from(show_screen))
}

#[derive(Debug, Clone, PartialEq)]
pub struct PipelineSpec {
    pub video: VideoSource,
    /// Captured (source) size in pixels.
    pub source_size: (u32, u32),
    pub fps: FrameRate,
    pub resolution: Resolution,
    pub backend: Backend,
    pub codec: Codec,
    pub rate: RateControl,
    /// Keep frames on the GPU between capture and encode.
    pub zero_copy: bool,
    pub gpu_scaler: Option<GpuScaler>,
    /// `scale_d3d11` is available (needed to merge the focus screen into the
    /// GPU path).
    pub has_scale_d3d11: bool,
    pub focus_screen: Option<FocusScreen>,
    pub audio: Vec<AudioInput>,
    pub mixdown: bool,
    pub audio_bitrate_kbps: u32,
    pub output: Output,
}

/// Escapes a value for use inside a single-quoted filter option.
fn filter_quote(v: &str) -> String {
    format!(
        "'{}'",
        v.replace('\\', "/").replace(':', "\\:").replace('\'', "")
    )
}

impl PipelineSpec {
    pub fn output_size(&self) -> (u32, u32) {
        let (w, h) = self.source_size;
        self.resolution.output_size(w, h)
    }

    /// Whether the generated graph keeps frames in GPU memory end to end.
    pub fn is_zero_copy(&self) -> bool {
        self.zero_copy
            && self.backend.is_hardware()
            && self.video.is_gpu()
            && (!self.needs_scale() || self.gpu_scaler.is_some() || self.focus_screen.is_some())
            && (self.focus_screen.is_none() || self.has_scale_d3d11)
    }

    fn needs_scale(&self) -> bool {
        let (w, h) = self.source_size;
        self.output_size() != (w & !1, h & !1)
    }

    fn capture_rate(&self) -> u32 {
        if self.fps.is_uncapped() {
            // ddagrab/gfxcapture treat the rate as an upper bound when
            // frame duplication is off.
            500
        } else {
            self.fps.0
        }
    }

    /// GPU-side chain after the capture (and focus switch) for the encoder.
    fn gpu_tail(&self, normalize: bool) -> Option<String> {
        let (ow, oh) = self.output_size();
        let scale = self.needs_scale();
        let d3d11 = format!("scale_d3d11=width={ow}:height={oh}:format=nv12");
        match (self.backend, scale, normalize) {
            // Mixing two frame sources: always re-render into one D3D11 pool.
            (Backend::Qsv, _, true) => Some(format!("{d3d11},hwmap=derive_device=qsv,format=qsv")),
            (_, _, true) => Some(d3d11),
            // NVENC and AMF accept D3D11 textures directly.
            (Backend::Nvenc | Backend::Amf, false, false) => None,
            (Backend::Qsv, false, false) => Some("hwmap=derive_device=qsv,format=qsv".into()),
            (Backend::Software, _, _) => unreachable!("is_zero_copy excludes software"),
            (_, true, false) => Some(
                match (
                    self.backend,
                    self.gpu_scaler.expect("checked by is_zero_copy"),
                ) {
                    (Backend::Amf, GpuScaler::Amf) => format!("vpp_amf=w={ow}:h={oh}:format=nv12"),
                    (Backend::Qsv, GpuScaler::Qsv) => {
                        format!("hwmap=derive_device=qsv,format=qsv,scale_qsv=w={ow}:h={oh}:format=nv12")
                    }
                    (Backend::Qsv, _) => format!("{d3d11},hwmap=derive_device=qsv,format=qsv"),
                    _ => d3d11,
                },
            ),
        }
    }

    /// CPU-side chain (scaling + pixel format for the encoder).
    fn cpu_tail(&self) -> String {
        let (ow, oh) = self.output_size();
        let mut chain = Vec::new();
        if self.needs_scale() {
            chain.push(format!("scale={ow}:{oh}:flags=bicubic"));
        }
        chain.push(
            if self.backend.is_hardware() {
                "format=nv12"
            } else {
                "format=yuv420p"
            }
            .to_string(),
        );
        chain.join(",")
    }

    /// The focus-screen source, sized like the capture.
    fn focus_source(&self, f: &FocusScreen, upload: bool) -> String {
        let (w, h) = self.source_size;
        let mut src = format!("color=c={FOCUS_SCREEN_COLOR}:s={w}x{h}:r=1");
        if let Some(font) = &f.font_file {
            let font = filter_quote(font);
            let big = (h / 16).max(16);
            let small = (h / 36).max(10);
            src.push_str(&format!(
                ",drawtext=fontfile={font}:text='Game not in focus':fontcolor=0xe6e9ef:fontsize={big}:x=(w-text_w)/2:y=(h/2)-text_h",
            ));
            src.push_str(&format!(
                ",drawtext=fontfile={font}:text='Capture resumes when you return to the game':fontcolor=0x9aa3b5:fontsize={small}:x=(w-text_w)/2:y=(h/2)+{small}",
            ));
        }
        src.push_str(",format=bgra");
        if upload {
            src.push_str(",hwupload");
        }
        src
    }

    /// Complete video part of the filter graph, ending in `[v]`.
    fn video_graph(&self, src: &str) -> String {
        let zero_copy = self.is_zero_copy();
        let fps = if self.fps.is_uncapped() {
            String::new()
        } else {
            format!(",fps={}", self.fps.0)
        };
        let download = if self.video.is_gpu() {
            ",hwdownload,format=bgra"
        } else {
            ""
        };
        match &self.focus_screen {
            None if zero_copy => match self.gpu_tail(false) {
                Some(t) => format!("{src},{t}[v]"),
                None => format!("{src}[v]"),
            },
            None => format!("{src}{download},{}[v]", self.cpu_tail()),
            Some(f) if zero_copy => format!(
                "{src}[cap];{}[fs];[cap][fs]streamselect=inputs=2:map=0{fps},{}[v]",
                self.focus_source(f, true),
                self.gpu_tail(true).expect("normalized tail")
            ),
            Some(f) => format!(
                "{src}{download}[cap];{}[fs];[cap][fs]streamselect=inputs=2:map=0{fps},{}[v]",
                self.focus_source(f, false),
                self.cpu_tail()
            ),
        }
    }

    /// Full argument vector (without the program name).
    pub fn build_args(&self) -> Vec<String> {
        let mut a: Vec<String> = Vec::new();
        let push =
            |a: &mut Vec<String>, items: &[&str]| a.extend(items.iter().map(|s| s.to_string()));
        push(
            &mut a,
            &["-hide_banner", "-loglevel", "warning", "-nostats"],
        );

        // A shared D3D11 device: needed to pick an adapter, and so the focus
        // screen is uploaded to the same device the capture uses.
        let needs_device =
            self.video.adapter().is_some() || (self.focus_screen.is_some() && self.is_zero_copy());
        if needs_device {
            let dev = match self.video.adapter() {
                Some(adapter) => format!("d3d11va=gc:{adapter}"),
                None => "d3d11va=gc".to_string(),
            };
            push(
                &mut a,
                &["-init_hw_device", &dev, "-filter_hw_device", "gc"],
            );
        }

        // --- video source (a filter-graph source, not an input file) ------
        let rate = self.capture_rate();
        let video_src = match &self.video {
            VideoSource::Ddagrab {
                output_idx,
                draw_mouse,
                region,
                ..
            } => {
                let mut src = format!(
                    "ddagrab=output_idx={output_idx}:framerate={rate}:draw_mouse={}",
                    u8::from(*draw_mouse)
                );
                if let Some((x, y, w, h)) = region {
                    src.push_str(&format!(":video_size={w}x{h}:offset_x={x}:offset_y={y}"));
                }
                if self.fps.is_uncapped() {
                    src.push_str(":dup_frames=0");
                }
                src
            }
            VideoSource::Window {
                hwnd, draw_mouse, ..
            } => {
                let (w, h) = self.source_size;
                format!(
                    "gfxcapture=hwnd={hwnd}:max_framerate={rate}:capture_cursor={}:width={w}:height={h}:resize_mode=scale_aspect:dup_frames={}",
                    u8::from(*draw_mouse),
                    u8::from(!self.fps.is_uncapped())
                )
            }
            VideoSource::TestPattern => {
                let (w, h) = self.source_size;
                format!("testsrc2=size={w}x{h}:rate={rate},realtime")
            }
        };

        // --- audio inputs (indices 0..n) ---------------------------------
        for input in &self.audio {
            push(
                &mut a,
                &[
                    "-thread_queue_size",
                    "4096",
                    "-probesize",
                    "32",
                    "-analyzeduration",
                    "0",
                    "-f",
                    "f32le",
                    "-ar",
                    &SAMPLE_RATE.to_string(),
                    "-ch_layout",
                    "stereo",
                    "-i",
                    &input.url,
                ],
            );
        }

        // --- filter graph --------------------------------------------------
        let mut graph: Vec<String> = vec![self.video_graph(&video_src)];
        let video_map = "[v]".to_string();

        let mix_members: Vec<usize> = (0..self.audio.len())
            .filter(|&i| self.audio[i].in_mix)
            .collect();
        let do_mix = self.mixdown && !mix_members.is_empty();
        let mut track_maps: Vec<String> = Vec::new();
        let mut mix_inputs: Vec<String> = Vec::new();
        for (i, input) in self.audio.iter().enumerate() {
            let idx = i;
            let mut label = format!("{idx}:a");
            if let Some(nr) = input.denoise {
                graph.push(format!("[{idx}:a]afftdn=nr={nr}:nf=-40[dn{idx}]"));
                label = format!("dn{idx}");
            }
            if do_mix && input.in_mix {
                graph.push(format!("[{label}]asplit=2[mx{idx}][tr{idx}]"));
                mix_inputs.push(format!("[mx{idx}]"));
                track_maps.push(format!("[tr{idx}]"));
            } else if label.contains(':') {
                track_maps.push(label);
            } else {
                track_maps.push(format!("[{label}]"));
            }
        }
        if do_mix {
            let mix = if mix_inputs.len() == 1 {
                format!("{}anull[mix]", mix_inputs[0])
            } else {
                format!(
                    "{}amix=inputs={}:normalize=0:dropout_transition=0,alimiter=limit=0.97:level=0[mix]",
                    mix_inputs.join(""),
                    mix_inputs.len()
                )
            };
            graph.push(mix);
            track_maps.insert(0, "[mix]".into());
        }
        push(&mut a, &["-filter_complex", &graph.join(";")]);

        push(&mut a, &["-map", &video_map]);
        for m in &track_maps {
            push(&mut a, &["-map", m]);
        }

        // --- video encoder ----------------------------------------------
        let enc = encoder_name(self.backend, self.codec);
        push(&mut a, &["-c:v", enc]);
        a.extend(rate_control_args(self.backend, self.codec, self.rate));
        a.extend(preset_args(self.backend, self.codec));
        if self.fps.is_uncapped() {
            push(&mut a, &["-fps_mode", "passthrough"]);
            let fk = format!("expr:gte(t,n_forced*{KEYFRAME_SECS})");
            push(&mut a, &["-force_key_frames", &fk]);
        } else {
            let g = (self.fps.0 * KEYFRAME_SECS).to_string();
            push(&mut a, &["-g", &g, "-keyint_min", &g]);
        }

        // --- audio encoder ----------------------------------------------
        if !track_maps.is_empty() {
            let br = format!("{}k", self.audio_bitrate_kbps);
            push(
                &mut a,
                &["-c:a", "aac", "-b:a", &br, "-ar", &SAMPLE_RATE.to_string()],
            );
        }

        // --- output -------------------------------------------------------
        match &self.output {
            Output::StdoutTs => push(
                &mut a,
                &[
                    "-flush_packets",
                    "1",
                    "-muxdelay",
                    "0",
                    "-muxpreload",
                    "0",
                    "-f",
                    "mpegts",
                    "pipe:1",
                ],
            ),
            Output::File(p) => push(&mut a, &["-y", p]),
        }
        a
    }
}

/// Rate-control arguments for a backend.
pub fn rate_control_args(backend: Backend, codec: Codec, rate: RateControl) -> Vec<String> {
    let k = |v: u32| format!("{v}k");
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    match (backend, rate) {
        (Backend::Nvenc, RateControl::Cbr { bitrate_kbps: b }) => s(&[
            "-rc",
            "cbr",
            "-b:v",
            &k(b),
            "-maxrate",
            &k(b),
            "-bufsize",
            &k(b * 2),
        ]),
        (
            Backend::Nvenc,
            RateControl::Vbr {
                bitrate_kbps: b,
                max_kbps: m,
            },
        ) => s(&[
            "-rc",
            "vbr",
            "-b:v",
            &k(b),
            "-maxrate",
            &k(m),
            "-bufsize",
            &k(m * 2),
        ]),
        (Backend::Nvenc, RateControl::Quality { level }) => {
            s(&["-rc", "constqp", "-qp", &level.to_string()])
        }

        (Backend::Amf, RateControl::Cbr { bitrate_kbps: b }) => s(&[
            "-rc",
            "cbr",
            "-b:v",
            &k(b),
            "-maxrate",
            &k(b),
            "-bufsize",
            &k(b * 2),
        ]),
        (
            Backend::Amf,
            RateControl::Vbr {
                bitrate_kbps: b,
                max_kbps: m,
            },
        ) => s(&[
            "-rc",
            "vbr_peak",
            "-b:v",
            &k(b),
            "-maxrate",
            &k(m),
            "-bufsize",
            &k(m * 2),
        ]),
        (Backend::Amf, RateControl::Quality { level }) => {
            let q = level.to_string();
            if codec == Codec::Av1 {
                // AV1 AMF uses a 0-255 qindex scale.
                let qi = (level as u32 * 255 / 51).to_string();
                s(&["-rc", "cqp", "-qp_i", &qi, "-qp_p", &qi])
            } else {
                s(&["-rc", "cqp", "-qp_i", &q, "-qp_p", &q, "-qp_b", &q])
            }
        }

        (Backend::Qsv, RateControl::Cbr { bitrate_kbps: b }) => s(&[
            "-b:v",
            &k(b),
            "-minrate",
            &k(b),
            "-maxrate",
            &k(b),
            "-bufsize",
            &k(b * 2),
        ]),
        (
            Backend::Qsv,
            RateControl::Vbr {
                bitrate_kbps: b,
                max_kbps: m,
            },
        ) => s(&["-b:v", &k(b), "-maxrate", &k(m), "-bufsize", &k(m * 2)]),
        (Backend::Qsv, RateControl::Quality { level }) => {
            s(&["-global_quality", &level.max(1).to_string()])
        }

        (Backend::Software, RateControl::Cbr { bitrate_kbps: b }) => {
            let mut v = s(&[
                "-b:v",
                &k(b),
                "-minrate",
                &k(b),
                "-maxrate",
                &k(b),
                "-bufsize",
                &k(b * 2),
            ]);
            if codec == Codec::H264 {
                v.extend(s(&["-x264-params", "nal-hrd=cbr"]));
            }
            v
        }
        (
            Backend::Software,
            RateControl::Vbr {
                bitrate_kbps: b,
                max_kbps: m,
            },
        ) => s(&["-b:v", &k(b), "-maxrate", &k(m), "-bufsize", &k(m * 2)]),
        (Backend::Software, RateControl::Quality { level }) => s(&["-crf", &level.to_string()]),
    }
}

/// Speed/quality presets tuned for low overhead while gaming.
pub fn preset_args(backend: Backend, codec: Codec) -> Vec<String> {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    match (backend, codec) {
        (Backend::Nvenc, _) => s(&["-preset", "p4", "-tune", "hq", "-forced-idr", "1"]),
        (Backend::Amf, _) => s(&["-usage", "transcoding", "-quality", "balanced"]),
        (Backend::Qsv, _) => s(&["-preset", "veryfast", "-low_power", "1"]),
        (Backend::Software, Codec::H264) => s(&["-preset", "veryfast", "-tune", "zerolatency"]),
        (Backend::Software, Codec::Hevc) => {
            s(&["-preset", "ultrafast", "-x265-params", "log-level=error"])
        }
        (Backend::Software, Codec::Av1) => s(&["-preset", "10"]),
    }
}

/// Arguments for a one-shot hardware probe: encode a few synthetic frames.
pub fn encoder_probe_args(encoder: &str) -> Vec<String> {
    [
        "-hide_banner",
        "-loglevel",
        "error",
        "-f",
        "lavfi",
        "-i",
        "color=black:size=1280x720:rate=30",
        "-frames:v",
        "3",
        "-pix_fmt",
        "nv12",
        "-c:v",
        encoder,
        "-f",
        "null",
        "-",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Options for remuxing buffered MPEG-TS into the final container.
#[derive(Debug, Clone, Default)]
pub struct RemuxSpec {
    /// Embedded metadata tags.
    pub tags: Vec<(String, String)>,
    /// Titles for audio streams in order.
    pub audio_titles: Vec<String>,
    /// Path to an `ffmetadata` file with chapters (bookmarks).
    pub chapters_file: Option<String>,
}

/// `ffmpeg` arguments that read MPEG-TS from `input` (`pipe:0` for stdin)
/// and copy all streams into `output` (MP4 or MKV by extension).
pub fn remux_args(input: &str, output: &Path, spec: &RemuxSpec) -> Vec<String> {
    let mut a: Vec<String> = ["-hide_banner", "-loglevel", "error", "-y"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    a.extend(["-fflags", "+genpts+discardcorrupt", "-f", "mpegts"].map(String::from));
    a.extend(["-i".to_string(), input.to_string()]);
    if let Some(ch) = &spec.chapters_file {
        a.extend(["-f", "ffmetadata", "-i"].map(String::from));
        a.push(ch.clone());
    }
    a.extend(["-map", "0:v?", "-map", "0:a?", "-c", "copy"].map(String::from));
    if spec.chapters_file.is_some() {
        a.extend(["-map_chapters", "1"].map(String::from));
    }
    a.extend(["-map_metadata", "-1", "-avoid_negative_ts", "make_zero"].map(String::from));
    for (k, v) in &spec.tags {
        a.push("-metadata".into());
        a.push(format!("{k}={v}"));
    }
    for (i, t) in spec.audio_titles.iter().enumerate() {
        // MP4 keeps per-track names in the handler (`hdlr`) atom; Matroska
        // uses the stream title. Write both.
        a.push(format!("-metadata:s:a:{i}"));
        a.push(format!("title={t}"));
        a.push(format!("-metadata:s:a:{i}"));
        a.push(format!("handler_name={t}"));
        a.push(format!("-disposition:a:{i}"));
        a.push(if i == 0 { "default" } else { "0" }.into());
    }
    let ext = output
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("mp4")
        .to_ascii_lowercase();
    if ext == "mkv" {
        a.extend(["-f", "matroska"].map(String::from));
    } else {
        a.extend(["-movflags", "+faststart+use_metadata_tags", "-f", "mp4"].map(String::from));
    }
    a.push(output.to_string_lossy().into_owned());
    a
}

/// Gain applied to one audio stream of a clip being edited.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackGain {
    /// Audio stream index within the file (0 = first audio track).
    pub index: usize,
    pub volume: f32,
    pub muted: bool,
}

impl TrackGain {
    pub fn gain(&self) -> f32 {
        if self.muted {
            0.0
        } else {
            self.volume.clamp(0.0, 2.0)
        }
    }
}

/// A trim/volume edit of an existing file.
#[derive(Debug, Clone, Default)]
pub struct ExportSpec {
    pub start_ms: u64,
    pub end_ms: u64,
    /// Number of audio streams in the input.
    pub audio_streams: usize,
    /// Track gains to bake in. `None` = audio is copied untouched.
    pub gains: Option<Vec<TrackGain>>,
    /// The first audio stream is a mix of the others (game-capture layout);
    /// it is rebuilt from the adjusted tracks.
    pub rebuild_master: bool,
    pub tags: Vec<(String, String)>,
    pub audio_titles: Vec<String>,
    pub chapters_file: Option<String>,
    pub audio_bitrate_kbps: u32,
}

/// `ffmpeg` arguments that trim `input` to `[start, end)` and optionally
/// re-mix its audio tracks. Video is always stream-copied (lossless, fast);
/// the cut therefore starts on the keyframe at or before `start`.
pub fn export_args(input: &Path, output: &Path, spec: &ExportSpec) -> Vec<String> {
    let mut a: Vec<String> = ["-hide_banner", "-loglevel", "error", "-y"]
        .map(String::from)
        .to_vec();
    if spec.start_ms > 0 {
        a.extend([
            "-ss".to_string(),
            format!("{:.3}", spec.start_ms as f64 / 1000.0),
        ]);
    }
    a.extend(["-i".to_string(), input.to_string_lossy().into_owned()]);
    if let Some(ch) = &spec.chapters_file {
        a.extend(["-f", "ffmetadata", "-i"].map(String::from));
        a.push(ch.clone());
    }
    let dur = spec.end_ms.saturating_sub(spec.start_ms).max(100);
    a.extend(["-t".to_string(), format!("{:.3}", dur as f64 / 1000.0)]);

    a.extend(["-map", "0:v?"].map(String::from));
    match &spec.gains {
        None => {
            a.extend(["-map", "0:a?", "-c", "copy"].map(String::from));
        }
        Some(gains) => {
            let gain_of = |i: usize| {
                gains
                    .iter()
                    .find(|g| g.index == i)
                    .map(|g| g.gain())
                    .unwrap_or(1.0)
            };
            let first = usize::from(spec.rebuild_master && spec.audio_streams > 1);
            let isolated: Vec<usize> = (first..spec.audio_streams).collect();
            let mut graph = Vec::new();
            let mut maps = Vec::new();
            let mut mix_in = Vec::new();
            for &i in &isolated {
                let g = gain_of(i);
                if first == 1 {
                    graph.push(format!("[0:a:{i}]volume={g:.3},asplit=2[t{i}][m{i}]"));
                    mix_in.push(format!("[m{i}]"));
                } else {
                    graph.push(format!("[0:a:{i}]volume={g:.3}[t{i}]"));
                }
                maps.push(format!("[t{i}]"));
            }
            if first == 1 {
                graph.push(format!(
                    "{}amix=inputs={}:normalize=0:dropout_transition=0,alimiter=limit=0.97:level=0[mix]",
                    mix_in.join(""),
                    mix_in.len()
                ));
                maps.insert(0, "[mix]".into());
            }
            a.extend(["-filter_complex".to_string(), graph.join(";")]);
            for m in maps {
                a.extend(["-map".to_string(), m]);
            }
            a.extend(["-c:v", "copy", "-c:a", "aac"].map(String::from));
            a.extend(["-b:a".to_string(), format!("{}k", spec.audio_bitrate_kbps)]);
        }
    }
    a.extend(["-map_metadata", "0"].map(String::from));
    if spec.chapters_file.is_some() {
        a.extend(["-map_chapters", "1"].map(String::from));
    } else {
        a.extend(["-map_chapters", "-1"].map(String::from));
    }
    for (k, v) in &spec.tags {
        a.push("-metadata".into());
        a.push(format!("{k}={v}"));
    }
    for (i, t) in spec.audio_titles.iter().enumerate() {
        a.push(format!("-metadata:s:a:{i}"));
        a.push(format!("title={t}"));
        a.push(format!("-metadata:s:a:{i}"));
        a.push(format!("handler_name={t}"));
        a.push(format!("-disposition:a:{i}"));
        a.push(if i == 0 { "default" } else { "0" }.into());
    }
    let ext = output
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("mp4")
        .to_ascii_lowercase();
    if ext == "mkv" {
        a.extend(["-f", "matroska"].map(String::from));
    } else {
        a.extend(["-movflags", "+faststart+use_metadata_tags", "-f", "mp4"].map(String::from));
    }
    a.push(output.to_string_lossy().into_owned());
    a
}

/// `ffmpeg` arguments extracting audio streams into separate `.m4a` files
/// (for per-track playback in the built-in player). `copy = false`
/// re-encodes to AAC for codecs MP4 audio cannot hold.
pub fn extract_tracks_args(
    input: &Path,
    outputs: &[(usize, std::path::PathBuf)],
    copy: bool,
) -> Vec<String> {
    let mut a: Vec<String> = ["-hide_banner", "-loglevel", "error", "-y", "-i"]
        .map(String::from)
        .to_vec();
    a.push(input.to_string_lossy().into_owned());
    for (i, out) in outputs {
        a.extend([
            "-map".to_string(),
            format!("0:a:{i}"),
            "-vn".into(),
            "-sn".into(),
        ]);
        if copy {
            a.extend(["-c:a", "copy"].map(String::from));
        } else {
            a.extend(["-c:a", "aac", "-b:a", "192k"].map(String::from));
        }
        a.extend(["-movflags", "+faststart", "-f", "mp4"].map(String::from));
        a.push(out.to_string_lossy().into_owned());
    }
    a
}

/// `ffmpeg` arguments to render a JPEG thumbnail.
pub fn thumbnail_args(input: &Path, output: &Path, at_secs: f64, is_image: bool) -> Vec<String> {
    let mut a: Vec<String> = ["-hide_banner", "-loglevel", "error", "-y"]
        .map(String::from)
        .to_vec();
    if !is_image && at_secs > 0.0 {
        a.extend(["-ss".to_string(), format!("{at_secs:.2}")]);
    }
    a.extend(["-i".to_string(), input.to_string_lossy().into_owned()]);
    a.extend(
        [
            "-frames:v",
            "1",
            "-vf",
            "scale=480:-2:flags=bicubic",
            "-q:v",
            "4",
            "-update",
            "1",
        ]
        .map(String::from),
    );
    a.push(output.to_string_lossy().into_owned());
    a
}

/// `ffprobe` arguments producing the JSON consumed by [`parse_probe`].
pub fn probe_args(input: &Path) -> Vec<String> {
    let mut a: Vec<String> = [
        "-v",
        "error",
        "-print_format",
        "json",
        "-show_format",
        "-show_streams",
        "-show_chapters",
    ]
    .map(String::from)
    .to_vec();
    a.push(input.to_string_lossy().into_owned());
    a
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeInfo {
    pub duration_ms: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub video_codec: Option<String>,
    /// Average video frame rate.
    pub fps: Option<f64>,
    pub audio_tracks: Vec<String>,
    pub tags: BTreeMap<String, String>,
    /// Chapter start offsets (ms) – bookmarks written by us or other tools.
    pub chapters_ms: Vec<u64>,
}

/// Parses `ffprobe -print_format json -show_format -show_streams` output.
pub fn parse_probe(json: &str) -> anyhow::Result<ProbeInfo> {
    #[derive(Deserialize)]
    struct Root {
        #[serde(default)]
        streams: Vec<Stream>,
        format: Option<Format>,
        #[serde(default)]
        chapters: Vec<Chapter>,
    }
    #[derive(Deserialize)]
    struct Stream {
        codec_type: Option<String>,
        codec_name: Option<String>,
        width: Option<u32>,
        height: Option<u32>,
        avg_frame_rate: Option<String>,
        #[serde(default)]
        tags: BTreeMap<String, String>,
        #[serde(default)]
        disposition: BTreeMap<String, i64>,
    }
    #[derive(Deserialize)]
    struct Format {
        duration: Option<String>,
        #[serde(default)]
        tags: BTreeMap<String, String>,
    }
    #[derive(Deserialize)]
    struct Chapter {
        start_time: Option<String>,
    }

    let root: Root = serde_json::from_str(json)?;
    let mut info = ProbeInfo::default();
    if let Some(f) = root.format {
        info.duration_ms = f
            .duration
            .and_then(|d| d.parse::<f64>().ok())
            .map(|d| (d * 1000.0).round() as u64)
            .unwrap_or(0);
        info.tags = f.tags;
    }
    let mut audio_n = 0;
    for s in root.streams {
        match s.codec_type.as_deref() {
            Some("video")
                if info.width.is_none() && s.disposition.get("attached_pic") != Some(&1) =>
            {
                info.width = s.width;
                info.height = s.height;
                info.video_codec = s.codec_name;
                info.fps = s.avg_frame_rate.as_deref().and_then(|r| {
                    let (n, d) = r.split_once('/')?;
                    let (n, d) = (n.parse::<f64>().ok()?, d.parse::<f64>().ok()?);
                    (d > 0.0 && n > 0.0).then(|| n / d)
                });
            }
            Some("audio") => {
                audio_n += 1;
                let tag = |name: &str| {
                    s.tags
                        .iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case(name))
                        .map(|(_, v)| v.clone())
                };
                let title = tag("title")
                    .or_else(|| {
                        tag("handler_name").filter(|h| h != "SoundHandler" && !h.is_empty())
                    })
                    .unwrap_or_else(|| format!("Track {audio_n}"));
                info.audio_tracks.push(title);
            }
            _ => {}
        }
    }
    info.chapters_ms = root
        .chapters
        .iter()
        .filter_map(|c| c.start_time.as_ref()?.parse::<f64>().ok())
        .map(|s| (s * 1000.0).round() as u64)
        .collect();
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> PipelineSpec {
        PipelineSpec {
            video: VideoSource::Ddagrab {
                adapter_idx: None,
                output_idx: 0,
                draw_mouse: false,
                region: None,
            },
            source_size: (2560, 1440),
            fps: FrameRate(60),
            resolution: Resolution::Native,
            backend: Backend::Nvenc,
            codec: Codec::H264,
            rate: RateControl::Cbr {
                bitrate_kbps: 20_000,
            },
            zero_copy: true,
            gpu_scaler: None,
            has_scale_d3d11: false,
            focus_screen: None,
            audio: vec![
                AudioInput {
                    url: r"\\.\pipe\gc-1-0".into(),
                    in_mix: true,
                    denoise: None,
                },
                AudioInput {
                    url: r"\\.\pipe\gc-1-1".into(),
                    in_mix: true,
                    denoise: Some(12),
                },
                AudioInput {
                    url: r"\\.\pipe\gc-1-2".into(),
                    in_mix: false,
                    denoise: None,
                },
            ],
            mixdown: true,
            audio_bitrate_kbps: 160,
            output: Output::StdoutTs,
        }
    }

    fn arg_after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
        args.iter()
            .position(|a| a == flag)
            .map(|i| args[i + 1].as_str())
    }

    #[test]
    fn nvenc_native_is_zero_copy_passthrough() {
        let s = spec();
        assert!(s.is_zero_copy());
        let a = s.build_args();
        assert_eq!(arg_after(&a, "-i"), Some(r"\\.\pipe\gc-1-0"));
        assert!(!a.iter().any(|x| x == "-init_hw_device"));
        let fc = arg_after(&a, "-filter_complex").unwrap();
        assert!(
            fc.starts_with("ddagrab=output_idx=0:framerate=60:draw_mouse=0[v];"),
            "no video filter expected: {fc}"
        );
        assert!(fc.contains("[1:a]afftdn=nr=12:nf=-40[dn1]"));
        assert!(fc.contains("[mx0][mx1]amix=inputs=2"));
        let maps: Vec<_> = a
            .iter()
            .enumerate()
            .filter(|(_, x)| *x == "-map")
            .map(|(i, _)| a[i + 1].as_str())
            .collect();
        assert_eq!(maps, ["[v]", "[mix]", "[tr0]", "[tr1]", "2:a"]);
        assert_eq!(arg_after(&a, "-c:v"), Some("h264_nvenc"));
        assert_eq!(arg_after(&a, "-rc"), Some("cbr"));
        assert_eq!(arg_after(&a, "-g"), Some("60"));
        assert_eq!(a.last().unwrap(), "pipe:1");
    }

    #[test]
    fn scaling_uses_gpu_scaler_when_available() {
        let mut s = spec();
        s.resolution = Resolution::P1080;
        assert!(!s.is_zero_copy(), "no scaler → falls back to download path");
        let a = s.build_args();
        let fc = arg_after(&a, "-filter_complex").unwrap();
        assert!(fc.starts_with(
            "ddagrab=output_idx=0:framerate=60:draw_mouse=0,hwdownload,format=bgra,scale=1920:1080:flags=bicubic,format=nv12[v]"
        ));

        s.gpu_scaler = Some(GpuScaler::D3d11);
        assert!(s.is_zero_copy());
        let fc = arg_after(&s.build_args(), "-filter_complex")
            .unwrap()
            .to_string();
        assert!(fc.contains(",scale_d3d11=width=1920:height=1080:format=nv12[v]"));

        s.backend = Backend::Amf;
        s.gpu_scaler = Some(GpuScaler::Amf);
        let fc = arg_after(&s.build_args(), "-filter_complex")
            .unwrap()
            .to_string();
        assert!(fc.contains(",vpp_amf=w=1920:h=1080:format=nv12[v]"));

        s.backend = Backend::Qsv;
        s.gpu_scaler = Some(GpuScaler::Qsv);
        let fc = arg_after(&s.build_args(), "-filter_complex")
            .unwrap()
            .to_string();
        assert!(fc.contains(",hwmap=derive_device=qsv,format=qsv,scale_qsv=w=1920:h=1080"));
    }

    #[test]
    fn software_and_uncapped() {
        let mut s = spec();
        s.backend = Backend::Software;
        s.fps = FrameRate(0);
        s.rate = RateControl::Quality { level: 20 };
        s.audio.clear();
        let a = s.build_args();
        assert_eq!(
            arg_after(&a, "-filter_complex"),
            Some("ddagrab=output_idx=0:framerate=500:draw_mouse=0:dup_frames=0,hwdownload,format=bgra,format=yuv420p[v]")
        );
        assert_eq!(arg_after(&a, "-c:v"), Some("libx264"));
        assert_eq!(arg_after(&a, "-crf"), Some("20"));
        assert_eq!(arg_after(&a, "-fps_mode"), Some("passthrough"));
        assert!(!a.iter().any(|x| x == "-c:a"));
    }

    #[test]
    fn single_mix_member_uses_anull() {
        let mut s = spec();
        s.audio.truncate(1);
        let fc = arg_after(&s.build_args(), "-filter_complex")
            .unwrap()
            .to_string();
        assert!(fc.contains("[mx0]anull[mix]"));
    }

    #[test]
    fn region_capture() {
        let mut s = spec();
        s.video = VideoSource::Ddagrab {
            adapter_idx: Some(1),
            output_idx: 1,
            draw_mouse: true,
            region: Some((100, 50, 1280, 720)),
        };
        let a = s.build_args();
        assert_eq!(arg_after(&a, "-init_hw_device"), Some("d3d11va=gc:1"));
        assert_eq!(arg_after(&a, "-filter_hw_device"), Some("gc"));
        assert!(arg_after(&a, "-filter_complex").unwrap().starts_with(
            "ddagrab=output_idx=1:framerate=60:draw_mouse=1:video_size=1280x720:offset_x=100:offset_y=50[v]"
        ));
    }

    #[test]
    fn window_capture_with_focus_screen_zero_copy() {
        let mut s = spec();
        s.video = VideoSource::Window {
            hwnd: 0x1234,
            draw_mouse: false,
            adapter_idx: None,
        };
        s.source_size = (1920, 1080);
        s.focus_screen = Some(FocusScreen {
            font_file: Some(r"C:\Windows\Fonts\segoeui.ttf".into()),
        });
        // Without scale_d3d11 the two frame sources cannot share the GPU path.
        assert!(!s.is_zero_copy());
        s.has_scale_d3d11 = true;
        assert!(s.is_zero_copy());
        let a = s.build_args();
        assert_eq!(arg_after(&a, "-init_hw_device"), Some("d3d11va=gc"));
        let fc = arg_after(&a, "-filter_complex").unwrap();
        assert!(fc.starts_with(
            "gfxcapture=hwnd=4660:max_framerate=60:capture_cursor=0:width=1920:height=1080:resize_mode=scale_aspect:dup_frames=1[cap];"
        ), "{fc}");
        assert!(fc.contains("color=c=0x0b0d12:s=1920x1080:r=1,drawtext=fontfile='C\\:/Windows/Fonts/segoeui.ttf'"), "{fc}");
        assert!(fc.contains(",format=bgra,hwupload[fs];"), "{fc}");
        assert!(fc.contains("[cap][fs]streamselect=inputs=2:map=0,fps=60,scale_d3d11=width=1920:height=1080:format=nv12[v]"), "{fc}");
        assert_eq!(focus_command(true), "cstreamselect -1 map 1\n");
    }

    #[test]
    fn focus_screen_download_path() {
        let mut s = spec();
        s.backend = Backend::Software;
        s.resolution = Resolution::P1080;
        s.focus_screen = Some(FocusScreen { font_file: None });
        let a = s.build_args();
        assert!(!a.iter().any(|x| x == "-init_hw_device"));
        let fc = arg_after(&a, "-filter_complex").unwrap();
        assert!(fc.starts_with(
            "ddagrab=output_idx=0:framerate=60:draw_mouse=0,hwdownload,format=bgra[cap];color=c=0x0b0d12:s=2560x1440:r=1,format=bgra[fs];[cap][fs]streamselect=inputs=2:map=0,fps=60,scale=1920:1080:flags=bicubic,format=yuv420p[v]"
        ), "{fc}");
    }

    #[test]
    fn export_trim_copy_and_remix() {
        let mut spec = ExportSpec {
            start_ms: 2_500,
            end_ms: 10_000,
            audio_streams: 4,
            audio_bitrate_kbps: 160,
            ..Default::default()
        };
        let a = export_args(Path::new("in.mp4"), Path::new("out.mp4"), &spec);
        assert_eq!(arg_after(&a, "-ss"), Some("2.500"));
        assert_eq!(arg_after(&a, "-t"), Some("7.500"));
        assert!(a.windows(2).any(|w| w == ["-c", "copy"]));
        assert_eq!(arg_after(&a, "-map_chapters"), Some("-1"));

        spec.rebuild_master = true;
        spec.gains = Some(vec![TrackGain {
            index: 2,
            volume: 1.0,
            muted: true,
        }]);
        let a = export_args(Path::new("in.mp4"), Path::new("out.mkv"), &spec);
        let fc = arg_after(&a, "-filter_complex").unwrap();
        assert!(fc.contains("[0:a:1]volume=1.000,asplit=2[t1][m1]"));
        assert!(fc.contains("[0:a:2]volume=0.000,asplit=2[t2][m2]"));
        assert!(fc.contains("[m1][m2][m3]amix=inputs=3"));
        let maps: Vec<_> = a
            .windows(2)
            .filter(|w| w[0] == "-map")
            .map(|w| w[1].as_str())
            .collect();
        assert_eq!(maps, ["0:v?", "[mix]", "[t1]", "[t2]", "[t3]"]);
        assert!(a.windows(2).any(|w| w == ["-c:v", "copy"]));
        assert!(a.windows(2).any(|w| w == ["-f", "matroska"]));

        let x = extract_tracks_args(
            Path::new("in.mkv"),
            &[(1, "a1.m4a".into()), (2, "a2.m4a".into())],
            true,
        );
        let maps: Vec<_> = x
            .windows(2)
            .filter(|w| w[0] == "-map")
            .map(|w| w[1].as_str())
            .collect();
        assert_eq!(maps, ["0:a:1", "0:a:2"]);
    }

    #[test]
    fn remux_mp4_and_mkv() {
        let spec = RemuxSpec {
            tags: vec![("game_title".into(), "Fortnite".into())],
            audio_titles: vec!["Master".into(), "Game".into()],
            chapters_file: Some("ch.txt".into()),
        };
        let a = remux_args("pipe:0", Path::new("out.mp4"), &spec);
        assert_eq!(
            arg_after(&a, "-movflags"),
            Some("+faststart+use_metadata_tags")
        );
        assert!(a.windows(2).any(|w| w == ["-metadata:s:a:1", "title=Game"]));
        assert!(a
            .windows(2)
            .any(|w| w == ["-metadata:s:a:1", "handler_name=Game"]));
        assert_eq!(arg_after(&a, "-map_chapters"), Some("1"));
        let a = remux_args("in.ts", Path::new("out.MKV"), &RemuxSpec::default());
        assert!(a.windows(2).any(|w| w == ["-f", "matroska"]));
        assert!(!a.iter().any(|x| x == "-map_chapters"));
    }

    #[test]
    fn parses_encoder_and_filter_lists() {
        let enc = "Encoders:\n V..... = Video\n ------\n V....D h264_nvenc           NVIDIA NVENC H.264 encoder (codec h264)\n A....D aac                  AAC (Advanced Audio Coding)\n";
        assert_eq!(parse_encoder_list(enc), ["h264_nvenc", "aac"]);
        let flt = "Filters:\n  T.. = Timeline support\n ... abench            A->A       Benchmark part of a filtergraph.\n ... scale_d3d11       V->V       Scale video using Direct3D11\n";
        assert_eq!(parse_filter_list(flt), ["abench", "scale_d3d11"]);
    }

    #[test]
    fn parses_probe_json() {
        let json = r#"{
          "streams": [
            {"codec_type":"video","codec_name":"h264","width":1920,"height":1080,"avg_frame_rate":"60000/1001"},
            {"codec_type":"audio","codec_name":"aac","tags":{"title":"Master"}},
            {"codec_type":"audio","codec_name":"aac"}
          ],
          "chapters": [{"start_time":"12.000000"}],
          "format": {"duration":"30.016000","tags":{"GAME_TITLE":"Fortnite"}}
        }"#;
        let p = parse_probe(json).unwrap();
        assert_eq!(p.duration_ms, 30_016);
        assert_eq!((p.width, p.height), (Some(1920), Some(1080)));
        assert!((p.fps.unwrap() - 59.94).abs() < 0.01);
        assert_eq!(p.audio_tracks, ["Master", "Track 2"]);
        assert_eq!(p.chapters_ms, [12_000]);
        assert_eq!(p.tags["GAME_TITLE"], "Fortnite");
    }
}

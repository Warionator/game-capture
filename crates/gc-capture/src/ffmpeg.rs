//! Locating the bundled FFmpeg binaries, probing build capabilities and
//! selecting a working hardware encoder at runtime.

use gc_core::config::{Codec, EncoderPreference};
use gc_core::ffmpeg::{
    backend_candidates, encoder_name, encoder_probe_args, parse_encoder_list, parse_filter_list,
    remux_args, Backend, FfmpegCaps, RemuxSpec,
};
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

#[derive(Debug, Clone)]
pub struct Ffmpeg {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

impl Ffmpeg {
    /// Looks for `ffmpeg(.exe)`/`ffprobe(.exe)` in each directory of
    /// `search`, then on `PATH`.
    pub fn locate(search: &[PathBuf]) -> anyhow::Result<Self> {
        let exe = |name: &str| format!("{name}{}", std::env::consts::EXE_SUFFIX);
        for dir in search {
            let (f, p) = (dir.join(exe("ffmpeg")), dir.join(exe("ffprobe")));
            if f.is_file() && p.is_file() {
                return Ok(Self {
                    ffmpeg: f,
                    ffprobe: p,
                });
            }
        }
        let on_path = |name: &str| -> Option<PathBuf> {
            let path = std::env::var_os("PATH")?;
            std::env::split_paths(&path)
                .map(|d| d.join(exe(name)))
                .find(|p| p.is_file())
        };
        match (on_path("ffmpeg"), on_path("ffprobe")) {
            (Some(ffmpeg), Some(ffprobe)) => Ok(Self { ffmpeg, ffprobe }),
            _ => anyhow::bail!(
                "FFmpeg not found (looked in {:?} and PATH). Reinstall game-capture or put ffmpeg on PATH.",
                search
            ),
        }
    }

    pub fn command(&self) -> Command {
        hidden(&self.ffmpeg)
    }

    pub fn caps(&self) -> FfmpegCaps {
        let run = |args: &[&str]| {
            self.command()
                .args(args)
                .stderr(Stdio::null())
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                .unwrap_or_default()
        };
        FfmpegCaps {
            encoders: parse_encoder_list(&run(&["-hide_banner", "-encoders"])),
            filters: parse_filter_list(&run(&["-hide_banner", "-filters"])),
        }
    }

    /// Spawns a stream-copy remux reading MPEG-TS on stdin.
    pub fn spawn_remux(&self, output: &Path, spec: &RemuxSpec) -> anyhow::Result<Child> {
        Ok(self
            .command()
            .args(remux_args("pipe:0", output, spec))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?)
    }

    /// Remuxes an MPEG-TS file into `output`.
    pub fn remux_file(&self, input: &Path, output: &Path, spec: &RemuxSpec) -> anyhow::Result<()> {
        let out = self
            .command()
            .args(remux_args(&input.to_string_lossy(), output, spec))
            .stdout(Stdio::null())
            .output()?;
        anyhow::ensure!(
            out.status.success(),
            "remux failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
        Ok(())
    }

    /// Writes `data` into a remux child and waits for it.
    pub fn finish_remux(
        mut child: Child,
        write: impl FnOnce(&mut dyn Write) -> std::io::Result<()>,
    ) -> anyhow::Result<()> {
        {
            let mut stdin = child.stdin.take().expect("piped stdin");
            if let Err(e) = write(&mut stdin) {
                let _ = child.kill();
                return Err(e.into());
            }
        }
        let out = child.wait_with_output()?;
        anyhow::ensure!(
            out.status.success(),
            "remux failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
        Ok(())
    }
}

/// A `Command` that never flashes a console window on Windows.
pub fn hidden(program: &Path) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.stdin(Stdio::null());
    cmd
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EncoderInfo {
    pub backend: Backend,
    pub codec: Codec,
    pub name: String,
    pub label: String,
    /// Present in the FFmpeg build.
    pub compiled: bool,
    /// Successfully encoded test frames on this machine.
    pub working: bool,
}

/// Tests every backend/codec pair on this machine. Each probe encodes three
/// synthetic frames, which fails fast when the GPU/driver lacks support.
pub fn probe_encoders(ff: &Ffmpeg, caps: &FfmpegCaps) -> Vec<EncoderInfo> {
    let pairs: Vec<(Backend, Codec)> = Backend::ALL
        .iter()
        .flat_map(|b| [Codec::H264, Codec::Hevc, Codec::Av1].map(|c| (*b, c)))
        .collect();
    // Probes run in parallel; each takes a fraction of a second.
    std::thread::scope(|scope| {
        let handles: Vec<_> = pairs
            .iter()
            .map(|&(backend, codec)| {
                scope.spawn(move || {
                    let name = encoder_name(backend, codec);
                    let compiled = caps.has_encoder(name);
                    let working = compiled
                        && ff
                            .command()
                            .args(encoder_probe_args(name))
                            .stdout(Stdio::null())
                            .stderr(Stdio::null())
                            .status()
                            .is_ok_and(|s| s.success());
                    EncoderInfo {
                        backend,
                        codec,
                        name: name.to_string(),
                        label: format!("{} {}", backend.label(), codec_label(codec)),
                        compiled,
                        working,
                    }
                })
            })
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    })
}

pub fn codec_label(c: Codec) -> &'static str {
    match c {
        Codec::H264 => "H.264",
        Codec::Hevc => "HEVC",
        Codec::Av1 => "AV1",
    }
}

/// Ordered list of backends to attempt for a preference and codec, using
/// probe results. If the codec is unsupported everywhere, H.264 software
/// is the last resort.
pub fn choose_backends(
    pref: EncoderPreference,
    codec: Codec,
    probed: &[EncoderInfo],
) -> Vec<(Backend, Codec)> {
    let ok = |b: Backend, c: Codec| {
        probed
            .iter()
            .any(|e| e.backend == b && e.codec == c && e.working)
    };
    let mut v: Vec<(Backend, Codec)> = backend_candidates(pref)
        .into_iter()
        .filter(|b| ok(*b, codec))
        .map(|b| (b, codec))
        .collect();
    if v.is_empty() && probed.is_empty() {
        // Not probed yet (or probing failed): optimistic order.
        v = backend_candidates(pref)
            .into_iter()
            .map(|b| (b, codec))
            .collect();
    }
    if !v.contains(&(Backend::Software, Codec::H264)) {
        v.push((Backend::Software, Codec::H264));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(b: Backend, c: Codec, working: bool) -> EncoderInfo {
        EncoderInfo {
            backend: b,
            codec: c,
            name: encoder_name(b, c).into(),
            label: String::new(),
            compiled: true,
            working,
        }
    }

    #[test]
    fn chooses_working_backends_in_order() {
        let probed = vec![
            info(Backend::Nvenc, Codec::Hevc, false),
            info(Backend::Amf, Codec::Hevc, true),
            info(Backend::Software, Codec::Hevc, true),
            info(Backend::Software, Codec::H264, true),
        ];
        assert_eq!(
            choose_backends(EncoderPreference::Auto, Codec::Hevc, &probed),
            [
                (Backend::Amf, Codec::Hevc),
                (Backend::Software, Codec::Hevc),
                (Backend::Software, Codec::H264)
            ]
        );
        assert_eq!(
            choose_backends(EncoderPreference::Nvenc, Codec::Av1, &probed),
            [(Backend::Software, Codec::H264)]
        );
        assert_eq!(
            choose_backends(EncoderPreference::Qsv, Codec::H264, &[]).len(),
            2
        );
    }
}

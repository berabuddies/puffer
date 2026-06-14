use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// Controls deterministic generated-video poster extraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VideoPosterOptions {
    pub(crate) long_edge_px: u32,
    pub(crate) quality: u8,
    pub(crate) timeout: Duration,
}

impl Default for VideoPosterOptions {
    fn default() -> Self {
        Self {
            long_edge_px: 480,
            quality: 3,
            timeout: Duration::from_secs(10),
        }
    }
}

/// Describes one poster extraction request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VideoPosterExtractionRequest {
    input_path: PathBuf,
    output_path: PathBuf,
    options: VideoPosterOptions,
}

impl VideoPosterExtractionRequest {
    /// Creates a poster extraction request.
    pub(crate) fn new(
        input_path: impl AsRef<Path>,
        output_path: impl AsRef<Path>,
        options: VideoPosterOptions,
    ) -> Self {
        Self {
            input_path: input_path.as_ref().to_path_buf(),
            output_path: output_path.as_ref().to_path_buf(),
            options,
        }
    }
}

/// Describes the poster extraction result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum VideoPosterExtraction {
    Available { byte_count: u64 },
    Missing { reason: String },
}

/// Describes a completed ffmpeg command run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum VideoPosterCommandResult {
    Succeeded,
    Failed(String),
    TimedOut,
}

/// Carries the deterministic ffmpeg command used for poster extraction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VideoPosterCommand {
    pub(crate) program: String,
    args: Vec<OsString>,
    pub(crate) timeout: Duration,
}

impl VideoPosterCommand {
    /// Returns command arguments as UTF-8 strings for test assertions.
    #[cfg(test)]
    pub(crate) fn args_as_strings(&self) -> Vec<String> {
        self.args
            .iter()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect()
    }
}

/// Extracts a poster image using the production ffmpeg runner.
pub(crate) fn extract_video_poster(request: VideoPosterExtractionRequest) -> VideoPosterExtraction {
    extract_video_poster_with_runner(request, run_ffmpeg_poster_command)
}

/// Extracts a poster image using an injected command runner.
pub(crate) fn extract_video_poster_with_runner(
    request: VideoPosterExtractionRequest,
    runner: impl FnOnce(&VideoPosterCommand) -> VideoPosterCommandResult,
) -> VideoPosterExtraction {
    if let Some(parent) = request.output_path.parent() {
        if let Err(error) = fs::create_dir_all(parent) {
            return VideoPosterExtraction::Missing {
                reason: format!("create poster output directory: {error}"),
            };
        }
    }
    let command = poster_command(&request);
    match runner(&command) {
        VideoPosterCommandResult::Succeeded => validate_poster_output(&request.output_path),
        VideoPosterCommandResult::Failed(reason) => VideoPosterExtraction::Missing { reason },
        VideoPosterCommandResult::TimedOut => VideoPosterExtraction::Missing {
            reason: format!(
                "ffmpeg timed out after {}s",
                request.options.timeout.as_secs()
            ),
        },
    }
}

fn poster_command(request: &VideoPosterExtractionRequest) -> VideoPosterCommand {
    let filter = format!(
        "scale=w='if(gte(iw,ih),{},-2)':h='if(gte(iw,ih),-2,{})'",
        request.options.long_edge_px, request.options.long_edge_px
    );
    VideoPosterCommand {
        program: "ffmpeg".to_string(),
        args: vec![
            OsString::from("-hide_banner"),
            OsString::from("-loglevel"),
            OsString::from("error"),
            OsString::from("-y"),
            OsString::from("-i"),
            request.input_path.as_os_str().to_os_string(),
            OsString::from("-map"),
            OsString::from("0:v:0"),
            OsString::from("-frames:v"),
            OsString::from("1"),
            OsString::from("-vf"),
            OsString::from(filter),
            OsString::from("-q:v"),
            OsString::from(request.options.quality.to_string()),
            request.output_path.as_os_str().to_os_string(),
        ],
        timeout: request.options.timeout,
    }
}

fn run_ffmpeg_poster_command(command: &VideoPosterCommand) -> VideoPosterCommandResult {
    let mut child = match Command::new(&command.program)
        .args(&command.args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            return VideoPosterCommandResult::Failed(format!("spawn ffmpeg: {error}"));
        }
    };
    let started_at = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                return VideoPosterCommandResult::Succeeded;
            }
            Ok(Some(status)) => {
                return VideoPosterCommandResult::Failed(format!("ffmpeg exited with {status}"));
            }
            Ok(None) => {}
            Err(error) => {
                return VideoPosterCommandResult::Failed(format!("wait for ffmpeg: {error}"));
            }
        }
        if started_at.elapsed() >= command.timeout {
            let _ = child.kill();
            let _ = child.wait();
            return VideoPosterCommandResult::TimedOut;
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn validate_poster_output(path: &Path) -> VideoPosterExtraction {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => {
            return VideoPosterExtraction::Missing {
                reason: format!("poster output missing: {error}"),
            };
        }
    };
    if !metadata.is_file() {
        return VideoPosterExtraction::Missing {
            reason: "poster output is not a file".to_string(),
        };
    }
    if metadata.len() == 0 {
        return VideoPosterExtraction::Missing {
            reason: "poster output is empty".to_string(),
        };
    }
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) => {
            return VideoPosterExtraction::Missing {
                reason: format!("read poster output: {error}"),
            };
        }
    };
    let mut buffer = [0_u8; 3];
    let count = match file.read(&mut buffer) {
        Ok(count) => count,
        Err(error) => {
            return VideoPosterExtraction::Missing {
                reason: format!("read poster magic bytes: {error}"),
            };
        }
    };
    if count < 3 || buffer != [0xff, 0xd8, 0xff] {
        return VideoPosterExtraction::Missing {
            reason: "poster output is not a JPEG".to_string(),
        };
    }
    VideoPosterExtraction::Available {
        byte_count: metadata.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const JPEG_BYTES: &[u8] = &[0xff, 0xd8, 0xff, 0xd9];

    #[test]
    fn video_poster_extractor_builds_deterministic_ffmpeg_command() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("generated.mp4");
        let output = temp.path().join("poster.jpg");
        std::fs::write(&input, b"mp4-bytes").unwrap();

        let result = extract_video_poster_with_runner(
            VideoPosterExtractionRequest::new(&input, &output, VideoPosterOptions::default()),
            |command| {
                assert_eq!(command.program, "ffmpeg");
                assert_eq!(command.timeout, Duration::from_secs(10));
                assert_eq!(
                    command.args_as_strings(),
                    vec![
                        "-hide_banner",
                        "-loglevel",
                        "error",
                        "-y",
                        "-i",
                        input.to_str().unwrap(),
                        "-map",
                        "0:v:0",
                        "-frames:v",
                        "1",
                        "-vf",
                        "scale=w='if(gte(iw,ih),480,-2)':h='if(gte(iw,ih),-2,480)'",
                        "-q:v",
                        "3",
                        output.to_str().unwrap(),
                    ]
                );
                std::fs::write(&output, JPEG_BYTES).unwrap();
                VideoPosterCommandResult::Succeeded
            },
        );

        assert_eq!(result, VideoPosterExtraction::Available { byte_count: 4 });
    }

    #[test]
    fn video_poster_extractor_records_command_failure_as_missing() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("generated.mp4");
        let output = temp.path().join("poster.jpg");
        std::fs::write(&input, b"mp4-bytes").unwrap();

        let result = extract_video_poster_with_runner(
            VideoPosterExtractionRequest::new(&input, &output, VideoPosterOptions::default()),
            |_| VideoPosterCommandResult::Failed("ffmpeg exited with status 1".to_string()),
        );

        assert!(matches!(
            result,
            VideoPosterExtraction::Missing { reason } if reason.contains("status 1")
        ));
    }

    #[test]
    fn video_poster_extractor_records_timeout_as_missing() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("generated.mp4");
        let output = temp.path().join("poster.jpg");
        std::fs::write(&input, b"mp4-bytes").unwrap();

        let result = extract_video_poster_with_runner(
            VideoPosterExtractionRequest::new(&input, &output, VideoPosterOptions::default()),
            |_| VideoPosterCommandResult::TimedOut,
        );

        assert!(matches!(
            result,
            VideoPosterExtraction::Missing { reason } if reason.contains("timed out")
        ));
    }

    #[test]
    fn video_poster_extractor_rejects_invalid_output() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("generated.mp4");
        let output = temp.path().join("poster.jpg");
        std::fs::write(&input, b"mp4-bytes").unwrap();

        let result = extract_video_poster_with_runner(
            VideoPosterExtractionRequest::new(&input, &output, VideoPosterOptions::default()),
            |_| {
                std::fs::write(&output, b"not-jpeg").unwrap();
                VideoPosterCommandResult::Succeeded
            },
        );

        assert!(matches!(
            result,
            VideoPosterExtraction::Missing { reason } if reason.contains("JPEG")
        ));
    }
}

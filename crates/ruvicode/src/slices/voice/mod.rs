use std::path::Path;

use async_trait::async_trait;
use ruvi_core::{
    CoreError, CoreResult, LocalRuntimeManager, RuntimeKind, RuntimeStatus, SpeechTranscriber,
};
use tokio::process::Command;

use crate::slices::local_llm::{find_executable, install_via_brew_or_hint, probe_version};

/// Local Whisper speech-to-text adapter wrapping `whisper-cli` or `whisper`.
pub struct WhisperCliTranscriber;

impl WhisperCliTranscriber {
    fn detect_binary() -> Option<&'static str> {
        if find_executable("whisper-cli").is_some() {
            Some("whisper-cli")
        } else if find_executable("whisper").is_some() {
            Some("whisper")
        } else {
            None
        }
    }
}

#[async_trait]
impl SpeechTranscriber for WhisperCliTranscriber {
    fn id(&self) -> &'static str {
        "whisper-cpp"
    }

    fn display_name(&self) -> &'static str {
        "Local Whisper (STT)"
    }

    async fn is_available(&self) -> bool {
        Self::detect_binary().is_some()
    }

    async fn transcribe_file(&self, audio_path: &Path) -> CoreResult<String> {
        if !audio_path.is_file() {
            return Err(CoreError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("audio file not found: {}", audio_path.display()),
            )));
        }

        let Some(binary) = Self::detect_binary() else {
            return Err(CoreError::Execution {
                component: self.id().to_string(),
                message: format!(
                    "no local whisper binary found; install via `{}`",
                    self.install_instructions()
                ),
            });
        };

        let path_str = audio_path.to_string_lossy().to_string();
        let mut cmd = Command::new(binary);
        if binary == "whisper-cli" {
            cmd.args(["-f", &path_str, "-nt"]);
        } else {
            cmd.args([&path_str, "--output_format", "txt"]);
        }

        let output = cmd.output().await.map_err(|err| CoreError::Execution {
            component: self.id().to_string(),
            message: err.to_string(),
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(CoreError::Execution {
                component: self.id().to_string(),
                message: format!("{binary} failed: {}", stderr.trim()),
            });
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn install_instructions(&self) -> &'static str {
        "brew install whisper-cpp"
    }
}

/// Cross-platform CLI microphone recorder (`rec` / `sox` / `ffmpeg`) producing 16 kHz mono WAV files.
#[derive(Debug, Default, Clone, Copy)]
pub struct MicrophoneRecorder;

impl MicrophoneRecorder {
    fn detect_binary() -> Option<&'static str> {
        if find_executable("rec").is_some() {
            Some("rec")
        } else if find_executable("sox").is_some() {
            Some("sox")
        } else if find_executable("ffmpeg").is_some() {
            Some("ffmpeg")
        } else {
            None
        }
    }

    /// Returns `true` when a supported CLI audio recorder (`rec`, `sox`, or `ffmpeg`) is on `PATH`.
    #[must_use]
    pub fn is_available(&self) -> bool {
        Self::detect_binary().is_some()
    }

    /// Platform-specific command hint for installing a supported microphone recorder.
    #[must_use]
    pub const fn install_instructions(&self) -> &'static str {
        if cfg!(target_os = "macos") {
            "brew install sox  # or: brew install ffmpeg"
        } else {
            "sudo apt install sox  # or: sudo apt install ffmpeg"
        }
    }

    /// Records `seconds` of 16 kHz mono 16-bit PCM audio into `output_path`.
    ///
    /// # Errors
    /// Returns [`CoreError::Config`] if `seconds` is `0`, or [`CoreError::Execution`] if no
    /// supported recorder binary is available or the recording process fails.
    pub async fn record_wav(&self, output_path: &Path, seconds: u8) -> CoreResult<()> {
        self.record_wav_with_binary(Self::detect_binary(), output_path, seconds)
            .await
    }

    async fn record_wav_with_binary(
        &self,
        binary: Option<&str>,
        output_path: &Path,
        seconds: u8,
    ) -> CoreResult<()> {
        if seconds == 0 {
            return Err(CoreError::Config(
                "recording duration must be at least 1 second".to_string(),
            ));
        }

        let Some(bin) = binary else {
            return Err(CoreError::Execution {
                component: "mic-recorder".to_string(),
                message: format!(
                    "no microphone recorder (`rec`, `sox`, or `ffmpeg`) found; install via `{}`",
                    self.install_instructions()
                ),
            });
        };

        let secs = seconds.to_string();
        let path_str = output_path.to_string_lossy().to_string();
        let mut cmd = Command::new(bin);
        match bin {
            "rec" => {
                cmd.args([
                    "-q", "-r", "16000", "-c", "1", "-b", "16", &path_str, "trim", "0", &secs,
                ]);
            }
            "sox" => {
                cmd.args([
                    "-q", "-d", "-r", "16000", "-c", "1", "-b", "16", &path_str, "trim", "0", &secs,
                ]);
            }
            _ => {
                let (input_fmt, input_dev) = if cfg!(target_os = "macos") {
                    ("avfoundation", ":0")
                } else {
                    ("pulse", "default")
                };
                cmd.args([
                    "-y",
                    "-f",
                    input_fmt,
                    "-i",
                    input_dev,
                    "-t",
                    &secs,
                    "-ar",
                    "16000",
                    "-ac",
                    "1",
                    "-sample_fmt",
                    "s16",
                    &path_str,
                ]);
            }
        }

        let output = cmd.output().await.map_err(|err| CoreError::Execution {
            component: "mic-recorder".to_string(),
            message: err.to_string(),
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(CoreError::Execution {
                component: "mic-recorder".to_string(),
                message: format!("{bin} failed: {}", stderr.trim()),
            });
        }

        Ok(())
    }
}

/// Exposes Whisper STT installation & status uniformly in the TUI Local Installer modal.
pub struct WhisperRuntimeManager;

#[async_trait]
impl LocalRuntimeManager for WhisperRuntimeManager {
    fn id(&self) -> &'static str {
        "whisper-cpp"
    }

    fn display_name(&self) -> &'static str {
        "Whisper STT"
    }

    async fn status(&self) -> CoreResult<RuntimeStatus> {
        let bin = WhisperCliTranscriber::detect_binary();
        let installed = bin.is_some();
        let version = if let Some(name) = bin {
            probe_version(name, &["--help"]).await
        } else {
            None
        };

        Ok(RuntimeStatus {
            id: self.id().to_string(),
            display_name: self.display_name().to_string(),
            kind: RuntimeKind::Custom("voice-stt".to_string()),
            installed,
            running: installed,
            version,
            models: Vec::new(),
            install_hint: "brew install whisper-cpp".to_string(),
        })
    }

    async fn install_runtime(&self) -> CoreResult<String> {
        install_via_brew_or_hint(
            self.id(),
            "whisper-cpp",
            "brew install whisper-cpp  # or build https://github.com/ggml-org/whisper.cpp",
        )
        .await
    }

    async fn pull_model(&self, model: &str) -> CoreResult<String> {
        Ok(format!(
            "Download GGML Whisper weights (e.g. `ggml-{model}.bin`) into `~/.cache/whisper/`."
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn whisper_errors_cleanly_on_missing_audio_file() {
        let transcriber = WhisperCliTranscriber;
        let err = transcriber
            .transcribe_file(Path::new("/tmp/ruvi-definitely-missing-audio.wav"))
            .await
            .expect_err("missing audio file must return Io error");
        assert!(matches!(err, CoreError::Io(_)));
    }

    #[tokio::test]
    async fn mic_recorder_rejects_zero_seconds() {
        let recorder = MicrophoneRecorder;
        let err = recorder
            .record_wav(Path::new("/tmp/ruvi-mic-zero.wav"), 0)
            .await
            .expect_err("zero seconds must return Config error");
        assert!(matches!(err, CoreError::Config(_)));
    }

    #[tokio::test]
    async fn mic_recorder_handles_missing_binary_gracefully() {
        let recorder = MicrophoneRecorder;
        let err = recorder
            .record_wav_with_binary(None, Path::new("/tmp/ruvi-mic-missing.wav"), 3)
            .await
            .expect_err("missing recorder binary must return Execution error");
        match err {
            CoreError::Execution { component, message } => {
                assert_eq!(component, "mic-recorder");
                assert!(message.contains(recorder.install_instructions()));
            }
            other => panic!("expected CoreError::Execution, got {other:?}"),
        }
    }

    #[test]
    fn mic_recorder_exposes_install_instructions() {
        let recorder = MicrophoneRecorder;
        assert_ne!(recorder.install_instructions(), "");
    }
}

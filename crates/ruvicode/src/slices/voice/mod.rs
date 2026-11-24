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
}

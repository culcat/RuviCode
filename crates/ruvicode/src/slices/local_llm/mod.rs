use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use ruvi_core::{CoreError, CoreResult, LocalRuntimeManager, RuntimeKind, RuntimeStatus};
use serde::Deserialize;
use tokio::process::Command;

/// Locates an executable binary on `PATH`.
#[must_use]
pub fn find_executable(binary: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(binary);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Runs `<binary> <args...>` and returns the first non-empty trimmed output line.
pub async fn probe_version(binary: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(binary).args(args).output().await.ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    stdout
        .lines()
        .chain(stderr.lines())
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.contains("could not connect"))
        .map(|line| line.strip_prefix("Warning: ").unwrap_or(line).to_string())
        .next()
}

/// Installs a Homebrew formula on macOS when `brew` is present, or returns platform instructions.
///
/// # Errors
/// Returns [`CoreError::Execution`] if `brew` execution fails.
pub async fn install_via_brew_or_hint(
    component: &str,
    formula: &str,
    fallback_hint: &str,
) -> CoreResult<String> {
    if cfg!(target_os = "macos") && find_executable("brew").is_some() {
        let output = Command::new("brew")
            .args(["install", formula])
            .env("HOMEBREW_NO_AUTO_UPDATE", "1")
            .output()
            .await
            .map_err(|err| CoreError::Execution {
                component: component.to_string(),
                message: err.to_string(),
            })?;

        if output.status.success() {
            return Ok(format!("Installed `{formula}` via Homebrew."));
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CoreError::Execution {
            component: component.to_string(),
            message: format!("brew install {formula} failed: {}", stderr.trim()),
        });
    }

    Ok(format!(
        "Automatic installer unavailable on this host. Run manually: {fallback_hint}"
    ))
}

/// Manages local `Ollama` installation, daemon detection, and model pulling.
pub struct OllamaRuntimeManager {
    endpoint: String,
    client: Client,
}

impl OllamaRuntimeManager {
    /// Creates an [`OllamaRuntimeManager`] targeting `http://127.0.0.1:11434` (or `OLLAMA_HOST`).
    #[must_use]
    pub fn new() -> Self {
        let endpoint = std::env::var("OLLAMA_HOST")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| "http://127.0.0.1:11434".to_string())
            .trim_end_matches('/')
            .to_string();
        let client = Client::builder()
            .timeout(Duration::from_millis(800))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self { endpoint, client }
    }
}

impl Default for OllamaRuntimeManager {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl LocalRuntimeManager for OllamaRuntimeManager {
    fn id(&self) -> &'static str {
        "ollama"
    }

    fn display_name(&self) -> &'static str {
        "Ollama"
    }

    async fn status(&self) -> CoreResult<RuntimeStatus> {
        let installed = find_executable("ollama").is_some();
        let version = if installed {
            probe_version("ollama", &["--version"]).await
        } else {
            None
        };

        let mut running = false;
        let mut models = Vec::new();
        if let Ok(resp) = self
            .client
            .get(format!("{}/api/tags", self.endpoint))
            .send()
            .await
            && resp.status().is_success()
        {
            running = true;
            if let Ok(parsed) = resp.json::<OllamaTags>().await {
                models = parsed.models.into_iter().map(|m| m.name).collect();
            }
        }

        Ok(RuntimeStatus {
            id: self.id().to_string(),
            display_name: self.display_name().to_string(),
            kind: RuntimeKind::Ollama,
            installed,
            running,
            version,
            models,
            install_hint: "brew install ollama  # or curl -fsSL https://ollama.com/install.sh | sh"
                .to_string(),
        })
    }

    async fn install_runtime(&self) -> CoreResult<String> {
        install_via_brew_or_hint(
            self.id(),
            "ollama",
            "curl -fsSL https://ollama.com/install.sh | sh",
        )
        .await
    }

    async fn pull_model(&self, model: &str) -> CoreResult<String> {
        if find_executable("ollama").is_none() {
            return Err(CoreError::Execution {
                component: self.id().to_string(),
                message: "ollama binary not installed; install Ollama first".to_string(),
            });
        }

        let output = Command::new("ollama")
            .args(["pull", model])
            .output()
            .await
            .map_err(|err| CoreError::Execution {
                component: self.id().to_string(),
                message: err.to_string(),
            })?;

        if output.status.success() {
            return Ok(format!("Pulled Ollama model `{model}`."));
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(CoreError::Execution {
            component: self.id().to_string(),
            message: format!("ollama pull {model} failed: {}", stderr.trim()),
        })
    }
}

/// Manages local `llama.cpp` (`llama-server` / `llama-cli`) installation and health checks.
pub struct LlamaCppRuntimeManager {
    endpoint: String,
    client: Client,
}

impl LlamaCppRuntimeManager {
    /// Creates a [`LlamaCppRuntimeManager`] targeting `http://127.0.0.1:8080`.
    #[must_use]
    pub fn new() -> Self {
        let endpoint = std::env::var("LLAMA_CPP_URL")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| "http://127.0.0.1:8080".to_string())
            .trim_end_matches("/v1")
            .trim_end_matches('/')
            .to_string();
        let client = Client::builder()
            .timeout(Duration::from_millis(500))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self { endpoint, client }
    }
}

impl Default for LlamaCppRuntimeManager {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl LocalRuntimeManager for LlamaCppRuntimeManager {
    fn id(&self) -> &'static str {
        "llama-cpp"
    }

    fn display_name(&self) -> &'static str {
        "llama.cpp"
    }

    async fn status(&self) -> CoreResult<RuntimeStatus> {
        let bin = find_executable("llama-server")
            .or_else(|| find_executable("llama-cli"))
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()));
        let installed = bin.is_some();
        let version = if let Some(bin_name) = bin {
            probe_version(&bin_name, &["--version"]).await
        } else {
            None
        };

        let running = self
            .client
            .get(format!("{}/health", self.endpoint))
            .send()
            .await
            .is_ok_and(|resp| resp.status().is_success());

        Ok(RuntimeStatus {
            id: self.id().to_string(),
            display_name: self.display_name().to_string(),
            kind: RuntimeKind::LlamaCpp,
            installed,
            running,
            version,
            models: Vec::new(),
            install_hint: "brew install llama.cpp".to_string(),
        })
    }

    async fn install_runtime(&self) -> CoreResult<String> {
        install_via_brew_or_hint(
            self.id(),
            "llama.cpp",
            "brew install llama.cpp  # or build https://github.com/ggml-org/llama.cpp",
        )
        .await
    }

    async fn pull_model(&self, model: &str) -> CoreResult<String> {
        if find_executable("llama-cli").is_none() {
            return Err(CoreError::Execution {
                component: self.id().to_string(),
                message: "llama-cli not installed; install llama.cpp first".to_string(),
            });
        }
        Ok(format!(
            "Run `llama-server -hf {model} --port 8080` to serve HuggingFace GGUF models with llama.cpp."
        ))
    }
}

#[derive(Deserialize)]
struct OllamaTags {
    models: Vec<OllamaModel>,
}

#[derive(Deserialize)]
struct OllamaModel {
    name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn runtime_managers_return_valid_status_without_hanging() {
        let ollama = OllamaRuntimeManager::new();
        let llama = LlamaCppRuntimeManager::new();

        let ollama_status = ollama.status().await.expect("status probe must not error");
        let llama_status = llama.status().await.expect("status probe must not error");

        assert_eq!(ollama_status.id, "ollama");
        assert_eq!(llama_status.id, "llama-cpp");
        assert_ne!(ollama_status.install_hint, "");
        assert_ne!(llama_status.install_hint, "");
    }
}

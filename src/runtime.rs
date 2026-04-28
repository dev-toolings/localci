use crate::ir::Job;
use crate::secrets::SecretStore;
use anyhow::{Context, Result};
use async_trait::async_trait;
use clap::ValueEnum;
use indexmap::IndexMap;
use serde::Serialize;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Instant;
use tokio::process::Command;

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum RuntimeKind {
    Docker,
    Shell,
}

#[derive(Debug, Clone)]
pub struct ShellRequest {
    pub job_id: String,
    pub step_name: String,
    pub image: Option<String>,
    pub workspace: PathBuf,
    pub working_directory: Option<String>,
    pub command: String,
    pub shell: Option<String>,
    pub env: IndexMap<String, String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommandOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u128,
}

#[async_trait]
pub trait RuntimeBackend: Send + Sync {
    async fn run_shell(
        &self,
        request: ShellRequest,
        secrets: &SecretStore,
    ) -> Result<CommandOutput>;
}

pub fn backend(kind: RuntimeKind) -> Box<dyn RuntimeBackend> {
    match kind {
        RuntimeKind::Docker => Box::new(DockerBackend),
        RuntimeKind::Shell => Box::new(ShellBackend),
    }
}

pub fn image_for_job(job: &Job) -> String {
    if let Some(container) = &job.container {
        return container.clone();
    }

    if job.runs_on.iter().any(|label| label.contains("alpine")) {
        return "alpine:3.20".to_string();
    }

    if job.runs_on.iter().any(|label| label.contains("debian")) {
        return "debian:bookworm".to_string();
    }

    if job
        .runs_on
        .iter()
        .any(|label| label.starts_with("ubuntu") || label == "linux")
    {
        return "ghcr.io/catthehacker/ubuntu:act-latest".to_string();
    }

    "ghcr.io/catthehacker/ubuntu:act-latest".to_string()
}

struct DockerBackend;

#[async_trait]
impl RuntimeBackend for DockerBackend {
    async fn run_shell(
        &self,
        request: ShellRequest,
        secrets: &SecretStore,
    ) -> Result<CommandOutput> {
        let image = request
            .image
            .clone()
            .unwrap_or_else(|| "ghcr.io/catthehacker/ubuntu:act-latest".to_string());
        let shell = request.shell.as_deref().unwrap_or("bash");
        let workdir = docker_workdir(&request.working_directory);
        let mut command = Command::new("docker");
        command
            .arg("run")
            .arg("--rm")
            .arg("-v")
            .arg(format!("{}:/workspace", request.workspace.display()))
            .arg("-w")
            .arg(workdir)
            .arg("-e")
            .arg("CI=true")
            .arg("-e")
            .arg("GITHUB_WORKSPACE=/workspace");

        for (key, value) in &request.env {
            command.arg("-e").arg(format!("{key}={value}"));
        }
        for (key, value) in secrets.env_pairs() {
            command.arg("-e").arg(format!("{key}={value}"));
        }

        command
            .arg(image)
            .arg(shell)
            .arg("-lc")
            .arg(request.command)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        run_command(command, secrets)
            .await
            .with_context(|| format!("docker step failed to start for job {}", request.job_id))
    }
}

struct ShellBackend;

#[async_trait]
impl RuntimeBackend for ShellBackend {
    async fn run_shell(
        &self,
        request: ShellRequest,
        secrets: &SecretStore,
    ) -> Result<CommandOutput> {
        let shell = request.shell.as_deref().unwrap_or("bash");
        let mut command = Command::new(shell);
        command.arg("-c").arg(request.command);

        let workdir = if let Some(working_directory) = &request.working_directory {
            request.workspace.join(working_directory)
        } else {
            request.workspace.clone()
        };

        command.current_dir(workdir);
        command.env("CI", "true");
        command.env("GITHUB_WORKSPACE", &request.workspace);
        for (key, value) in &request.env {
            command.env(key, value);
        }
        for (key, value) in secrets.env_pairs() {
            command.env(key, value);
        }

        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        run_command(command, secrets)
            .await
            .with_context(|| format!("shell step failed to start for job {}", request.job_id))
    }
}

async fn run_command(mut command: Command, secrets: &SecretStore) -> Result<CommandOutput> {
    let started = Instant::now();
    let output = command.output().await?;
    Ok(CommandOutput {
        exit_code: output.status.code().unwrap_or(1),
        stdout: secrets.mask(&String::from_utf8_lossy(&output.stdout)),
        stderr: secrets.mask(&String::from_utf8_lossy(&output.stderr)),
        duration_ms: started.elapsed().as_millis(),
    })
}

fn docker_workdir(working_directory: &Option<String>) -> String {
    match working_directory {
        Some(path) if path.starts_with('/') => path.clone(),
        Some(path) => format!("/workspace/{path}"),
        None => "/workspace".to_string(),
    }
}

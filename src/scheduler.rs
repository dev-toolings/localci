use crate::expr::{EvalContext, eval_condition, interpolate};
use crate::ir::{Job, Pipeline, Step, StepKind};
use crate::runtime::{RuntimeBackend, RuntimeKind, ShellRequest, backend, image_for_job};
use crate::secrets::SecretStore;
use crate::storage::Storage;
use anyhow::{Context, Result, bail};
use futures::stream::{FuturesUnordered, StreamExt};
use indexmap::IndexMap;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct RunOptions {
    pub job: Option<String>,
    pub from: Option<String>,
    pub parallelism: usize,
    pub runtime: RuntimeKind,
    pub strict_actions: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunSummary {
    pub run_id: String,
    pub success: bool,
    pub duration_ms: u128,
    pub jobs: Vec<JobOutcome>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum JobStatus {
    Success,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobOutcome {
    pub job_id: String,
    pub name: String,
    pub status: JobStatus,
    pub reason: Option<String>,
    pub duration_ms: u128,
    pub steps: Vec<StepOutcome>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum StepStatus {
    Success,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Serialize)]
pub struct StepOutcome {
    pub name: String,
    pub status: StepStatus,
    pub exit_code: Option<i32>,
    pub duration_ms: u128,
    pub reason: Option<String>,
}

#[derive(Debug, Clone)]
struct CacheRequest {
    key: String,
    path: String,
}

pub async fn run_pipeline(
    pipeline: &Pipeline,
    project: &Path,
    storage: Storage,
    secrets: SecretStore,
    options: RunOptions,
) -> Result<RunSummary> {
    let started = Instant::now();
    let selected = select_job_ids(pipeline, options.job.as_deref(), options.from.as_deref())?;
    let mut pending = selected.clone();
    let mut status_by_job = HashMap::new();
    let mut outcomes_by_job: HashMap<String, JobOutcome> = HashMap::new();
    let mut running = FuturesUnordered::new();
    let parallelism = options.parallelism.max(1);
    let runtime = Arc::from(backend(options.runtime));
    let secrets = Arc::new(secrets);
    let storage = Arc::new(storage);
    let project = project.to_path_buf();

    while !pending.is_empty() || !running.is_empty() {
        let skipped = jobs_blocked_by_failed_needs(pipeline, &pending, &selected, &status_by_job);
        let skipped_any = !skipped.is_empty();
        for job_id in skipped {
            pending.remove(&job_id);
            let job = pipeline.jobs.get(&job_id).expect("selected jobs are valid");
            let outcome = skipped_job(job, "dependency failed or skipped");
            status_by_job.insert(job_id.clone(), outcome.status.clone());
            outcomes_by_job.insert(job_id, outcome);
        }
        if skipped_any {
            continue;
        }

        while running.len() < parallelism {
            let Some(job_id) = next_ready_job(pipeline, &pending, &selected, &status_by_job) else {
                break;
            };
            pending.remove(&job_id);
            let job = pipeline
                .jobs
                .get(&job_id)
                .with_context(|| format!("selected job {job_id} does not exist"))?
                .clone();
            let runtime = Arc::clone(&runtime);
            let secrets = Arc::clone(&secrets);
            let storage = Arc::clone(&storage);
            let project = project.clone();
            let strict_actions = options.strict_actions;

            running.push(tokio::spawn(async move {
                execute_job(job, project, storage, secrets, runtime, strict_actions).await
            }));
        }

        if running.is_empty() {
            if pending.is_empty() {
                break;
            }
            let ids = pending.iter().cloned().collect::<Vec<_>>().join(", ");
            bail!("could not schedule remaining jobs, possible cycle: {ids}");
        }

        let outcome = running
            .next()
            .await
            .context("job worker disappeared")?
            .context("job worker panicked")??;
        status_by_job.insert(outcome.job_id.clone(), outcome.status.clone());
        outcomes_by_job.insert(outcome.job_id.clone(), outcome);
    }

    let mut jobs = Vec::new();
    for job_id in selected_in_pipeline_order(pipeline, &selected) {
        if let Some(outcome) = outcomes_by_job.remove(&job_id) {
            jobs.push(outcome);
        }
    }

    let success = jobs.iter().all(|job| job.status != JobStatus::Failed);
    Ok(RunSummary {
        run_id: storage.run_id.clone(),
        success,
        duration_ms: started.elapsed().as_millis(),
        jobs,
    })
}

pub fn select_job_ids(
    pipeline: &Pipeline,
    job: Option<&str>,
    from: Option<&str>,
) -> Result<HashSet<String>> {
    if job.is_some() && from.is_some() {
        bail!("--job and --from are mutually exclusive");
    }

    if let Some(selector) = job {
        let targets = resolve_selector(pipeline, selector)?;
        let mut selected = HashSet::new();
        for target in targets {
            include_ancestors(pipeline, &target, &mut selected)?;
        }
        return Ok(selected);
    }

    if let Some(selector) = from {
        let targets = resolve_selector(pipeline, selector)?;
        let mut selected = HashSet::new();
        let reverse = reverse_needs(pipeline);
        for target in targets {
            include_descendants(&target, &reverse, &mut selected);
        }
        return Ok(selected);
    }

    Ok(pipeline.jobs.keys().cloned().collect())
}

pub fn selected_in_pipeline_order(pipeline: &Pipeline, selected: &HashSet<String>) -> Vec<String> {
    pipeline
        .jobs
        .keys()
        .filter(|job_id| selected.contains(*job_id))
        .cloned()
        .collect()
}

pub fn topological_order(pipeline: &Pipeline, selected: &HashSet<String>) -> Result<Vec<String>> {
    let mut pending = selected.clone();
    let mut completed = HashSet::new();
    let mut output = Vec::new();

    while !pending.is_empty() {
        let ready = pending
            .iter()
            .find(|job_id| {
                let job = pipeline.jobs.get(*job_id).expect("selected jobs are valid");
                job.needs
                    .iter()
                    .filter(|need| selected.contains(*need))
                    .all(|need| completed.contains(need))
            })
            .cloned();

        let Some(job_id) = ready else {
            let ids = pending.iter().cloned().collect::<Vec<_>>().join(", ");
            bail!("cycle detected while ordering jobs: {ids}");
        };

        pending.remove(&job_id);
        completed.insert(job_id.clone());
        output.push(job_id);
    }

    Ok(output)
}

fn resolve_selector(pipeline: &Pipeline, selector: &str) -> Result<Vec<String>> {
    if pipeline.jobs.contains_key(selector) {
        return Ok(vec![selector.to_string()]);
    }

    let matches = pipeline
        .jobs
        .values()
        .filter(|job| {
            job.source_id == selector
                || job.source_id.ends_with(&format!(":{selector}"))
                || job.id.ends_with(&format!(":{selector}"))
                || job.name.as_deref() == Some(selector)
        })
        .map(|job| job.id.clone())
        .collect::<Vec<_>>();

    if matches.is_empty() {
        bail!("no job matches selector {selector}");
    }

    Ok(matches)
}

fn include_ancestors(
    pipeline: &Pipeline,
    job_id: &str,
    selected: &mut HashSet<String>,
) -> Result<()> {
    if !selected.insert(job_id.to_string()) {
        return Ok(());
    }

    let job = pipeline
        .jobs
        .get(job_id)
        .with_context(|| format!("job {job_id} does not exist"))?;
    for need in &job.needs {
        include_ancestors(pipeline, need, selected)?;
    }

    Ok(())
}

fn include_descendants(
    job_id: &str,
    reverse: &HashMap<String, Vec<String>>,
    selected: &mut HashSet<String>,
) {
    if !selected.insert(job_id.to_string()) {
        return;
    }

    if let Some(children) = reverse.get(job_id) {
        for child in children {
            include_descendants(child, reverse, selected);
        }
    }
}

fn reverse_needs(pipeline: &Pipeline) -> HashMap<String, Vec<String>> {
    let mut reverse: HashMap<String, Vec<String>> = HashMap::new();
    for job in pipeline.jobs.values() {
        for need in &job.needs {
            reverse
                .entry(need.clone())
                .or_default()
                .push(job.id.clone());
        }
    }
    reverse
}

fn jobs_blocked_by_failed_needs(
    pipeline: &Pipeline,
    pending: &HashSet<String>,
    selected: &HashSet<String>,
    status_by_job: &HashMap<String, JobStatus>,
) -> Vec<String> {
    pending
        .iter()
        .filter(|job_id| {
            let job = pipeline.jobs.get(*job_id).expect("selected jobs are valid");
            !condition_has_always(job.if_condition.as_deref())
                && job
                    .needs
                    .iter()
                    .filter(|need| selected.contains(*need))
                    .any(|need| {
                        matches!(
                            status_by_job.get(need),
                            Some(JobStatus::Failed | JobStatus::Skipped)
                        )
                    })
        })
        .cloned()
        .collect()
}

fn next_ready_job(
    pipeline: &Pipeline,
    pending: &HashSet<String>,
    selected: &HashSet<String>,
    status_by_job: &HashMap<String, JobStatus>,
) -> Option<String> {
    pipeline
        .jobs
        .keys()
        .filter(|job_id| pending.contains(*job_id))
        .find(|job_id| {
            let job = pipeline.jobs.get(*job_id).expect("selected jobs are valid");
            job.needs
                .iter()
                .filter(|need| selected.contains(*need))
                .all(|need| status_by_job.contains_key(need))
        })
        .cloned()
}

fn skipped_job(job: &Job, reason: &str) -> JobOutcome {
    JobOutcome {
        job_id: job.id.clone(),
        name: job.display_name().to_string(),
        status: JobStatus::Skipped,
        reason: Some(reason.to_string()),
        duration_ms: 0,
        steps: Vec::new(),
    }
}

async fn execute_job(
    job: Job,
    project: PathBuf,
    storage: Arc<Storage>,
    secrets: Arc<SecretStore>,
    runtime: Arc<dyn RuntimeBackend>,
    strict_actions: bool,
) -> Result<JobOutcome> {
    let started = Instant::now();
    println!("localci: starting {}", job.id);
    let workspace = storage.prepare_workspace(&project, &job.id)?;
    let mut env = base_env(&job);
    let github = github_context(&project);

    {
        let ctx = EvalContext {
            env: &env,
            matrix: &job.matrix,
            secrets: secrets.values(),
            github: &github,
            runner_os: "Linux",
        };

        if !eval_condition(job.if_condition.as_deref(), &ctx) {
            println!("localci: skipped {} (if condition)", job.id);
            return Ok(JobOutcome {
                job_id: job.id.clone(),
                name: job.display_name().to_string(),
                status: JobStatus::Skipped,
                reason: Some("if condition evaluated to false".to_string()),
                duration_ms: started.elapsed().as_millis(),
                steps: Vec::new(),
            });
        }
    }

    interpolate_env(&mut env, &job, secrets.values(), &github);
    let image = {
        let ctx = EvalContext {
            env: &env,
            matrix: &job.matrix,
            secrets: secrets.values(),
            github: &github,
            runner_os: "Linux",
        };
        interpolate(&image_for_job(&job), &ctx)
    };
    if !job.services.is_empty() {
        eprintln!(
            "localci: job {} declares services; service containers are parsed but not started in this MVP",
            job.id
        );
    }

    let mut steps = Vec::new();
    let mut cache_requests = Vec::new();

    for step in &job.steps {
        let outcome = execute_step(
            &job,
            step,
            &workspace,
            &image,
            &env,
            &github,
            Arc::clone(&storage),
            Arc::clone(&secrets),
            Arc::clone(&runtime),
            strict_actions,
            &mut cache_requests,
        )
        .await?;

        let failed = outcome.status == StepStatus::Failed;
        steps.push(outcome);
        if failed {
            println!("localci: failed {}", job.id);
            return Ok(JobOutcome {
                job_id: job.id.clone(),
                name: job.display_name().to_string(),
                status: JobStatus::Failed,
                reason: Some("step failed".to_string()),
                duration_ms: started.elapsed().as_millis(),
                steps,
            });
        }
    }

    for cache in cache_requests {
        if let Err(error) = storage.save_cache(&cache.key, &cache.path, &workspace) {
            steps.push(StepOutcome {
                name: format!("save cache {}", cache.key),
                status: StepStatus::Failed,
                exit_code: None,
                duration_ms: 0,
                reason: Some(error.to_string()),
            });
            return Ok(JobOutcome {
                job_id: job.id.clone(),
                name: job.display_name().to_string(),
                status: JobStatus::Failed,
                reason: Some("cache save failed".to_string()),
                duration_ms: started.elapsed().as_millis(),
                steps,
            });
        }
    }

    println!("localci: succeeded {}", job.id);
    Ok(JobOutcome {
        job_id: job.id.clone(),
        name: job.display_name().to_string(),
        status: JobStatus::Success,
        reason: None,
        duration_ms: started.elapsed().as_millis(),
        steps,
    })
}

#[allow(clippy::too_many_arguments)]
async fn execute_step(
    job: &Job,
    step: &Step,
    workspace: &Path,
    image: &str,
    job_env: &IndexMap<String, String>,
    github: &IndexMap<String, String>,
    storage: Arc<Storage>,
    secrets: Arc<SecretStore>,
    runtime: Arc<dyn RuntimeBackend>,
    strict_actions: bool,
    cache_requests: &mut Vec<CacheRequest>,
) -> Result<StepOutcome> {
    let started = Instant::now();
    let mut env = job_env.clone();
    for (key, value) in &step.env {
        env.insert(key.clone(), value.clone());
    }
    interpolate_env_values(&mut env, &job.matrix, secrets.values(), github);

    let ctx = EvalContext {
        env: &env,
        matrix: &job.matrix,
        secrets: secrets.values(),
        github,
        runner_os: "Linux",
    };

    let step_name = step.label();
    if !eval_condition(step.if_condition.as_deref(), &ctx) {
        return Ok(StepOutcome {
            name: step_name,
            status: StepStatus::Skipped,
            exit_code: None,
            duration_ms: started.elapsed().as_millis(),
            reason: Some("if condition evaluated to false".to_string()),
        });
    }

    match &step.kind {
        StepKind::Run { command, shell } => {
            let command = interpolate(command, &ctx);
            let output = runtime
                .run_shell(
                    ShellRequest {
                        job_id: job.id.clone(),
                        step_name: step_name.clone(),
                        image: Some(image.to_string()),
                        workspace: workspace.to_path_buf(),
                        working_directory: step.working_directory.clone(),
                        command,
                        shell: shell.clone(),
                        env,
                    },
                    &secrets,
                )
                .await?;
            print_output(&job.id, &step_name, &output.stdout, &output.stderr);
            let success = output.exit_code == 0;
            Ok(StepOutcome {
                name: step_name,
                status: if success {
                    StepStatus::Success
                } else {
                    StepStatus::Failed
                },
                exit_code: Some(output.exit_code),
                duration_ms: output.duration_ms,
                reason: (!success).then(|| format!("exit code {}", output.exit_code)),
            })
        }
        StepKind::Uses { uses, with } => handle_action_step(
            job,
            &step_name,
            uses,
            with,
            workspace,
            storage,
            &ctx,
            strict_actions,
            cache_requests,
            started,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_action_step(
    job: &Job,
    step_name: &str,
    uses: &str,
    with: &IndexMap<String, String>,
    workspace: &Path,
    storage: Arc<Storage>,
    ctx: &EvalContext<'_>,
    strict_actions: bool,
    cache_requests: &mut Vec<CacheRequest>,
    started: Instant,
) -> Result<StepOutcome> {
    let action = uses.split('@').next().unwrap_or(uses).to_ascii_lowercase();
    match action.as_str() {
        "actions/checkout" => Ok(StepOutcome {
            name: step_name.to_string(),
            status: StepStatus::Success,
            exit_code: None,
            duration_ms: started.elapsed().as_millis(),
            reason: Some("workspace is already mounted locally".to_string()),
        }),
        "actions/cache" => {
            let key = with
                .get("key")
                .map(|value| interpolate(value, ctx))
                .unwrap_or_else(|| format!("{}-cache", job.id));
            let path = with
                .get("path")
                .map(|value| interpolate(value, ctx))
                .unwrap_or_default();
            if path.trim().is_empty() {
                bail!("actions/cache step requires with.path");
            }
            let restored = storage.restore_cache(&key, &path, workspace)?;
            cache_requests.push(CacheRequest {
                key: key.clone(),
                path,
            });
            Ok(StepOutcome {
                name: step_name.to_string(),
                status: StepStatus::Success,
                exit_code: None,
                duration_ms: started.elapsed().as_millis(),
                reason: Some(if restored {
                    format!("restored cache {key}")
                } else {
                    format!("cache miss {key}")
                }),
            })
        }
        "actions/upload-artifact" => {
            let name = with
                .get("name")
                .map(|value| interpolate(value, ctx))
                .unwrap_or_else(|| "artifact".to_string());
            let path = with
                .get("path")
                .map(|value| interpolate(value, ctx))
                .unwrap_or_default();
            if path.trim().is_empty() {
                bail!("actions/upload-artifact step requires with.path");
            }
            storage.upload_artifact(&name, &path, workspace)?;
            Ok(StepOutcome {
                name: step_name.to_string(),
                status: StepStatus::Success,
                exit_code: None,
                duration_ms: started.elapsed().as_millis(),
                reason: Some(format!("uploaded artifact {name}")),
            })
        }
        "actions/download-artifact" => {
            let name = with.get("name").map(|value| interpolate(value, ctx));
            let path = with
                .get("path")
                .map(|value| interpolate(value, ctx))
                .unwrap_or_else(|| ".".to_string());
            storage.download_artifact(name.as_deref(), &workspace.join(path))?;
            Ok(StepOutcome {
                name: step_name.to_string(),
                status: StepStatus::Success,
                exit_code: None,
                duration_ms: started.elapsed().as_millis(),
                reason: Some("downloaded artifact".to_string()),
            })
        }
        _ if strict_actions => Ok(StepOutcome {
            name: step_name.to_string(),
            status: StepStatus::Failed,
            exit_code: None,
            duration_ms: started.elapsed().as_millis(),
            reason: Some(format!("unsupported marketplace action {uses}")),
        }),
        _ => {
            eprintln!(
                "localci: warning: skipping unsupported action {} in job {}",
                uses, job.id
            );
            Ok(StepOutcome {
                name: step_name.to_string(),
                status: StepStatus::Skipped,
                exit_code: None,
                duration_ms: started.elapsed().as_millis(),
                reason: Some(format!("unsupported marketplace action {uses}")),
            })
        }
    }
}

fn base_env(job: &Job) -> IndexMap<String, String> {
    let mut env = job.env.clone();
    env.insert("CI".to_string(), "true".to_string());
    env.insert("RUNNER_OS".to_string(), "Linux".to_string());
    for (key, value) in &job.matrix {
        env.insert(
            format!("MATRIX_{}", key.to_ascii_uppercase()),
            value.clone(),
        );
    }
    env
}

fn interpolate_env(
    env: &mut IndexMap<String, String>,
    job: &Job,
    secrets: &IndexMap<String, String>,
    github: &IndexMap<String, String>,
) {
    interpolate_env_values(env, &job.matrix, secrets, github);
}

fn interpolate_env_values(
    env: &mut IndexMap<String, String>,
    matrix: &IndexMap<String, String>,
    secrets: &IndexMap<String, String>,
    github: &IndexMap<String, String>,
) {
    let snapshot = env.clone();
    let ctx = EvalContext {
        env: &snapshot,
        matrix,
        secrets,
        github,
        runner_os: "Linux",
    };
    for value in env.values_mut() {
        *value = interpolate(value, &ctx);
    }
}

fn github_context(project: &Path) -> IndexMap<String, String> {
    let mut github = IndexMap::new();
    github.insert("workspace".to_string(), "/workspace".to_string());
    github.insert("repository".to_string(), "".to_string());
    github.insert("event_name".to_string(), "localci".to_string());
    github.insert(
        "sha".to_string(),
        git_output(project, ["rev-parse", "HEAD"]).unwrap_or_default(),
    );
    github.insert(
        "ref".to_string(),
        git_output(project, ["symbolic-ref", "-q", "HEAD"])
            .or_else(|| git_output(project, ["rev-parse", "--abbrev-ref", "HEAD"]))
            .unwrap_or_else(|| "refs/heads/local".to_string()),
    );
    github
}

fn git_output<const N: usize>(project: &Path, args: [&str; N]) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(project)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn condition_has_always(condition: Option<&str>) -> bool {
    condition.is_some_and(|condition| condition.contains("always()"))
}

fn print_output(job_id: &str, step_name: &str, stdout: &str, stderr: &str) {
    for line in stdout.lines() {
        println!("[{job_id} :: {step_name}] {line}");
    }
    for line in stderr.lines() {
        eprintln!("[{job_id} :: {step_name}] {line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Provider, StepKind};

    #[test]
    fn selects_job_with_ancestors() {
        let mut pipeline = Pipeline::new(Provider::GitHubActions);
        pipeline.jobs.insert(
            "build".to_string(),
            Job {
                id: "build".to_string(),
                source_id: "build".to_string(),
                workflow: "CI".to_string(),
                source_file: PathBuf::from("ci.yml"),
                name: None,
                needs: Vec::new(),
                runs_on: vec!["ubuntu-latest".to_string()],
                container: None,
                env: IndexMap::new(),
                matrix: IndexMap::new(),
                if_condition: None,
                services: Vec::new(),
                steps: vec![Step {
                    id: None,
                    name: None,
                    if_condition: None,
                    env: IndexMap::new(),
                    working_directory: None,
                    kind: StepKind::Run {
                        command: "true".to_string(),
                        shell: None,
                    },
                }],
            },
        );
        pipeline.jobs.insert(
            "test".to_string(),
            Job {
                id: "test".to_string(),
                source_id: "test".to_string(),
                workflow: "CI".to_string(),
                source_file: PathBuf::from("ci.yml"),
                name: None,
                needs: vec!["build".to_string()],
                runs_on: vec!["ubuntu-latest".to_string()],
                container: None,
                env: IndexMap::new(),
                matrix: IndexMap::new(),
                if_condition: None,
                services: Vec::new(),
                steps: Vec::new(),
            },
        );

        let selected = select_job_ids(&pipeline, Some("test"), None).unwrap();
        assert!(selected.contains("build"));
        assert!(selected.contains("test"));
        assert_eq!(
            topological_order(&pipeline, &selected).unwrap(),
            vec!["build".to_string(), "test".to_string()]
        );
    }
}

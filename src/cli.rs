use crate::github;
use crate::ir::{Pipeline, StepKind};
use crate::report;
use crate::runtime::RuntimeKind;
use crate::scheduler::{
    RunOptions, run_pipeline, select_job_ids, selected_in_pipeline_order, topological_order,
};
use crate::secrets::SecretStore;
use crate::storage::Storage;
use anyhow::{Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Parser)]
#[command(name = "localci")]
#[command(about = "Run CI pipelines locally with a normalized execution engine")]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Run(RunArgs),
    #[command(name = "dry-run")]
    DryRun(DryRunArgs),
    Diff {
        pipeline_run: String,
    },
    Replay {
        gh_run_id: String,
    },
}

#[derive(Debug, Args)]
struct RunArgs {
    #[arg(short = 'C', long, default_value = ".")]
    project: PathBuf,
    #[arg(long, conflicts_with = "from")]
    job: Option<String>,
    #[arg(long, conflicts_with = "job")]
    from: Option<String>,
    #[arg(short = 'j', long)]
    parallelism: Option<usize>,
    #[arg(long, value_enum, default_value_t = RuntimeKind::Docker)]
    backend: RuntimeKind,
    #[arg(long)]
    strict_actions: bool,
    #[arg(long)]
    report_json: Option<PathBuf>,
    #[arg(long)]
    report_junit: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct DryRunArgs {
    #[arg(short = 'C', long, default_value = ".")]
    project: PathBuf,
    #[arg(long, conflicts_with = "from")]
    job: Option<String>,
    #[arg(long, conflicts_with = "job")]
    from: Option<String>,
    #[arg(long, value_enum, default_value_t = DryRunFormat::Text)]
    format: DryRunFormat,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum DryRunFormat {
    Text,
    Json,
    Dot,
}

pub async fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Run(args) => run_command(args).await,
        Command::DryRun(args) => dry_run_command(args),
        Command::Diff { pipeline_run } => {
            bail!(
                "localci diff is reserved for cloud/local comparison; {pipeline_run} is not implemented yet"
            )
        }
        Command::Replay { gh_run_id } => {
            bail!(
                "localci replay requires GitHub API integration; run {gh_run_id} is not implemented yet"
            )
        }
    }
}

async fn run_command(args: RunArgs) -> Result<()> {
    let project = resolve_project(&args.project)?;
    let pipeline = github::parse_project(&project)?;
    let secrets = SecretStore::load(&project)?;
    let storage = Storage::new()?;
    let parallelism = args.parallelism.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1)
    });

    let summary = run_pipeline(
        &pipeline,
        &project,
        storage,
        secrets,
        RunOptions {
            job: args.job,
            from: args.from,
            parallelism,
            runtime: args.backend,
            strict_actions: args.strict_actions,
        },
    )
    .await?;

    if let Some(path) = args.report_json {
        report::write_json(&summary, &path)?;
    }
    if let Some(path) = args.report_junit {
        report::write_junit(&summary, &path)?;
    }

    println!(
        "localci: run {} finished in {:.2}s ({})",
        summary.run_id,
        summary.duration_ms as f64 / 1000.0,
        if summary.success { "success" } else { "failed" }
    );

    if !summary.success {
        std::process::exit(1);
    }

    Ok(())
}

fn dry_run_command(args: DryRunArgs) -> Result<()> {
    let project = resolve_project(&args.project)?;
    let pipeline = github::parse_project(&project)?;
    let selected = select_job_ids(&pipeline, args.job.as_deref(), args.from.as_deref())?;

    match args.format {
        DryRunFormat::Text => print_text_dry_run(&pipeline, &selected)?,
        DryRunFormat::Json => {
            let jobs = selected_in_pipeline_order(&pipeline, &selected)
                .into_iter()
                .filter_map(|job_id| pipeline.jobs.get(&job_id).cloned())
                .collect::<Vec<_>>();
            println!("{}", serde_json::to_string_pretty(&DryRunJson { jobs })?);
        }
        DryRunFormat::Dot => print_dot(&pipeline, &selected),
    }

    Ok(())
}

fn print_text_dry_run(
    pipeline: &Pipeline,
    selected: &std::collections::HashSet<String>,
) -> Result<()> {
    let order = topological_order(pipeline, selected)?;
    println!("Provider: {:?}", pipeline.provider);
    println!("Workflow files:");
    for source in &pipeline.source_files {
        println!("  - {}", source.display());
    }
    println!("Jobs:");

    for job_id in order {
        let job = pipeline.jobs.get(&job_id).expect("ordered job exists");
        println!("  - {}", job.id);
        println!("    workflow: {}", job.workflow);
        if let Some(name) = &job.name {
            println!("    name: {name}");
        }
        if !job.needs.is_empty() {
            println!("    needs: {}", job.needs.join(", "));
        }
        if !job.runs_on.is_empty() {
            println!("    runs-on: {}", job.runs_on.join(", "));
        }
        if let Some(container) = &job.container {
            println!("    container: {container}");
        }
        if !job.matrix.is_empty() {
            let matrix = job
                .matrix
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
                .join(", ");
            println!("    matrix: {matrix}");
        }
        if !job.services.is_empty() {
            println!("    services: {}", job.services.len());
        }
        println!("    steps:");
        for step in &job.steps {
            match &step.kind {
                StepKind::Run { command, .. } => {
                    let first_line = command.lines().next().unwrap_or("").trim();
                    println!("      - run: {first_line}");
                }
                StepKind::Uses { uses, .. } => {
                    println!("      - uses: {uses}");
                }
            }
        }
    }

    Ok(())
}

fn print_dot(pipeline: &Pipeline, selected: &std::collections::HashSet<String>) {
    println!("digraph localci {{");
    for job in pipeline
        .jobs
        .values()
        .filter(|job| selected.contains(&job.id))
    {
        println!("  {:?};", job.id);
        for need in job.needs.iter().filter(|need| selected.contains(*need)) {
            println!("  {:?} -> {:?};", need, job.id);
        }
    }
    println!("}}");
}

fn resolve_project(project: &Path) -> Result<PathBuf> {
    let path = if project.is_absolute() {
        project.to_path_buf()
    } else {
        std::env::current_dir()?.join(project)
    };

    if !path.exists() {
        bail!("project path does not exist: {}", path.display());
    }

    Ok(path)
}

#[derive(Debug, Serialize)]
struct DryRunJson {
    jobs: Vec<crate::ir::Job>,
}

use crate::ir::{Job, Pipeline, Provider, Service, Step, StepKind};
use anyhow::{Context, Result, bail};
use indexmap::IndexMap;
use serde::Deserialize;
use serde_yaml::Value;
use std::fs;
use std::path::{Path, PathBuf};

pub fn parse_project(project: &Path) -> Result<Pipeline> {
    let workflow_files = discover_workflow_files(project)?;
    if workflow_files.is_empty() {
        bail!(
            "no GitHub Actions workflows found under {}",
            project.join(".github/workflows").display()
        );
    }

    parse_workflow_files(project, workflow_files)
}

fn parse_workflow_files(project: &Path, workflow_files: Vec<PathBuf>) -> Result<Pipeline> {
    let mut pipeline = Pipeline::new(Provider::GitHubActions);
    let multiple_workflows = workflow_files.len() > 1;
    let mut expanded = Vec::new();
    let mut expansion_map: IndexMap<String, Vec<String>> = IndexMap::new();

    for workflow_file in workflow_files {
        let content = fs::read_to_string(&workflow_file)
            .with_context(|| format!("failed to read {}", workflow_file.display()))?;
        let workflow: GithubWorkflow = serde_yaml::from_str(&content)
            .with_context(|| format!("failed to parse {}", workflow_file.display()))?;
        let workflow_id = workflow_id(&workflow_file);
        let workflow_name = workflow.name.unwrap_or_else(|| workflow_id.clone());
        pipeline.source_files.push(workflow_file.clone());

        for (job_key, raw_job) in workflow.jobs {
            let base_id = if multiple_workflows {
                format!("{workflow_id}:{job_key}")
            } else {
                job_key.clone()
            };
            let raw_needs = raw_job.needs.clone();
            let matrixes = expand_matrix(
                raw_job
                    .strategy
                    .as_ref()
                    .and_then(|strategy| strategy.matrix.as_ref()),
            )?;
            let mut expanded_ids = Vec::new();

            for matrix in matrixes {
                let id = expanded_job_id(&base_id, &matrix);
                expanded_ids.push(id.clone());
                expanded.push(ExpandedJob {
                    raw_needs: raw_needs.clone(),
                    workflow_id: workflow_id.clone(),
                    job: convert_job(
                        id,
                        base_id.clone(),
                        workflow_name.clone(),
                        workflow_file.clone(),
                        &raw_job,
                        matrix,
                    )?,
                });
            }

            expansion_map.insert(base_id, expanded_ids);
        }
    }

    for mut expanded_job in expanded {
        let mut needs = Vec::new();
        for need in &expanded_job.raw_needs {
            let base_need = if expansion_map.contains_key(need) {
                need.clone()
            } else {
                format!("{}:{need}", expanded_job.workflow_id)
            };
            let Some(ids) = expansion_map.get(&base_need) else {
                bail!(
                    "job {} needs unknown job {}",
                    expanded_job.job.source_id,
                    need
                );
            };
            needs.extend(ids.iter().cloned());
        }
        expanded_job.job.needs = needs;
        pipeline
            .jobs
            .insert(expanded_job.job.id.clone(), expanded_job.job);
    }

    let _ = project;
    Ok(pipeline)
}

fn discover_workflow_files(project: &Path) -> Result<Vec<PathBuf>> {
    let workflows_dir = project.join(".github/workflows");
    if !workflows_dir.exists() {
        return Ok(Vec::new());
    }

    let mut files = Vec::new();
    for entry in fs::read_dir(&workflows_dir)
        .with_context(|| format!("failed to read {}", workflows_dir.display()))?
    {
        let path = entry?.path();
        let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
            continue;
        };
        if matches!(extension, "yml" | "yaml") {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn convert_job(
    id: String,
    source_id: String,
    workflow: String,
    source_file: PathBuf,
    raw: &GithubJob,
    matrix: IndexMap<String, String>,
) -> Result<Job> {
    Ok(Job {
        id,
        source_id,
        workflow,
        source_file,
        name: raw.name.clone(),
        needs: Vec::new(),
        runs_on: raw.runs_on.clone(),
        container: container_image(raw.container.as_ref())?,
        env: raw.env.clone(),
        matrix,
        if_condition: raw.if_condition.clone(),
        services: convert_services(&raw.services)?,
        steps: convert_steps(&raw.steps)?,
    })
}

fn convert_steps(raw_steps: &[GithubStep]) -> Result<Vec<Step>> {
    let mut steps = Vec::new();
    for (index, raw) in raw_steps.iter().enumerate() {
        let kind = match (&raw.run, &raw.uses) {
            (Some(command), None) => StepKind::Run {
                command: command.clone(),
                shell: raw.shell.clone(),
            },
            (None, Some(uses)) => StepKind::Uses {
                uses: uses.clone(),
                with: raw.with.clone(),
            },
            (Some(_), Some(_)) => bail!("step {} cannot define both run and uses", index + 1),
            (None, None) => bail!("step {} must define run or uses", index + 1),
        };

        steps.push(Step {
            id: raw.id.clone(),
            name: raw.name.clone(),
            if_condition: raw.if_condition.clone(),
            env: raw.env.clone(),
            working_directory: raw.working_directory.clone(),
            kind,
        });
    }
    Ok(steps)
}

fn convert_services(raw_services: &IndexMap<String, GithubService>) -> Result<Vec<Service>> {
    let mut services = Vec::new();
    for (id, service) in raw_services {
        match service {
            GithubService::Image(image) => services.push(Service {
                id: id.clone(),
                image: image.clone(),
                env: IndexMap::new(),
                ports: Vec::new(),
                options: None,
            }),
            GithubService::Full(service) => services.push(Service {
                id: id.clone(),
                image: service.image.clone(),
                env: service.env.clone(),
                ports: service.ports.clone(),
                options: service.options.clone(),
            }),
        }
    }
    Ok(services)
}

fn container_image(value: Option<&Value>) -> Result<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };

    match value {
        Value::String(image) => Ok(Some(image.clone())),
        Value::Mapping(map) => Ok(map
            .get(Value::String("image".to_string()))
            .map(value_to_string)
            .transpose()?),
        _ => bail!("job container must be a string or mapping with image"),
    }
}

fn expand_matrix(
    matrix: Option<&IndexMap<String, Value>>,
) -> Result<Vec<IndexMap<String, String>>> {
    let Some(matrix) = matrix else {
        return Ok(vec![IndexMap::new()]);
    };

    let mut axes = Vec::new();
    let mut includes = Vec::new();
    let mut excludes = Vec::new();

    for (key, value) in matrix {
        match key.as_str() {
            "include" => includes = sequence_of_maps(value)?,
            "exclude" => excludes = sequence_of_maps(value)?,
            _ => axes.push((key.clone(), axis_values(value)?)),
        }
    }

    let mut combinations = vec![IndexMap::new()];
    for (axis, values) in axes {
        let mut next = Vec::new();
        for current in &combinations {
            for value in &values {
                let mut item = current.clone();
                item.insert(axis.clone(), value.clone());
                next.push(item);
            }
        }
        combinations = next;
    }

    combinations.retain(|combo| !excludes.iter().any(|exclude| map_matches(combo, exclude)));

    for include in includes {
        let mut merged = false;
        for combo in &mut combinations {
            if compatible(combo, &include) {
                for (key, value) in &include {
                    combo.insert(key.clone(), value.clone());
                }
                merged = true;
            }
        }
        if !merged {
            combinations.push(include);
        }
    }

    if combinations.is_empty() {
        combinations.push(IndexMap::new());
    }

    Ok(combinations)
}

fn axis_values(value: &Value) -> Result<Vec<String>> {
    match value {
        Value::Sequence(values) => values.iter().map(value_to_string).collect(),
        _ => Ok(vec![value_to_string(value)?]),
    }
}

fn sequence_of_maps(value: &Value) -> Result<Vec<IndexMap<String, String>>> {
    let Value::Sequence(items) = value else {
        bail!("matrix include/exclude must be arrays");
    };

    items
        .iter()
        .map(|item| {
            let Value::Mapping(map) = item else {
                bail!("matrix include/exclude items must be maps");
            };
            let mut output = IndexMap::new();
            for (key, value) in map {
                output.insert(value_to_string(key)?, value_to_string(value)?);
            }
            Ok(output)
        })
        .collect()
}

fn map_matches(combo: &IndexMap<String, String>, needle: &IndexMap<String, String>) -> bool {
    needle
        .iter()
        .all(|(key, value)| combo.get(key).is_some_and(|current| current == value))
}

fn compatible(combo: &IndexMap<String, String>, include: &IndexMap<String, String>) -> bool {
    include
        .iter()
        .all(|(key, value)| combo.get(key).is_none_or(|current| current == value))
}

fn value_to_string(value: &Value) -> Result<String> {
    match value {
        Value::Null => Ok(String::new()),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Number(value) => Ok(value.to_string()),
        Value::String(value) => Ok(value.clone()),
        Value::Sequence(_) | Value::Mapping(_) | Value::Tagged(_) => {
            Ok(serde_yaml::to_string(value)?.trim().to_string())
        }
    }
}

fn expanded_job_id(base_id: &str, matrix: &IndexMap<String, String>) -> String {
    if matrix.is_empty() {
        return base_id.to_string();
    }

    let values = matrix
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(",");
    format!("{base_id}[{values}]")
}

fn workflow_id(path: &Path) -> String {
    path.file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("workflow")
        .to_string()
}

#[derive(Debug)]
struct ExpandedJob {
    raw_needs: Vec<String>,
    workflow_id: String,
    job: Job,
}

#[derive(Debug, Deserialize)]
struct GithubWorkflow {
    name: Option<String>,
    #[serde(default)]
    jobs: IndexMap<String, GithubJob>,
}

#[derive(Debug, Clone, Deserialize)]
struct GithubJob {
    name: Option<String>,
    #[serde(default, deserialize_with = "deserialize_string_vec")]
    needs: Vec<String>,
    #[serde(
        default,
        rename = "runs-on",
        deserialize_with = "deserialize_string_vec"
    )]
    runs_on: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_string_map")]
    env: IndexMap<String, String>,
    strategy: Option<GithubStrategy>,
    container: Option<Value>,
    #[serde(default)]
    services: IndexMap<String, GithubService>,
    #[serde(default)]
    steps: Vec<GithubStep>,
    #[serde(rename = "if")]
    if_condition: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct GithubStrategy {
    matrix: Option<IndexMap<String, Value>>,
}

#[derive(Debug, Clone, Deserialize)]
struct GithubStep {
    id: Option<String>,
    name: Option<String>,
    run: Option<String>,
    uses: Option<String>,
    shell: Option<String>,
    #[serde(default, deserialize_with = "deserialize_string_map")]
    env: IndexMap<String, String>,
    #[serde(default, deserialize_with = "deserialize_string_map")]
    with: IndexMap<String, String>,
    #[serde(rename = "working-directory")]
    working_directory: Option<String>,
    #[serde(rename = "if")]
    if_condition: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum GithubService {
    Image(String),
    Full(GithubServiceFull),
}

#[derive(Debug, Clone, Deserialize)]
struct GithubServiceFull {
    image: String,
    #[serde(default, deserialize_with = "deserialize_string_map")]
    env: IndexMap<String, String>,
    #[serde(default, deserialize_with = "deserialize_string_vec")]
    ports: Vec<String>,
    options: Option<String>,
}

fn deserialize_string_map<'de, D>(deserializer: D) -> Result<IndexMap<String, String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<IndexMap<String, Value>>::deserialize(deserializer)?;
    let mut output = IndexMap::new();
    for (key, value) in value.unwrap_or_default() {
        output.insert(
            key,
            value_to_string(&value).map_err(serde::de::Error::custom)?,
        );
    }
    Ok(output)
}

fn deserialize_string_vec<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    let Some(value) = value else {
        return Ok(Vec::new());
    };

    match value {
        Value::String(value) => Ok(vec![value]),
        Value::Sequence(values) => values
            .iter()
            .map(value_to_string)
            .collect::<Result<Vec<_>>>()
            .map_err(serde::de::Error::custom),
        Value::Null => Ok(Vec::new()),
        other => value_to_string(&other)
            .map(|value| vec![value])
            .map_err(serde::de::Error::custom),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_matrix_and_expands_needs() {
        let dir = tempfile::tempdir().unwrap();
        let workflows = dir.path().join(".github/workflows");
        fs::create_dir_all(&workflows).unwrap();
        fs::write(
            workflows.join("ci.yml"),
            r#"
name: CI
on: push
jobs:
  build:
    runs-on: ubuntu-latest
    strategy:
      matrix:
        os: [ubuntu-latest]
        node: [18, 20]
    steps:
      - run: echo "${{ matrix.node }}"
  test:
    needs: build
    runs-on: ubuntu-latest
    steps:
      - run: cargo test
"#,
        )
        .unwrap();

        let pipeline = parse_project(dir.path()).unwrap();
        assert!(
            pipeline
                .jobs
                .contains_key("build[os=ubuntu-latest,node=18]")
        );
        assert!(
            pipeline
                .jobs
                .contains_key("build[os=ubuntu-latest,node=20]")
        );
        let test = pipeline.jobs.get("test").unwrap();
        assert_eq!(
            test.needs,
            vec![
                "build[os=ubuntu-latest,node=18]".to_string(),
                "build[os=ubuntu-latest,node=20]".to_string()
            ]
        );
    }
}

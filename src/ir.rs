use indexmap::IndexMap;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Provider {
    GitHubActions,
}

#[derive(Debug, Clone, Serialize)]
pub struct Pipeline {
    pub provider: Provider,
    pub jobs: IndexMap<String, Job>,
    pub source_files: Vec<PathBuf>,
}

impl Pipeline {
    pub fn new(provider: Provider) -> Self {
        Self {
            provider,
            jobs: IndexMap::new(),
            source_files: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Job {
    pub id: String,
    pub source_id: String,
    pub workflow: String,
    pub source_file: PathBuf,
    pub name: Option<String>,
    pub needs: Vec<String>,
    pub runs_on: Vec<String>,
    pub container: Option<String>,
    pub env: IndexMap<String, String>,
    pub matrix: IndexMap<String, String>,
    pub if_condition: Option<String>,
    pub services: Vec<Service>,
    pub steps: Vec<Step>,
}

impl Job {
    pub fn display_name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Service {
    pub id: String,
    pub image: String,
    pub env: IndexMap<String, String>,
    pub ports: Vec<String>,
    pub options: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Step {
    pub id: Option<String>,
    pub name: Option<String>,
    pub if_condition: Option<String>,
    pub env: IndexMap<String, String>,
    pub working_directory: Option<String>,
    pub kind: StepKind,
}

impl Step {
    pub fn label(&self) -> String {
        if let Some(name) = &self.name {
            return name.clone();
        }

        match &self.kind {
            StepKind::Run { .. } => "run".to_string(),
            StepKind::Uses { uses, .. } => uses.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum StepKind {
    Run {
        command: String,
        shell: Option<String>,
    },
    Uses {
        uses: String,
        with: IndexMap<String, String>,
    },
}

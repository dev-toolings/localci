use anyhow::{Context, Result, bail};
use glob::glob;
use ignore::WalkBuilder;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;

#[derive(Debug, Clone)]
pub struct Storage {
    pub run_id: String,
    pub cache_dir: PathBuf,
    pub artifacts_dir: PathBuf,
    pub work_dir: PathBuf,
}

impl Storage {
    pub fn new() -> Result<Self> {
        let home = dirs::home_dir().context("could not resolve home directory")?;
        let root = home.join(".localci");
        let run_id = run_id();
        let cache_dir = root.join("cache");
        let artifacts_dir = root.join("artifacts").join(&run_id);
        let work_dir = root.join("runs").join(&run_id).join("work");

        fs::create_dir_all(&cache_dir)?;
        fs::create_dir_all(&artifacts_dir)?;
        fs::create_dir_all(&work_dir)?;

        Ok(Self {
            run_id,
            cache_dir,
            artifacts_dir,
            work_dir,
        })
    }

    pub fn prepare_workspace(&self, project: &Path, job_id: &str) -> Result<PathBuf> {
        let destination = self.work_dir.join(sanitize_path_fragment(job_id));
        if destination.exists() {
            fs::remove_dir_all(&destination)?;
        }
        copy_project(project, &destination)?;
        Ok(destination)
    }

    pub fn restore_cache(&self, key: &str, path_spec: &str, workspace: &Path) -> Result<bool> {
        let cache_root = self.cache_dir.join(cache_key(key));
        let files_root = cache_root.join("files");
        if !files_root.exists() {
            return Ok(false);
        }

        for path in resolve_path_specs(path_spec, workspace)? {
            let relative = relative_or_name(&path, workspace);
            let cached = files_root.join(&relative);
            if cached.exists() {
                copy_entry(&cached, &path)?;
            }
        }

        Ok(true)
    }

    pub fn save_cache(&self, key: &str, path_spec: &str, workspace: &Path) -> Result<()> {
        let cache_root = self.cache_dir.join(cache_key(key));
        let files_root = cache_root.join("files");
        if files_root.exists() {
            fs::remove_dir_all(&files_root)?;
        }
        fs::create_dir_all(&files_root)?;

        for path in resolve_path_specs(path_spec, workspace)? {
            if !path.exists() {
                continue;
            }
            let relative = relative_or_name(&path, workspace);
            copy_entry(&path, &files_root.join(relative))?;
        }

        fs::write(cache_root.join("key"), key)?;
        Ok(())
    }

    pub fn upload_artifact(&self, name: &str, path_spec: &str, workspace: &Path) -> Result<()> {
        let artifact_root = self.artifacts_dir.join(sanitize_path_fragment(name));
        fs::create_dir_all(&artifact_root)?;

        let paths = resolve_path_specs(path_spec, workspace)?;
        if paths.is_empty() {
            bail!("artifact {name} matched no files for path {path_spec}");
        }

        for path in paths {
            let relative = relative_or_name(&path, workspace);
            copy_entry(&path, &artifact_root.join(relative))?;
        }

        Ok(())
    }

    pub fn download_artifact(&self, name: Option<&str>, destination: &Path) -> Result<()> {
        fs::create_dir_all(destination)?;
        if let Some(name) = name {
            let artifact_root = self.artifacts_dir.join(sanitize_path_fragment(name));
            if !artifact_root.exists() {
                bail!("artifact {name} does not exist in this localci run");
            }
            copy_dir_contents(&artifact_root, destination)?;
            return Ok(());
        }

        for entry in fs::read_dir(&self.artifacts_dir)? {
            let artifact = entry?.path();
            if artifact.is_dir() {
                let destination = destination.join(
                    artifact
                        .file_name()
                        .and_then(|value| value.to_str())
                        .unwrap_or("artifact"),
                );
                copy_entry(&artifact, &destination)?;
            }
        }

        Ok(())
    }
}

fn copy_project(project: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)?;
    let walker = WalkBuilder::new(project)
        .hidden(false)
        .git_ignore(true)
        .git_global(false)
        .parents(true)
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            name != ".git" && name != ".localci" && name != "target"
        })
        .build();

    for entry in walker {
        let entry = entry?;
        let path = entry.path();
        let relative = path.strip_prefix(project)?;
        if relative.as_os_str().is_empty() {
            continue;
        }

        let destination_path = destination.join(relative);
        if entry
            .file_type()
            .is_some_and(|file_type| file_type.is_dir())
        {
            fs::create_dir_all(destination_path)?;
        } else if entry
            .file_type()
            .is_some_and(|file_type| file_type.is_file())
        {
            if let Some(parent) = destination_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(path, destination_path)
                .with_context(|| format!("failed to copy {}", path.display()))?;
        }
    }

    Ok(())
}

fn resolve_path_specs(path_spec: &str, workspace: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for line in path_spec.lines() {
        let spec = line.trim();
        if spec.is_empty() {
            continue;
        }

        let candidate = if Path::new(spec).is_absolute() {
            PathBuf::from(spec)
        } else {
            workspace.join(spec)
        };

        if contains_glob(spec) {
            let pattern = candidate.to_string_lossy().to_string();
            for entry in glob(&pattern).with_context(|| format!("invalid glob pattern {spec}"))? {
                paths.push(entry?);
            }
        } else {
            paths.push(candidate);
        }
    }

    Ok(paths)
}

fn copy_entry(source: &Path, destination: &Path) -> Result<()> {
    if source.is_dir() {
        if destination.exists() {
            fs::remove_dir_all(destination)?;
        }
        fs::create_dir_all(destination)?;
        copy_dir_contents(source, destination)
    } else if source.is_file() {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(source, destination)
            .with_context(|| format!("failed to copy {}", source.display()))?;
        Ok(())
    } else {
        Ok(())
    }
}

fn copy_dir_contents(source: &Path, destination: &Path) -> Result<()> {
    for entry in WalkDir::new(source) {
        let entry = entry?;
        let path = entry.path();
        let relative = path.strip_prefix(source)?;
        if relative.as_os_str().is_empty() {
            continue;
        }

        let destination_path = destination.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(destination_path)?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = destination_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(path, destination_path)
                .with_context(|| format!("failed to copy {}", path.display()))?;
        }
    }
    Ok(())
}

fn relative_or_name(path: &Path, base: &Path) -> PathBuf {
    path.strip_prefix(base)
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            path.file_name()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("entry"))
        })
}

fn contains_glob(input: &str) -> bool {
    input.contains('*') || input.contains('?') || input.contains('[')
}

fn cache_key(key: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(key.as_bytes());
    format!("{:x}", digest.finalize())
}

fn sanitize_path_fragment(input: &str) -> String {
    input
        .chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '-' | '_' => ch,
            _ => '_',
        })
        .collect()
}

fn run_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    millis.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_and_restores_cache_paths() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("work");
        fs::create_dir_all(workspace.join("target")).unwrap();
        fs::write(workspace.join("target/file.txt"), "cached").unwrap();

        let storage = Storage {
            run_id: "test".to_string(),
            cache_dir: dir.path().join("cache"),
            artifacts_dir: dir.path().join("artifacts"),
            work_dir: dir.path().join("runs"),
        };
        fs::create_dir_all(&storage.cache_dir).unwrap();
        fs::create_dir_all(&storage.artifacts_dir).unwrap();
        fs::create_dir_all(&storage.work_dir).unwrap();

        storage.save_cache("key", "target", &workspace).unwrap();
        fs::remove_dir_all(workspace.join("target")).unwrap();
        assert!(storage.restore_cache("key", "target", &workspace).unwrap());
        assert_eq!(
            fs::read_to_string(workspace.join("target/file.txt")).unwrap(),
            "cached"
        );
    }
}

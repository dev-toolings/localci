use anyhow::{Context, Result};
use indexmap::IndexMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct SecretStore {
    values: IndexMap<String, String>,
}

impl SecretStore {
    pub fn load(project: &Path) -> Result<Self> {
        let path = project.join(".localci/secrets.env");
        if !path.exists() {
            return Ok(Self::default());
        }

        let content = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let mut values = IndexMap::new();

        for (index, line) in content.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            let Some((key, value)) = line.split_once('=') else {
                anyhow::bail!("invalid secrets.env line {}: expected KEY=value", index + 1);
            };

            let key = key.trim();
            if key.is_empty() {
                anyhow::bail!("invalid secrets.env line {}: empty key", index + 1);
            }

            values.insert(key.to_string(), unquote(value.trim()).to_string());
        }

        Ok(Self { values })
    }

    pub fn values(&self) -> &IndexMap<String, String> {
        &self.values
    }

    pub fn env_pairs(&self) -> impl Iterator<Item = (&String, &String)> {
        self.values.iter()
    }

    pub fn mask(&self, input: &str) -> String {
        let mut output = input.to_string();
        for value in self.values.values() {
            if value.len() >= 4 {
                output = output.replace(value, "***");
            }
        }
        output
    }
}

fn unquote(input: &str) -> &str {
    input
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            input
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        })
        .unwrap_or(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_loaded_secret_values() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".localci")).unwrap();
        fs::write(dir.path().join(".localci/secrets.env"), "TOKEN=abcd1234\n").unwrap();

        let secrets = SecretStore::load(dir.path()).unwrap();
        assert_eq!(secrets.mask("token abcd1234"), "token ***");
    }
}

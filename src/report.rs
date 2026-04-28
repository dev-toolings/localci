use crate::scheduler::{JobStatus, RunSummary};
use anyhow::Result;
use std::fs;
use std::path::Path;

pub fn write_json(summary: &RunSummary, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_string_pretty(summary)?)?;
    Ok(())
}

pub fn write_junit(summary: &RunSummary, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let tests = summary.jobs.len();
    let failures = summary
        .jobs
        .iter()
        .filter(|job| job.status == JobStatus::Failed)
        .count();
    let skipped = summary
        .jobs
        .iter()
        .filter(|job| job.status == JobStatus::Skipped)
        .count();

    let mut xml = String::new();
    xml.push_str(r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    xml.push('\n');
    xml.push_str(&format!(
        r#"<testsuite name="localci" tests="{tests}" failures="{failures}" skipped="{skipped}" time="{:.3}">"#,
        summary.duration_ms as f64 / 1000.0
    ));
    xml.push('\n');

    for job in &summary.jobs {
        xml.push_str(&format!(
            r#"  <testcase classname="localci" name="{}" time="{:.3}">"#,
            escape_xml(&job.job_id),
            job.duration_ms as f64 / 1000.0
        ));
        xml.push('\n');

        match job.status {
            JobStatus::Success => {}
            JobStatus::Skipped => {
                xml.push_str(&format!(
                    r#"    <skipped message="{}" />"#,
                    escape_xml(job.reason.as_deref().unwrap_or("skipped"))
                ));
                xml.push('\n');
            }
            JobStatus::Failed => {
                xml.push_str(&format!(
                    r#"    <failure message="{}">"#,
                    escape_xml(job.reason.as_deref().unwrap_or("failed"))
                ));
                xml.push_str(&escape_xml(&serde_json::to_string(&job.steps)?));
                xml.push_str("</failure>\n");
            }
        }

        xml.push_str("  </testcase>\n");
    }

    xml.push_str("</testsuite>\n");
    fs::write(path, xml)?;
    Ok(())
}

fn escape_xml(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

# Changelog

## v0.1.0

- Initial Rust CLI with `run`, `dry-run`, `diff`, and `replay` commands.
- GitHub Actions parser for workflow files, jobs, `needs`, env, containers, services, steps, and matrix expansion.
- DAG scheduler with configurable parallelism.
- Docker and shell execution backends.
- Local secret loading and log masking from `.localci/secrets.env`.
- Local emulation for `actions/cache`, `actions/upload-artifact`, and `actions/download-artifact`.
- JSON and JUnit reports.
- Bundled PHP, Node.js, Python, Rust, and Go CI fixtures.

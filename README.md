# localci

Run GitHub Actions pipelines locally from the workflow files that already exist in a project.

`localci` parses `.github/workflows/*.yml`, normalizes jobs into a DAG, expands matrix jobs, and executes the selected pipeline locally with Docker or a shell backend.

> v0.1.0 is an early MVP. It is useful for local smoke tests, parser validation, matrix/DAG debugging, cache/artifact flow checks, and quick CI reproduction experiments.

## Features

- GitHub Actions workflow discovery.
- Normalized Pipeline IR for jobs, steps, dependencies, env, matrix, containers, services, and artifacts.
- Matrix expansion with basic `include` and `exclude` support.
- DAG scheduling with configurable parallelism via `-j`.
- Docker backend by default, plus `--backend shell` for fast local tests.
- `--job <name>` to run one job and its dependencies.
- `--from <name>` to resume from a job and its descendants.
- Secret loading from `.localci/secrets.env` with log masking.
- Local emulation for `actions/cache`.
- Local emulation for `actions/upload-artifact` and `actions/download-artifact`.
- JSON and JUnit reports.

## Install

From source:

```sh
cargo install --git https://github.com/MakFly/localci
```

Or clone and run:

```sh
git clone https://github.com/MakFly/localci.git
cd localci
cargo run -- dry-run -C examples/symfony-demo
```

## Usage

Print the normalized pipeline without executing it:

```sh
localci dry-run -C /path/to/project
localci dry-run -C /path/to/project --format json
localci dry-run -C /path/to/project --format dot
```

Run the full pipeline:

```sh
localci run -C /path/to/project -j 4
```

Run one job and its dependencies:

```sh
localci run -C /path/to/project --job build
```

Resume from a job and run its descendants:

```sh
localci run -C /path/to/project --from test
```

Generate reports:

```sh
localci run -C /path/to/project \
  --report-json localci.json \
  --report-junit localci.xml
```

Use the shell backend for fast local smoke tests:

```sh
localci run -C /path/to/project --backend shell -j 4
```

## Secrets

Create `.localci/secrets.env` inside the target project:

```dotenv
API_TOKEN=example-token
DATABASE_URL=postgres://example
```

Values loaded from this file are injected into job steps and masked from logs.

Add `.localci/secrets.env` to the target project's `.gitignore`.

## Verified Fixtures

The repository includes small projects that exercise common CI/CD shapes:

```sh
cargo run -- run -C examples/symfony-demo --backend shell -j 2
cargo run -- run -C examples/node-demo --backend shell -j 2
cargo run -- run -C examples/python-demo --backend shell -j 2
cargo run -- run -C examples/rust-demo --backend shell -j 2
cargo run -- run -C examples/go-demo --backend shell -j 2
```

Fixtures included:

- `examples/symfony-demo`: PHP/Composer, Symfony-style layout, lint, tests, matrix job, cache, artifacts.
- `examples/node-demo`: Node.js/npm, matrix jobs, cache, package artifact.
- `examples/python-demo`: Python, matrix jobs, package artifact.
- `examples/rust-demo`: Rust/Cargo, matrix jobs, cache, release artifact.
- `examples/go-demo`: Go modules, matrix jobs, cache, binary artifact.

Docker mode is also supported when the workflow declares a Linux container image:

```sh
cargo run -- run -C examples/node-demo --backend docker -j 2
```

## Current Compatibility

Supported today:

- `jobs.<job_id>.needs`
- `jobs.<job_id>.runs-on`
- `jobs.<job_id>.container`
- `jobs.<job_id>.env`
- `jobs.<job_id>.strategy.matrix`
- job and step `if`
- `run` steps
- `uses` steps for:
  - `actions/checkout`
  - `actions/cache`
  - `actions/upload-artifact`
  - `actions/download-artifact`

Expression support covers common `matrix`, `env`, `secrets`, `github`, `runner.os`, equality, boolean operators, `startsWith`, and `contains` cases.

## Limitations

- GitHub Actions is the only provider implemented in v0.1.0.
- Service containers are parsed but not started yet.
- Marketplace actions beyond the built-ins above are skipped by default.
- Use `--strict-actions` to fail on unsupported marketplace actions.
- Expression compatibility targets useful local reproduction, not full provider bug compatibility.
- `diff` and `replay` are CLI placeholders for future cloud-run comparison and GitHub API replay.

## Roadmap

- Start service containers with per-job networking.
- Add marketplace action resolution for composite, Docker, and Node actions.
- Improve GitHub expression parity.
- Add GitLab CI parser support.
- Add TUI and richer logs.
- Add `replay <gh-run-id>` and local/cloud diffing.

## License

MIT

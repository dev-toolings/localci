# Symfony Demo

This is a tiny Symfony-style PHP project used to test `localci` locally.

It intentionally avoids external Symfony packages so the CI stays fast, but it keeps familiar structure:

- `src/`
- `public/`
- `bin/console`
- `tests/`
- `.github/workflows/ci.yml`

## Test with localci

From the `localci` crate:

```sh
cargo run -- dry-run -C examples/symfony-demo
cargo run -- dry-run -C examples/symfony-demo --format dot
cargo run -- run -C examples/symfony-demo --backend shell -j 2
```

Docker mode also works if Docker can pull `composer:2`:

```sh
cargo run -- run -C examples/symfony-demo --backend docker -j 2
```

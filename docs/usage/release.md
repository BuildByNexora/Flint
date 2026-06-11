# Release Checklist

This project is packaged with Maturin and published to PyPI as
`flint-limiter`.

## Local Validation

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
python3 -m venv .venv
.venv/bin/pip install -U pip maturin pytest twine
.venv/bin/maturin develop
.venv/bin/python -m pytest -q tests/python
.venv/bin/maturin build --release
.venv/bin/twine check target/wheels/*
```

## Version

Update:

- `Cargo.toml`;
- `pyproject.toml`;
- `Cargo.lock`.

Use matching versions for Rust workspace and Python package.

## Tag

```bash
git tag v0.2.1
git push origin v0.2.1
```

## Publish With Trusted Publishing

Recommended PyPI setup:

- PyPI project: `flint-limiter`;
- GitHub owner: `BuildByNexora`;
- repository: `Flint`;
- workflow: `.github/workflows/publish.yml`;
- environment: `pypi`.

Then run the publish workflow from GitHub Actions.

## Manual Publish

Use an API token if publishing locally:

```bash
.venv/bin/maturin publish
```

Credentials:

```text
username: __token__
password: pypi-...
```

## Smoke Test

Install in a clean virtual environment:

```bash
python3 -m venv /tmp/flint-wheel-test
/tmp/flint-wheel-test/bin/pip install -U pip
/tmp/flint-wheel-test/bin/pip install flint-limiter
/tmp/flint-wheel-test/bin/python -c "import flint; print(flint.Limiter)"
```

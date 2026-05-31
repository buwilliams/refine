# Python WebApp

Scaffold a production-ready Python web application for the attached empty project.

## Stack

- Python 3.14 or the newest stable Python available in the local environment.
- FastAPI for the application layer.
- Uvicorn/FastAPI CLI for local development and container entrypoints.
- `uv` with `pyproject.toml` and `uv.lock` for dependency and environment management.
- Ruff for linting and formatting.
- Pytest plus HTTPX/TestClient coverage for the HTTP surface.
- Dockerfile suitable for running the app as a single web process.

## Requirements

- Generate the app in the repository root with an `app/` package and tests under `tests/`.
- Provide commands or scripts named `start`, `dev`, `test`, `lint`, and `format` through the project tooling or a small checked-in task runner.
- `start` must work with Refine target-app lifecycle management and avoid blocking command completion if Refine expects a backgrounded service.
- Add a `/health` endpoint and a simple root route that returns HTML or JSON.
- Include a concise README with setup, development, testing, and container commands.
- Use typed request/response models where data crosses the API boundary.
- Do not add authentication, a database, or paid services unless the repository already requires them.

## Acceptance

- `uv sync` succeeds.
- `uv run pytest` and `uv run ruff check .` pass.
- The app can be launched locally and responds on `/health`.

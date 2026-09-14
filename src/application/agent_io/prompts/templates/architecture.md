## Project architecture

Keep Refine's responsibilities clear:

- Model defines shared concepts and rules.
- Application coordinates workflows and owns shared behavior.
- Infrastructure handles Git, files, processes, and agent tools.
- Surfaces expose that behavior through the CLI, API, browser, and other interfaces.

Use Rust, Git, and flat files to keep work fast and easy to inspect and recover. Make caches safe to rebuild and reuse existing tools.

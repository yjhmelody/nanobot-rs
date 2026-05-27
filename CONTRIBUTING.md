# Contributing

Thanks for your interest in contributing to nanobot!

## Getting Started

- Read `docs/QUICK_START.md` for setup and daily usage.
- Read `docs/DEVELOPMENT.md` for development workflow, testing, and debugging.
- Read `CLAUDE.md` for project conventions, architecture, and code style guidelines.

## Development Workflow

1. Fork the repository and create a feature branch from `main`.
2. Make your changes, following the conventions in `CLAUDE.md`.
3. Run the local CI suite before committing:

```bash
just fmt-check
just lint
just test
# or just: just ci
```

4. For end-to-end verification:

```bash
just e2e
```

## Pull Request Process

- Keep PRs focused on a single concern. Split large changes into multiple PRs.
- Write a clear PR description explaining what and why.
- Ensure CI passes on your PR.
- Maintain or update documentation (QUICK_START.md, ARCHITECTURE.md, DEVELOPMENT.md) when behaviour changes.

## Code Style

- Follow Rust 2024 edition idioms.
- Use `tracing` for logging, not `println!` or `dbg!`.
- Define traits before implementations for major components.
- Use `mockall` for test mocking.
- Keep clippy warnings at zero for changed code.

## Questions

Open an issue for bugs, feature requests, or questions.

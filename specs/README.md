# Specs Index

| Status | Spec | Purpose | Code Location |
|--------|------|---------|---------------|
| Active | `coding-agent.md` | Unified Rust agent: single binary, streaming, 5 tools, <950 lines | `src/` |
| Active | `release-workflow.md` | Cross-platform release builds: macOS aarch64 + Linux x86_64, tag-triggered, tarballs | `.github/workflows/release.yml` |
| Active | `session-capture.md` | Persist conversation transcripts in Entire-compatible JSONL for post-session observability | `src/` (new module) + `.entire/metadata/` |
| Active | `api-endpoint.md` | Configurable API endpoint defaulting to tailnet OAuth proxy, optional API key | `src/api.rs`, `src/main.rs` |
| Complete | `tool-name-compliance.md` | Rename tool names to match Claude Code conventions for OAuth proxy compatibility | `src/tools/mod.rs`, `src/main.rs` |

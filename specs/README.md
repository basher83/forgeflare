# Specs Index

| Status | Spec | Purpose | Code Location |
|--------|------|---------|---------------|
| Active | `coding-agent.md` | Unified Rust agent: single binary, streaming, 5 tools, <950 lines | `src/` |
| Active | `release-workflow.md` | Cross-platform release builds: macOS aarch64 + Linux x86_64, tag-triggered, tarballs | `.github/workflows/release.yml` |
| Active | `session-capture.md` | Persist conversation transcripts in Entire-compatible JSONL for post-session observability | `src/` (new module) + `.entire/metadata/` |

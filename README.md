# agentusage — `au`

A fast, native CLI for monitoring AI provider usage across Anthropic, OpenAI, Google Gemini, Cursor, Codex, Copilot, and 10+ more — all from a single command.

Built for both humans and AI agents. Run `au status --json` as a skill inside Claude Code, Cursor, or any agent framework to make intelligent model-selection decisions based on remaining quota.

![au status output](screenshot.png)

## Install

```sh
brew install agentusage-team/tap/agentusage
```

Or build from source:

```sh
cargo build --release
```

## Usage

```sh
au status                        # human-readable usage summary
au status --json                 # machine-readable JSON (agent skill interface)
au status --provider claude      # filter to one provider
au status -v                     # verbose: include providers with errors
```

## As an AI agent skill

Add this to your `CLAUDE.md` (or equivalent) to let your agent pick the best available model:

```markdown
## Model selection
Before starting any task, run `au status --json` and check `recommendation.best_provider`.
- remaining_fraction > 0.3 → use claude-opus-4-6
- remaining_fraction < 0.3 → use claude-sonnet-4-6
- remaining_fraction < 0.1 → use claude-haiku-4-5-20251001
```

### JSON output schema

```json
{
  "schema_version": 1,
  "fetched_at": "2026-03-19T10:00:00Z",
  "providers": [
    {
      "id": "claude",
      "display_name": "Claude",
      "status": "ok",
      "lines": [...],
      "remaining_fraction": 0.55
    }
  ],
  "recommendation": {
    "best_provider": "antigravity",
    "best_provider_period": "session",
    "reason": "highest remaining fraction (100%) with sufficient headroom"
  }
}
```

## Supported providers

| Provider | Plugin |
|----------|--------|
| Anthropic Claude | `claude` |
| OpenAI Codex | `codex` |
| Google Gemini | `gemini` |
| GitHub Copilot | `copilot` |
| Cursor | `cursor` |
| Windsurf | `windsurf` |
| Amp | `amp` |
| Antigravity | `antigravity` |
| Factory | `factory` |
| JetBrains AI | `jetbrains-ai-assistant` |
| Kimi | `kimi` |
| MiniMax | `minimax` |
| OpenCode Go | `opencode-go` |
| Perplexity | `perplexity` |
| ZAI | `zai` |

Providers are loaded from `~/Library/Application Support/agentusage/plugins/` (macOS) or the XDG equivalent on Linux. Drop any compatible [OpenUsage](https://github.com/robinebers/openusage) plugin directory there to add a new provider instantly — no recompile needed.

## Plugin system

AgentUsage is compatible with the [robinebers/openusage](https://github.com/robinebers/openusage) JS plugin format. Each plugin is a directory with three files:

```
plugins/<id>/
  plugin.json   ← manifest (id, name, lines schema)
  plugin.js     ← probe() function runs in a QuickJS sandbox
  icon.svg      ← brand icon
```

Plugins run inside `rquickjs` — the same sandboxed JS runtime used by the upstream project. The host API surface (`ctx.host.http`, `ctx.host.fs`, `ctx.host.sqlite`, `ctx.host.keychain`, etc.) is implemented in Rust.

Credentials are stored in the native OS keychain (macOS Keychain, Windows Credential Manager, libsecret on Linux) — never in plaintext.

## Building

Requires Rust 1.78+.

```sh
git clone https://github.com/benliong/agentusage-cli
cd agentusage-cli
cargo build --release
./target/release/au status
```

## Credits

Bundled plugins are sourced from [robinebers/openusage](https://github.com/robinebers/openusage) under their original license. See [bundled_plugins/NOTICE](bundled_plugins/NOTICE).

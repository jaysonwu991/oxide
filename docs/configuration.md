# Configuration reference

Where Oxide's files live and every key, flag, and environment variable they
accept. For task-by-task instructions (adding MCP servers, agents, commands,
skills, plugins, permission rules) see [cli.md](cli.md).

## config.json

Oxide reads `config.json` from the platform config directory:

- Linux: `~/.config/Oxide/config.json`
- macOS: `~/Library/Application Support/Oxide/config.json`
- Windows: `%APPDATA%\Oxide\config.json`

```json
{
  "provider": "deepseek",
  "model": "deepseek-chat",
  "base_url": "https://api.deepseek.com/v1",
  "api_key": "",
  "system_prompt": "You are Oxide...",
  "max_tokens": 8192,
  "context_window": 1000000,
  "auto_approve": true,
  "reasoning": "auto",
  "theme": "dark"
}
```

`api_key` may be left empty when a key is available via `/connect` or the
environment.

### auto_approve

`auto_approve` controls whether tool calls run without prompting; with it off,
permission rules that resolve to `ask` are asked about by the TUI and by the
desktop app (an approval card), and are denied in a non-interactive run. The TUI
asks in the transcript and answers in the composer: `y` runs the tool once, `a`
allows it for this project from now on, and `n` — optionally with a reason the
agent reads as guidance — refuses it (`Esc` refuses too). `--ask-approvals` asks
for one run, `/permissions [on|off]` toggles the stored value, `/permissions list` shows the state and the tools this project
allows, and `/permissions clear` forgets them. The VS Code panel asks on its own
setting (`oxide.askApprovals`) rather than this one. See [Permissions](cli.md#permissions).

### reasoning

`reasoning` controls how much reasoning effort Oxide requests. `auto` (the
default) leaves reasoning behavior and effort to the provider/model. Newer Claude
models use adaptive thinking without a forced effort; other APIs receive no
effort override. `off`, `minimal`, `low`, `medium`, `high`, `xhigh`, and `max`
force a level using OpenAI-compatible `reasoning_effort`, Anthropic adaptive
thinking with `output_config.effort`, or a legacy Anthropic thinking budget as
appropriate; direct DeepSeek and Z.AI use their `thinking.type` object instead.
A model whose provider listing advertised its own effort levels narrows the set
to those (see [the model's own levels](cli.md#the-models-own-levels)). In the TUI
press Shift+Tab to cycle levels; `--reasoning` and `OXIDE_REASONING` set the
starting level, and resuming a session restores the level recorded on it. See
[Reasoning](cli.md#reasoning).

### max_tokens and context_window

`max_tokens` caps the output of a single model turn, reasoning included. When a
reasoning model exhausts that budget on hidden reasoning and returns nothing,
Oxide retries with a doubled budget (up to 32768) before reporting the failure,
so a long-thinking turn recovers instead of ending in an empty response.

`context_window` is the model's full context window in tokens, and controls both
the context gauge and the automatic compaction threshold. When it is unset (or
`0`), Oxide resolves the window the way Pi does: the model catalog the provider
publishes on `pi.dev`, then a built-in table of documented windows, then Pi's
`128000` fallback. The catalog is fetched in the background when a run starts,
revalidated with an ETag and remembered in `model-catalog.json` beside
`config.json`, so a launch reads the windows from disk and never waits on the
network for them; it is looked up again only once the remembered answer is four
hours old. That is what gives a model released since this binary was built, or
one a gateway spells its own way, the window its provider actually published —
including where a provider offers less than its own default, as Bedrock does
with Claude Sonnet 4.5's 200k against Anthropic's 1M. A model the catalog does
not hold falls back to the built-in table — including the 1M windows Pi
assigns (Gemini, `gpt-4.1`, `gpt-5.4` and later, Claude Sonnet 4.5/4.6,
Opus 4.6 and later, GLM 5.2/5.3, DeepSeek Flash and V4, Kimi K3). A routed
or Bedrock-spelled id matches by its basename, so
`anthropic/claude-opus-4.7` and `us.anthropic.claude-sonnet-4-5-…` resolve to
the same windows. Override one model with `modelContextWindows` in
`settings.json`, the whole run with `context_window`, or one environment with
`OXIDE_CONTEXT_LIMIT`; turn the catalog off with `modelCatalog: false` in
`settings.json` or `OXIDE_MODEL_CATALOG=false`, which leaves the built-in table
in charge — what an offline or air-gapped machine wants.

A front-end that cannot link `oxide-core` prints the same resolution instead of
keeping a table of its own:

```sh
oxide context --json [--model <MODEL>]     # {"model":…,"window":…}
```

The window is the model's, so `--model` answers for the model a caller would run
with rather than the one in `config.json` — the VS Code panel passes its own
`oxide.model` setting that way, which is what keeps its model chip and context
gauge in step with the run.

## CLI flags

```
oxide [OPTIONS] [@files...] [PROMPT...]
oxide mcp <COMMAND>
oxide plugin <COMMAND>
oxide sessions <COMMAND>
oxide uninstall [--keep-config] [--keep-data] [--dry-run] [--force]
oxide update [--check] [--component <cli|desktop|extension>] [--current <VERSION>] [--version <VERSION>] [--force] [--json]
```

| Flag | Description |
| --- | --- |
| `[PROMPT]...` | Prompt words. `@path` reads a file into the prompt (images and PDFs become attachments). Providing one implies non-interactive mode. |
| `-m, --model <MODEL>` | Model to use (overrides config). |
| `--provider <PROVIDER>` | Provider name (overrides config). |
| `--agent <AGENT>` | Agent to run, from `.oxide/agents` (or `.claude/agents`). |
| `--mode <MODE>` | Output mode: `print`, `json`, or `rpc` (defaults to print for a prompt). |
| `--ask-approvals` | Ask before a permission-gated tool runs: in the TUI the question is answered in the composer, and in `--mode rpc` it goes to the client as an `approval_request`. |
| `--no-ask-approvals` | Run gated tools without asking, overriding `auto_approve` for this run. |
| `--ask-questions` | Send `question_request` frames to `--mode rpc` clients (skill `ask` tool questions); without it the `ask` tool is not offered. |
| `--reasoning <LEVEL>` | Reasoning effort: `auto` (default), `off`, `minimal`, `low`, `medium`, `high`, `xhigh`, or `max`. |
| `--system-prompt <TEXT>` | Replace the default system prompt for this run. |
| `--append-system-prompt <TEXT>` | Append text to the system prompt (repeatable). |
| `--no-context-files` | Disable `AGENTS.md`/`CLAUDE.md` context-file discovery. |
| `-t, --tools <LIST>` | Allowlist tools (comma-separated, Pi or legacy names). |
| `-x, --exclude-tools <LIST>` | Disable tools (comma-separated). |
| `-a, --approve` | Trust project-local resources for this run. |
| `--no-approve` | Ignore project-local resources for this run. |
| `--use-theme <NAME>` | TUI theme (`dark`, `light`, or a custom `.oxide/themes` file). |
| `--session <PATH\|ID>` | Use a specific session file or id. |
| `-n, --name <NAME>` | Set the session display name at startup. |
| `--no-session` | Ephemeral mode: do not save the session. |
| `-p, --print` | Print the response and exit instead of launching the TUI. |
| `-c, --continue` | Resume the most recent session for this project. |
| `-r, --resume` | Browse and select a past session to resume. |
| `--fork <PATH\|ID>` | Fork a session file or id into a new session. |
| `--image <PATH>` | Attach an image, a PDF or a text file (repeatable). |
| `-C, --cwd <DIR>` | Working directory for the agent. |

## Environment variables

| Variable | Purpose |
| --- | --- |
| `OXIDE_PROVIDER` | Provider name. |
| `OXIDE_MODEL` | Model name. |
| `OXIDE_BASE_URL` | API base URL. |
| `OXIDE_API_KEY` | API key. |
| `OXIDE_REASONING` | Reasoning effort (`auto`, `off`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`). |
| `OXIDE_CONTEXT_LIMIT` | Override `context_window`, the model context window used for the footer's context percentage and compaction threshold. |
| `OXIDE_EXPERIMENTAL` | Set to `1` to show Pi's `xp` marker in the footer. |
| `OXIDE_COMPACTION_ENABLED` | Enable/disable automatic context compaction. |
| `OXIDE_COMPACTION_RESERVE_TOKENS` | Tokens reserved for the response before compaction triggers. |
| `OXIDE_COMPACTION_KEEP_RECENT_TOKENS` | Recent tokens kept verbatim when compacting. |
| `OXIDE_TRUNCATION_DIR` | Directory for saved truncated tool output (default `truncated/` in the config dir). |
| `OXIDE_NOTIFY_ON_COMPLETE` / `OXIDE_NOTIFY_SOUND` | Override the desktop-notification flags (`/notify`). |
| `OXIDE_SETTINGS_FILE` | Override the global `settings.json` path the TUI writes. |
| `OXIDE_CHECK_FOR_UPDATES` | Enable/disable the launch's look for a newer CLI release (`/updates`). |
| `OXIDE_MODEL_CATALOG` | Enable/disable the launch's look for the provider model catalogs (`modelCatalog` in `settings.json`). |
| `OXIDE_MODEL_CATALOG_FILE` | Override the `model-catalog.json` path the remembered catalogs are kept in. |
| `OXIDE_MODEL_CATALOG_URL` | Override the `https://pi.dev` base URL the catalogs are fetched from. |
| `OXIDE_UPDATES_FILE` | Override the `updates.json` path the launch notice is remembered in. |
| `OXIDE_USAGE_FILE` | Override the `portkey-usage.json` path for the Portkey spend bar. |
| `PORTKEY_CONFIG` | Optional Portkey config ID sent as `x-portkey-config`. |
| `PORTKEY_MODELS` | Optional comma-separated model catalog for keys that cannot call `/models`. |

Every provider's own variables are listed with it in
[Providers](#providers) below. A provider also reads `<NAME>_BASE_URL` when the
preset declares one, so `OPENAI_BASE_URL`, `ANTHROPIC_BASE_URL`,
`PORTKEY_BASE_URL`, `GEMINI_BASE_URL`, `GROQ_BASE_URL` and the rest point that
provider at another host. `OXIDE_API_KEY` and `OXIDE_BASE_URL` beat them all for
the provider in use.

## Providers

Every provider Oxide knows is declared once, in the single table the login
picker, the aliases, the environment variables and the client dispatch all read.
Naming one of these (by name or by alias) with `/connect`, `--provider` or
`"provider"` in `config.json` is enough: the endpoint, the default model, the key
variable and the wire dialect come from the table.

| Name | Aliases | API | Default model | Base URL | Key env |
| --- | --- | --- | --- | --- | --- |
| `openai` | `gpt`, `gpt-4`, `gpt-4o` | OpenAI-compatible | `gpt-4o-mini` | `https://api.openai.com/v1` | `OPENAI_API_KEY` |
| `anthropic` | `claude` | Anthropic Messages | `claude-3-5-sonnet-latest` | `https://api.anthropic.com/v1` | `ANTHROPIC_API_KEY` |
| `deepseek` |  | OpenAI-compatible | `deepseek-chat` | `https://api.deepseek.com/v1` | `DEEPSEEK_API_KEY` |
| `google` | `gemini`, `google-ai`, `googleai` | Gemini `generateContent` | `gemini-2.0-flash` | `https://generativelanguage.googleapis.com/v1beta` | `GEMINI_API_KEY`, `GOOGLE_API_KEY`, `GOOGLE_GENERATIVE_AI_API_KEY` |
| `portkey` | `port-key` | OpenAI-compatible gateway | `claude-sonnet-5` | `https://api.portkey.ai/v1` | `PORTKEY_API_KEY` |
| `zai` | `glm`, `z.ai`, `z-ai`, `zhipu`, `bigmodel` | OpenAI-compatible | `glm-5.3` | `https://api.z.ai/api/paas/v4` | `ZAI_API_KEY`, `ZHIPU_API_KEY`, `GLM_API_KEY` |
| `xai` | `grok`, `x-ai` | OpenAI-compatible | `grok-3` | `https://api.x.ai/v1` | `XAI_API_KEY` |
| `mistral` |  | OpenAI-compatible | `mistral-large-latest` | `https://api.mistral.ai/v1` | `MISTRAL_API_KEY` |
| `openrouter` |  | OpenAI-compatible | `anthropic/claude-3.5-sonnet` | `https://openrouter.ai/api/v1` | `OPENROUTER_API_KEY` |
| `groq` |  | OpenAI-compatible | `llama-3.3-70b-versatile` | `https://api.groq.com/openai/v1` | `GROQ_API_KEY` |
| `cerebras` |  | OpenAI-compatible | `llama-3.3-70b` | `https://api.cerebras.ai/v1` | `CEREBRAS_API_KEY` |
| `together` | `togetherai` | OpenAI-compatible | `meta-llama/Llama-3.3-70B-Instruct-Turbo` | `https://api.together.xyz/v1` | `TOGETHER_API_KEY` |
| `fireworks` | `fireworks-ai` | OpenAI-compatible | `accounts/fireworks/models/llama-v3p3-70b-instruct` | `https://api.fireworks.ai/inference/v1` | `FIREWORKS_API_KEY` |
| `deepinfra` |  | OpenAI-compatible | `meta-llama/Llama-3.3-70B-Instruct` | `https://api.deepinfra.com/v1/openai` | `DEEPINFRA_API_KEY` |
| `nebius` |  | OpenAI-compatible | `meta-llama/Llama-3.3-70B-Instruct` | `https://api.studio.nebius.com/v1` | `NEBIUS_API_KEY` |
| `baseten` |  | OpenAI-compatible | `meta-llama/Llama-3.3-70B-Instruct` | `https://inference.baseten.co/v1` | `BASETEN_API_KEY` |
| `siliconflow` |  | OpenAI-compatible | `Qwen/Qwen2.5-72B-Instruct` | `https://api.siliconflow.com/v1` | `SILICONFLOW_API_KEY` |
| `novita` | `novita-ai` | OpenAI-compatible | `meta-llama/llama-3.3-70b-instruct` | `https://api.novita.ai/openai` | `NOVITA_API_KEY` |
| `nvidia` | `nim` | OpenAI-compatible | `meta/llama-3.3-70b-instruct` | `https://integrate.api.nvidia.com/v1` | `NVIDIA_API_KEY` |
| `upstage` |  | OpenAI-compatible | `solar-pro` | `https://api.upstage.ai/v1/solar` | `UPSTAGE_API_KEY` |
| `moonshot` | `kimi`, `moonshot-ai` | OpenAI-compatible | `kimi-k2-0711-preview` | `https://api.moonshot.ai/v1` | `MOONSHOT_API_KEY` |
| `alibaba` | `qwen`, `dashscope` | OpenAI-compatible | `qwen-max` | `https://dashscope-intl.aliyuncs.com/compatible-mode/v1` | `DASHSCOPE_API_KEY`, `QWEN_API_KEY` |
| `minimax` |  | Anthropic Messages | `MiniMax-M2` | `https://api.minimax.io/anthropic/v1` | `MINIMAX_API_KEY` |
| `perplexity` | `pplx` | OpenAI-compatible | `sonar` | `https://api.perplexity.ai` | `PERPLEXITY_API_KEY` |
| `cohere` |  | OpenAI-compatible | `command-r-plus` | `https://api.cohere.ai/compatibility/v1` | `COHERE_API_KEY` |
| `vercel` | `ai-gateway`, `gateway` | OpenAI-compatible | `anthropic/claude-3.5-sonnet` | `https://ai-gateway.vercel.sh/v1` | `AI_GATEWAY_API_KEY` |
| `huggingface` | `hf` | OpenAI-compatible | `meta-llama/Llama-3.3-70B-Instruct` | `https://router.huggingface.co/v1` | `HF_TOKEN`, `HUGGINGFACE_API_KEY` |
| `ollama` |  | OpenAI-compatible | `llama3.3` | `http://localhost:11434/v1` | `OLLAMA_API_KEY` |
| `lmstudio` | `lm-studio` | OpenAI-compatible | `local-model` | `http://localhost:1234/v1` | `LMSTUDIO_API_KEY` |
| `llamacpp` | `llama-cpp`, `llama.cpp` | OpenAI-compatible | `local-model` | `http://localhost:8080/v1` | `LLAMACPP_API_KEY` |
| `vertex` | `google-vertex`, `vertex-ai` | Gemini `generateContent` | `gemini-2.0-flash` | derived from the project and location | `GOOGLE_VERTEX_CREDENTIALS`, `GOOGLE_APPLICATION_CREDENTIALS` |
| `bedrock` | `amazon-bedrock`, `aws-bedrock`, `aws` | Bedrock Converse | `anthropic.claude-3-5-sonnet-20241022-v2:0` | `https://bedrock-runtime.<region>.amazonaws.com` | `AWS_BEARER_TOKEN_BEDROCK` (or AWS SigV4 credentials) |
| `azure` | `azure-openai`, `azure-ai` | Azure OpenAI | `&mdash;` | your resource's endpoint | `AZURE_API_KEY`, `AZURE_OPENAI_API_KEY` |
| `github-copilot` | `copilot`, `github` | OpenAI-compatible | `gpt-4o` | `https://api.githubcopilot.com` | `GITHUB_TOKEN`, `GH_TOKEN` |
| `gitlab` | `gitlab-duo`, `duo` | OpenAI-compatible (Anthropic for a Claude model) | `gpt-4o` | the AI gateway's proxy | `GITLAB_TOKEN`, `GL_TOKEN` |

A provider that is not in the table still works the same way: set `provider`,
`base_url`, `model` and a key, and it is treated as an OpenAI-compatible
endpoint under the name you gave it (a key with no preset behind it is read from
`OPENAI_API_KEY`, or from `OXIDE_API_KEY`, which any provider accepts, and the
model and endpoint it uses are remembered separately from every other provider).
Naming a
known provider's host as the `base_url` of a custom provider also gets that
host's dialect, so a gateway in front of a first-party API is still spoken to the
right way.

### Wire dialects

Four dialects are implemented, and a provider speaks one of them:

| Dialect | Providers | Notes |
| --- | --- | --- |
| OpenAI-compatible | most of the table, plus any custom `base_url` | Chat completions, streamed over SSE. The session id is sent as `prompt_cache_key` (direct OpenAI) or as `x-session-id`/`x-client-request-id` (gateways) for cache affinity. |
| Anthropic Messages | `anthropic`, `minimax`, and `gitlab` for its `claude-*` models | Native `cache_control` breakpoints on the system prompt, the last tool and the newest message; extended thinking blocks are replayed on later turns. |
| Gemini `generateContent` | `google`, `vertex` | Streamed with `streamGenerateContent`; the key is a header on the Gemini API and an OAuth token on Vertex. |
| Bedrock Converse | `bedrock` | Streamed `ConverseStream`, signed with SigV4. |

Reasoning is translated per dialect: `reasoning_effort` on OpenAI-compatible
endpoints, adaptive or extended thinking on Anthropic, `thinkingConfig` on
Gemini, `thinking.type` on Z.AI and DeepSeek's own API,
`additionalModelRequestFields` on Bedrock.

### Google and Vertex AI

The Gemini API takes a key: `/connect google` (aliases `gemini`, `google-ai`,
`googleai`) stores it, or set `GEMINI_API_KEY`, `GOOGLE_API_KEY` or
`GOOGLE_GENERATIVE_AI_API_KEY`.

Vertex AI runs the same models under a Google Cloud project and location, and
authenticates with a service-account credential instead of a key. Set
`GOOGLE_VERTEX_CREDENTIALS` to the service-account JSON itself (or
`GOOGLE_APPLICATION_CREDENTIALS` to its path, which is also the file
`gcloud auth application-default login` writes — `~/.config/gcloud` on Unix and
`%APPDATA%\gcloud` on Windows, or the directory `CLOUDSDK_CONFIG` names), plus
`GOOGLE_VERTEX_LOCATION`
(default `us-central1`). The token is minted from the key and refreshed as it
expires, and the project — `GOOGLE_VERTEX_PROJECT` or `GOOGLE_CLOUD_PROJECT` — is
taken from the key itself when neither is set. A service account with no file
behind it (an ambient credential on a Google Cloud workload) is not read; a file
is what is looked for.

### Amazon Bedrock

`/connect bedrock` stores either kind of credential the API accepts:

- **A Bedrock API key** (`AWS_BEARER_TOKEN_BEDROCK`, or the key typed into
  `/connect bedrock`) is sent as a bearer token — it is the credential this
  configuration holds, so it wins over signing when it is set.
- **AWS credentials** (`AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY`, plus
  `AWS_SESSION_TOKEN` for a temporary one) sign each request with SigV4, read
  from the environment or from `~/.aws/credentials` (profile from
  `AWS_PROFILE`, default `default`).

`AWS_REGION` (or `AWS_DEFAULT_REGION`) names the region and defaults to
`us-east-1`. A turn streams from `bedrock-runtime.<region>.amazonaws.com`, while
the `/models` listing asks the control plane at `bedrock.<region>.amazonaws.com`,
and the model id
is Bedrock's own — `anthropic.claude-3-5-sonnet-20241022-v2:0` by default, with
`amazon.`, `meta.` or `mistral.` ids for the open models. `/models` lists the
foundation models the account can actually invoke.

### Azure OpenAI

Azure serves OpenAI models from your own resource, so it needs three things:

- The endpoint: `AZURE_OPENAI_ENDPOINT`, `AZURE_OPENAI_BASE_URL`, or a
  `base_url` in `config.json`. `AZURE_OPENAI_RESOURCE` (or
  `AZURE_RESOURCE_NAME`) builds `https://<resource>.openai.azure.com` instead.
- The deployment: the model name is used as the deployment, or set
  `AZURE_OPENAI_DEPLOYMENT` to name one explicitly.
- The key: `AZURE_API_KEY` or `AZURE_OPENAI_API_KEY`.

Requests go to
`<endpoint>/openai/deployments/<deployment>/chat/completions?api-version=<version>`,
with the key in the `api-key` header. The version defaults to `2024-10-21` and
can be overridden with `AZURE_OPENAI_API_VERSION` or `AZURE_API_VERSION`.

### GitHub Copilot

Copilot models are served from a GitHub subscription rather than an API key, and
the credential is a GitHub token that is exchanged for a short-lived Copilot
session token before each turn. The session names the endpoint your account is
served from, so a Copilot Business account's host is used rather than assumed.

In the TUI, `/connect github-copilot` (aliases `copilot`, `github`) runs GitHub's
device flow: it shows a URL and a one-time code (also copied to the system
clipboard), waits for the approval in the browser, and stores the token it mints
— then the optional settings step, as for any other provider. Set `GITHUB_TOKEN`
or `GH_TOKEN` instead to use a token you already have (`gh auth token` prints
one); the desktop app and the VS Code panel paste one the same way. `copilot`,
`github` and the "GitHub Copilot" row in the picker are the same provider.

`/models` lists the models the subscription exposes. A Copilot token is
short-lived: it is minted on demand and cached in memory until just before it
expires, so a long session does not fail in the middle of a turn.

### GitLab Duo

Duo is served from a GitLab subscription rather than an endpoint a token opens
directly. The token you store — a personal access token with the `ai_features`
scope — is presented to your GitLab instance, which mints a short-lived (30
minute) token for GitLab's AI gateway, and the turn is sent to the gateway's
proxy with that token and the headers the instance handed back beside it. The
minted token is cached in memory until just before it lapses, so a long session
is not re-authorized every turn; the token that lives on disk is the one you
pasted.

The gateway serves two proxies and the model name says which one a turn takes:
a `claude-*` model goes through Anthropic's wire (`/ai/v1/proxy/anthropic/v1`),
everything else through OpenAI's (`/ai/v1/proxy/openai/v1`). So the model is the
provider's own model name — `claude-sonnet-4-6`, `gpt-4o` — not the
`duo-chat-*` alias the Duo chat UI lists; `/models` lists the catalog of the
proxy the model you are set to speaks (Anthropic's for a Claude model, OpenAI's
otherwise), with the same minted token a turn carries. Because Anthropic is a
separate wire dialect, the gateway's Anthropic proxy is not reachable by
`base_url` alone: an Anthropic request authenticates with `x-api-key`, while the
proxy wants the token the instance minted, and the instance token is not that.
The provider name `gitlab` is what selects the proxy and the minted token.

```bash
export GITLAB_TOKEN=glpat-…   # a personal access token with ai_features
oxide --provider gitlab --model claude-sonnet-4-6 -p "explain this repo"
```

`GITLAB_INSTANCE_URL` (or `GITLAB_URL`) points at a self-managed instance
instead of `gitlab.com`, and `GITLAB_AI_GATEWAY_URL` points at a self-managed
gateway; the settings step's base URL does the same thing for the gateway. In the
TUI, `/connect gitlab` asks for the token and stores it, as for any pasted
credential — there is no browser flow for a personal access token.

Duo needs a subscription that includes it — GitLab Ultimate with the Duo
Enterprise add-on, and the AI features enabled for the account — on an instance
that serves the third-party agent endpoint. A licence without it, a credential
without the `ai_features` scope and an instance without the endpoint are each
reported in their own words rather than as a failed request.

### Local servers

`ollama` (`http://localhost:11434/v1`), `lmstudio` (`http://localhost:1234/v1`)
and `llamacpp` (`http://localhost:8080/v1`) need no credential: a model server on
this machine is reached without a key, and the key variables above only exist for
a setup that requires one. `/connect ollama` is therefore the whole login: the
provider is applied with an empty key and the dialog goes on to its settings
rather than asking for a credential nothing stores. A provider is treated as
local by the table, not by the URL, so a custom provider pointed at a remote host
still needs a key.

### Credentials

Credential management happens inside the TUI with the Pi-style commands:

```text
/connect [provider]    connect a provider and store its API key
/logout [provider]   remove stored credentials
```

`/connect` opens a provider picker (`/connect <provider>` skips straight to the key;
for a provider that is already connected it switches to it instead of asking for
the key again, and pressing Enter on the key step reuses the stored key). After
the key, an optional settings step lets you set the model, base URL, and (for
Portkey) Config ID, pre-filled with the provider's defaults so Enter keeps them.
Keys are stored in `auth.json` in the Oxide config directory (mode `0600`) and
resolved after environment variables and before the config file. Any number of
providers can be stored at once, and the active provider is written to
`config.json` so the next launch uses it. `/models` lists the catalogs of every
logged-in provider, and picking a model from another one switches to it. Each
provider remembers the model and custom endpoint it was last used with in the
`provider_models` and `provider_base_urls` maps of `config.json`, so a gateway
configured for one provider does not leak into the others. A Portkey Config ID
chosen in that settings step is written to `portkey_config`, and kept there
whatever provider is active next, since it belongs to Portkey rather than to the
selection. See
[Providers and credentials](cli.md#providers-and-credentials).

### Portkey

Run `/connect portkey` in the TUI, or set `PORTKEY_API_KEY`, then select a model
with `/models` or `"model"` in `config.json`. The preset uses
`https://api.portkey.ai/v1`, sends the key as `x-portkey-api-key`, and defaults
to `claude-sonnet-5`. Run `/spend` to open the spend-bar dialog and enable a
full-width bar (session, today, and month cost against an optional monthly budget
in `$` or `¥`) in the TUI.

For custom gateways, Config IDs, environment precedence, and model-catalog
fallbacks, see the full [Portkey configuration](cli.md#portkey) section. The
[Portkey usage bar](cli.md#portkey-usage-bar) documents the `/spend` dialog and
the `portkey-usage.json` file — the shared `/usage` reports this chat's tokens,
cost and context window instead.

### Z.AI (GLM)

Run `/connect glm` in the TUI (or `/connect zai`), or set `ZAI_API_KEY`, then pick a
model with `/models`. The preset talks to Z.AI's OpenAI-compatible endpoint
`https://api.z.ai/api/paas/v4` and defaults to `glm-5.3`; `glm-5.3-flash` is the
cheaper, faster option. Z.AI documents no model listing endpoint, so the picker
falls back to a bundled list of current GLM models.

GLM selects thinking with `thinking.type` rather than `reasoning_effort`, and the
GLM-5.3 series only accepts `low`, `high`, or `max`. `/reasoning` therefore maps
`minimal` and `low` to `low`, `medium` and `high` to `high`, and `xhigh` and
`max` to `max`; `off` disables thinking where the model allows it and asks for
the lowest effort on GLM-5.3, which always thinks. GLM prices ship in the
built-in table, so the footer's `$cost` works out of the box.

For the mainland-China BigModel endpoint
(`https://open.bigmodel.cn/api/paas/v4`), set `ZAI_BASE_URL` or `base_url`.

### DeepSeek

Run `/connect deepseek`, or set `DEEPSEEK_API_KEY`, then pick a model with
`/models`. The preset talks to `https://api.deepseek.com/v1` and defaults to
`deepseek-chat`. DeepSeek's chat and reasoning models are one thinking model:
the API toggles thinking with `thinking.type` and accepts `low`, `high`, or
`max` as its `reasoning_effort`. `/reasoning off` therefore sends
`thinking.type = "disabled"` (omitting the object would leave the model thinking
at its own default), and the explicit levels send `thinking.type = "enabled"`
with `minimal`/`low` → `low`, `medium`/`high` → `high`, and `xhigh`/`max` →
`max`. The model's listing advertises exactly `low`/`high`/`max`, so Oxide reads
those and clamps `medium` onto `high` rather than sending a level the API does
not know. Because the thinking mode expects
every earlier assistant message to carry its `reasoning_content`, Oxide replays
the thinking it captured on all of them, with an empty string where a turn
thought nothing.

A DeepSeek model accessed through a gateway (Portkey, OpenRouter) keeps that
gateway's reasoning shape rather than the DeepSeek `thinking` object; only the
provider preset or an `api.deepseek.com` endpoint uses the shape above.

## Context files and the system prompt

Oxide loads `AGENTS.md` (or `CLAUDE.md`) as project instructions by walking every
ancestor directory from the filesystem root down to the working directory, so
nested projects layer their instructions. If a directory contains
`AGENTS.override.md`, it replaces `AGENTS.md`/`CLAUDE.md` for that directory
only. The global `~/.oxide/AGENTS.md` is loaded first (lowest precedence).

- Disable ancestor context-file discovery with `--no-context-files`.
- Replace the default system prompt with `.oxide/SYSTEM.md` (project),
  `~/.oxide/SYSTEM.md`, or the corresponding platform config-directory file.
  Append without replacing with `APPEND_SYSTEM.md` in the same locations.
- `--system-prompt <text>` replaces the configured base prompt for one run, and
  `--append-system-prompt <text>` appends to that base (repeatable). A loaded
  `SYSTEM.md` remains the higher-precedence replacement.
- The startup welcome area lists loaded context files, and `/reload` re-reads
  them.

The composed prompt carries a `# Workspaces` section naming the working directory
and the other folders added to Oxide (the desktop's project list,
`desktop/projects.json`), each with its path, so a repository elsewhere on the
machine is found from that list instead of a `find ~` that reads every unrelated
file before the command times out. It says the file tools take absolute paths, so
a file in a sibling project can be read, searched, and edited from the run, while
`bash` still starts in the current project's root. The section is present in every
front-end — the terminal, the desktop app, and the VS Code panel — since it is
composed by the shared core.

## Project trust

Projects may contain local resources that change how the agent behaves or execute
code — agents, commands, prompts, skills, plugins, `SYSTEM.md`, and
`APPEND_SYSTEM.md`. Oxide treats the presence of any of these as requiring trust.
When a project requires trust and no decision has been saved for it (or a parent
directory), the TUI asks before loading them.

- `defaultProjectTrust` in `settings.json` controls the fallback: `ask`
  (default), `always`, or `never`.
- `--approve`/`-a` trusts project resources for one run; `--no-approve` ignores
  them.
- `/trust [show|off]` saves a decision for the current directory to `trust.json`.
- Non-interactive modes (`-p`, `--mode json`, `--mode rpc`) never prompt: with
  the `ask` or `never` setting they ignore project resources unless approved.
- Context files (`AGENTS.md`/`CLAUDE.md`) always load, trusted or not.

## Themes

Oxide ships `dark` and `light` themes. Add a custom theme as JSON under
`.oxide/themes/<name>.json` (project) or `<config>/Oxide/themes/<name>.json`
(global), then select it with `--use-theme <name>` or `/theme <name>`. The
built-in palettes come from `oxide_core::theme_view`, shared with the desktop
app, so the CLI and desktop use identical colors; custom theme files are read by
both. Colors accept names (`cyan`, `lightblue`) or `#rrggbb`; unspecified slots
fall back to the built-in `dark` theme:

```json
{
  "accent": "#5fd7ff",
  "success": "lightgreen",
  "tool": "cyan",
  "error": "lightred",
  "info": "gray",
  "border": "#5fd7ff"
}
```

Available slots: `accent`, `user`, `assistant`, `success`, `tool`, `error`,
`info`, `dim`, `border`, `tool_pending_bg`, `tool_success_bg`, `tool_error_bg`,
`usage_bar_bg`, `usage_bar_fg`, `usage_bar_label`, `thinking_off`,
`thinking_low`, `thinking_medium`, `thinking_high`, `thinking_text`.

Theme slots are semantic: `accent` marks focus and selections, `user` and
`assistant` label speakers, `success` and `error` communicate outcomes, `tool`
marks active tool work, and `info`/`dim` render supporting text. The `tool_*_bg`
slots fill the background behind a tool's header, output, and `Took`/`Elapsed`
footer (pending while running, success or error once it settles). Selection rows
also use reverse video and outcomes include text or symbols, so meaning does not
depend on color alone. For accessible custom themes, keep every foreground
readable against the terminal background and avoid assigning the same color to
`success`, `error`, and `tool`. The built-in themes use fixed `#rrggbb` colors
(not the terminal's own ANSI palette) so they match the desktop app exactly.

## Sessions and context

Sessions use Pi's on-disk format: an append-only JSONL tree whose first line is a
`session` header and whose remaining lines are `message`, `compaction`,
`branch_summary`, `session_info`, `model_change`, and `thinking_level_change`
entries linked by `id`/`parentId`. The last entry is the active leaf; the model
sees the leaf path with the latest compaction applied. Branching moves the leaf
back and appends a `branch_summary`, so in-file alternatives are preserved.

### Context compaction

Oxide compacts Pi-style once the outgoing context approaches the model window. It
walks back from the newest message until `keepRecentTokens` is reached and
summarizes the older span into a structured handoff (goal, progress, decisions,
next steps, critical context, plus cumulative read/modified file lists), keeping
the most recent tokens verbatim. A cut never separates a tool call from its
result. When the cut lands inside a user turn, the request that opened the turn
and the work before the cut are summarized on their own as a split-turn prefix
and merged into the checkpoint, so the request is not lost; a later compaction
updates the previous checkpoint instead of writing a fresh one, and a
summarization response is capped at a fraction of `reserveTokens` so it cannot
spend the whole response budget.

Settings live under `compaction` in `settings.json` (global) or
`.oxide/settings.json` (project):

```json
{
  "compaction": {
    "enabled": true,
    "reserveTokens": 16384,
    "keepRecentTokens": 20000,
    "modelOverrides": { "openai/gpt-4o": { "reserveTokens": 400000 } }
  }
}
```

Compaction triggers above `contextWindow - reserveTokens`, where the window is
`OXIDE_CONTEXT_LIMIT` or the model default. `OXIDE_COMPACTION_ENABLED`,
`OXIDE_COMPACTION_RESERVE_TOKENS`, and `OXIDE_COMPACTION_KEEP_RECENT_TOKENS`
override the file settings. Each compaction is appended to the session log as a
`compaction` entry anchored at `firstKeptEntryId` and replayed on resume, so the
model sees the same compacted view. Manual compaction is `/compact [focus]` in
the TUI or `oxide sessions compact`.

Branching summarizes the path being abandoned with the same structured format and
appends it as a `branch_summary` entry. `/tree <n>` branches the current session
in place (alternatives stay in the file); `/fork <n>` creates a new session
seeded with the summary.

### Queued messages

While the agent is busy, `Enter` steers the response being written: the message
is delivered after the current assistant turn's tool calls, before the next
model call. `Alt+Enter` queues a follow-up, delivered only once the run has
finished. Queued messages are listed above the message box as `Steering: …` /
`Follow-up: …`, and `Alt+Up` (`Alt+Q` on Windows and WSL, `Option+Up` on macOS)
pulls them all back into the box to edit or extend.

`steeringMode` and `followUpMode` in `settings.json` decide how a non-empty
queue is handed over, matching Pi:

- `"one-at-a-time"` (default) — one message per opportunity, so the model
  answers each before the next is delivered.
- `"all"` — the whole queue at once.

The project `.oxide/settings.json` wins per key, and an unknown value keeps the
default.

## Data locations

Runtime state lives under the platform Oxide config directory:

- Main configuration: `config.json`
- Credentials: `auth.json`
- Cached provider model lists: `model-cache.json` (refreshed after 24 hours)
- Remembered model catalogs: `model-catalog.json` (the context windows the
  provider catalogs published, by provider and model; refreshed after four
  hours — see `OXIDE_MODEL_CATALOG` and
  [context windows](#max_tokens-and-context_window))
- MCP OAuth tokens: `mcp-oauth/<server>.json` (mode `0600`)
- Sessions: `sessions/<project>/<timestamp>_<id>.jsonl` (Pi-style entry trees)
- Snapshots: `snapshots/<project>/` (bare git repo)
- Memory: `memory/`
- Project trust: `trust.json`
- Plugins: `plugins/` (installed plugin packages, marketplaces, and state)
- Portkey usage bar: `portkey-usage.json` (mode `0600`; see `OXIDE_USAGE_FILE`)
- Settings: `settings.json` (e.g. `defaultProjectTrust`, `compaction`,
  `modelPrices`, `modelContextWindows`, `modelCatalog`, `steeringMode`,
  `followUpMode`, `hideThinkingBlock`)
- Updates: `updates.json` (the newest release of each component the last launch
  found, so the launch notice needs no network wait; see `OXIDE_UPDATES_FILE`)
- Themes: `themes/<name>.json`
- Desktop projects: `desktop/projects.json` (folders added to the desktop
  sidebar)
- Approvals: `approvals.json` (tools allowed without prompting, per project;
  shared by the terminal, the desktop app and the VS Code extension)
- Truncated tool output: `truncated/` (retained 7 days; see
  `OXIDE_TRUNCATION_DIR`)
- Pasted pictures that named no file: `clipboard/<id>.png` (retained 7 days)
- Context compaction config: `compaction` in `settings.json` /
  `.oxide/settings.json`

Global ecosystem resources such as agents, commands, prompts, skills, plugins,
and MCP definitions may also live under `~/.oxide/`; compatibility resources are
read from `~/.claude/` and `~/.claude.json`.

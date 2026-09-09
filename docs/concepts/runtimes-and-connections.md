# Runtimes and connections

A graph defines the work and its control flow; the runtime plan supplies execution details for each
executable node. Since the two documents are separate, local, self-hosted, and managed targets can
run the same graph.

## Four explicit choices

Each agent binding names:

1. a **harness**, `codex` or `claude`;
2. a **provider** supported by that harness;
3. an opaque provider-owned **model** identifier;
4. zero or more named **connections**, each declaring exact environment field names.

Zeroshot accepts these harness/provider pairs:

| Harness  | Providers                            |
| -------- | ------------------------------------ |
| `codex`  | `openai`, `openrouter`, `bedrock`    |
| `claude` | `anthropic`, `openrouter`, `bedrock` |

Admission rejects the two known-incompatible pairs, `codex` with `anthropic` and `claude` with
`openai`. Zeroshot does not check current provider availability, and model names remain
provider-owned.

## Uniform and exact plans

`--uniform-runtime-config` expands one agent binding across all executable nodes, which keeps a
built-in template configuration short:

```json
{
  "harness": "claude",
  "provider": "anthropic",
  "model": "PROVIDER_MODEL_ID",
  "size": "medium",
  "effort": "high",
  "sessionScope": "execution"
}
```

`--runtime-config` accepts a full plan with one same-named binding per executable graph node. Use it
when reviewers and workers need different models, effort, sessions, or connections. Inspect node
names first with `zeroshot template show TEMPLATE`.

Session scope is either `execution` or `node_instance`. An `execution` scope opens a fresh provider
session for each execution; `node_instance` reuses a live session when the same graph node instance
runs again, such as across loop iterations.

## Runtime configuration contains names, not secret values

A connection maps a stable key to required environment field names:

```json
{
  "connections": {
    "openrouter": ["OPENROUTER_API_KEY"]
  }
}
```

The submission environment or a target-owned connection store supplies the values. Zeroshot keeps
them out of the graph, runtime JSON, run ledger, and observation records.

The uniform runtime defaults are:

| Provider     | Connection key | Fields                                   |
| ------------ | -------------- | ---------------------------------------- |
| `openai`     | `openai`       | `OPENAI_API_KEY`                         |
| `openrouter` | `openrouter`   | `OPENROUTER_API_KEY`                     |
| `anthropic`  | `anthropic`    | `ANTHROPIC_API_KEY`                      |
| `bedrock`    | `bedrock`      | `AWS_BEARER_TOKEN_BEDROCK`, `AWS_REGION` |

Store a local static connection by prompting for its fields:

```console
zeroshot connection set openrouter --field OPENROUTER_API_KEY
zeroshot connection list
```

`connection set` replaces the whole value for that key, so repeat every required `--field` when
updating a multi-field connection. `connection list` reports names and kind but never values.

Hosted targets can also own user- or organization-scoped connections. Setup and dynamic connection
kinds depend on the target; Zeroshot consumes only the resolved fields declared by the runtime.

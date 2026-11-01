# ReviewBench harness for the Zeroshot review graph

This directory runs a general-purpose Zeroshot pull-request review graph on
[ReviewBench](https://github.com/review-bench/ReviewBench) pull requests and scores the findings
with ReviewBench's own judging pipeline.

| Path                                     | Role                                                                                    | Refers to the benchmark? |
| ---------------------------------------- | --------------------------------------------------------------------------------------- | ------------------------ |
| `graph/build_graph.py`, `graph/prompts/` | The review graph and its instructions                                                   | No                       |
| `config/`                                | The generated graph, the runtime plan (models, effort, lanes) and Codex's model catalog | No                       |
| `image/`                                 | Reviewer image: the agent contract adapter and the agents' CLI settings                 | The adapter only         |
| `proxy/egress_proxy.py`                  | Network allowlist for reviewer containers                                               | Harness                  |
| `bench.py`                               | Prepare, run, judge, score and audit                                                    | Harness                  |

## The review graph

```
triage                   Sonnet 5.5   summary of the change, up to 6 review units
discover, in parallel
  per review unit
    A  broad review      Opus 5.5     every changed hunk: correctness, consistency, error handling,
                                      security, performance, interfaces, tests, documentation
    B  depth review      Sol          each changed behavior traced through every input and state
    C  broad review      Sol          A's instructions on the other model family
    D  design review     Opus 5.5     design, duplication, readability, leftovers, conventions, tests
  whole change, one failure class each, with a concrete failing scenario per finding
    logic                Sol
    state, concurrency   Opus 5.5
    contracts            Sol
    error paths, trust   Opus 5.5
    claims against code  Sol          documentation, comments, names and tests that disagree with it
finalize, in parallel    Sol          per unit, plus one agent for files outside every unit: merge
                                      duplicates, try to disprove each finding, re-anchor the rest
done                                  findings: file, line range, severity, title, body
```

Opus 5.5 and Sonnet 5.5 are Claude models run in Claude Code; Sol is GPT-6.1 Sol run in Codex
CLI. The Claude agents use high effort (triage medium). On Sol, reviewer B and the logic and
contracts hunters use high effort; reviewer C, the claims hunter and finalization use medium.
Triage has 4 minutes, each reviewer 13 and each finalizer 8, with two attempts each. Together that
is 25 minutes, inside the adapter's 26-minute deadline, which leaves time to stop the run and
write the findings file within the benchmark's 30 minutes per pull request.

Every agent is a Zeroshot `verifier` with no signals, so it inspects the checkout read-only. A
reviewer that fails or times out does not fail the run: finalization works with the other
reviewers' findings. Zeroshot keeps a map's results only when every item succeeds, so one failed
unit review drops that reviewer role's findings for every unit. The instructions describe ordinary
code review and never mention a benchmark, a golden set, a judge or expected finding counts. No
golden finding, repository name or detail of a benchmark pull request appears in the graph, the
instructions or the runtime plan. `graph/prompts/environment.md` is the one instruction about the
environment: it tells every agent that it has no network access. `python3 graph/build_graph.py`
regenerates `config/review.graph.json` from the prompts.

## Design choices and provenance

The graph was developed by iterating on ReviewBench's public test set of 25 pull requests, using
aggregate scores only; no content of the answer key entered the graph, the instructions or the
runtime plan. Three independent reviews checked the instructions for benchmark-specific content
and for wording that mirrors the judge's prompts, and the sentences they flagged were reworded.

The review favors recall. Reviewers report every real problem they can support, including minor
ones and problems in existing code that the change relies on, with one finding for each place
that needs its own change, and finalization removes only the findings it can disprove.
ReviewBench's grounded metrics count only findings that match its answer key, so additional
findings cost nothing there. Most findings in ReviewBench's answer key were written by Claude- and GPT-family
reviewers, and its judge is a Claude model; the graph uses both families.

## Isolation

The pull requests and their review threads are public, so a reviewer with web access could read
the answers. Each run is isolated the way ReviewBench's own runner isolates it:

- Reviewer containers sit on an internal Docker network (`rb-internal`). The only route out is
  `rb-egress-proxy`, which admits HTTPS to the model APIs (`api.anthropic.com` and
  `api.openai.com`) and logs every request. Codex reads its model metadata from `config/codex-model-catalog.json` (the catalog Codex CLI 0.164.0 was served on 2026-10-31) instead of fetching its catalog from `chatgpt.com` at every start.
- Hosted tools that reach the web through the model host are off: Claude Code denies
  `WebSearch`, `WebFetch` and `RemoteTrigger`, and Codex runs with web search, apps, plugins,
  browser and computer use disabled.
- Checkouts are frozen like the benchmark's: history down to the merge base only, no remotes, and
  no `FETCH_HEAD` or reflog naming the source.
- Each container starts from the image's home directory, so nothing carries over between pull
  requests. Active Memory and its postmortem are off.

`bench.py` starts each container the way ReviewBench's runner does: the contract's three mounts
and variables, the proxy and git's safe directory, nothing else. After the container exits,
`bench.py` copies the agents' home directory out of it, and `bench.py audit` scans the proxy log
and every agent transcript for web, connector and GitHub access. Nothing for the audit runs inside
the image, and the adapter writes only the findings file.

## Credentials

Set `ANTHROPIC_API_KEY` and `OPENAI_API_KEY` in the environment. `bench.py` passes them to each
container by name, never on a command line. Zeroshot hands `OPENAI_API_KEY` to Codex CLI, so no
login file is written. The agents do not see the contract's `RB_*` variables.

The image runs as root, like ReviewBench's example reviewer, so the mounts are writable whoever owns them. Claude Code accepts its permission bypass as root only in a declared sandbox, so the image sets `IS_SANDBOX=1` and the runtime plan passes it to the Claude agents through their `connections`. The judge uses
`ANTHROPIC_API_KEY`.

## Running

Prerequisites: Docker, git, Node.js 20 or later, and a ReviewBench checkout at commit `228f2a8`
with `npm ci` run, at `$RB_WORK/ReviewBench` (default `~/dev/reviewbench-work`). The image is
built on `ghcr.io/the-open-engine/zeroshot-target:10.24.1`.

```bash
cd benchmarks/reviewbench
docker network create --internal rb-internal
docker run -d --name rb-egress-proxy --network bridge -v $PWD/proxy:/proxy:ro \
  -v $RB_WORK/proxy-log:/log --entrypoint python3 ghcr.io/the-open-engine/zeroshot-target:10.24.1 \
  /proxy/egress_proxy.py --log /log/egress.jsonl --allow api.anthropic.com --allow api.openai.com
docker network connect rb-internal rb-egress-proxy
docker build -f image/Dockerfile -t rb-reviewer:dev .

keys=$(python3 bench.py pick --set full --exclude test --count 50 --seed 20261031)
python3 bench.py prepare --set full --keys $keys
python3 bench.py eval --exp sample --keys $keys --rounds 1
```

`eval` runs every pull request in a fresh container limited to 4 CPUs and 14 GB (about
ReviewBench's runner; `RB_CPUS` and `RB_MEMORY` change it), judges the findings, then scores and
audits the run. `RB_PROXY` and `RB_PROXY_LOG` select another proxy and its log.

## Scoring

`bench.py judge` runs ReviewBench's `npm run judge` with the leaderboard's judge model, Claude
Sonnet 5 (`claude-sonnet-5`). On an API key that model accepts only adaptive thinking, so the
judge's model entry is marked adaptive; the judge's prompts are unchanged. `bench.py score`
reports macro-averaged grounded and augmented precision, recall and F1, overall and for High
severity, and the minutes per pull request. Grounded F1 on High severity is the leaderboard's
default view. These are local runs: only runs started from ReviewBench's portal appear on the
leaderboard.

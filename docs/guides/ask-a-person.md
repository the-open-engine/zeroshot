---
description: 'Ask a person, find pending requests and continue a graph with a validated answer or deadline outcome.'
---

# Ask a person

A host-run request verifier records a question and ends its attempt `suspended` with reason
`request`. The controller exits. The next `answer` starts one successor on the retained workspace,
settles the held request, and continues without rerunning earlier nodes or restoring local/direct
workspace files. Agents remain non-interactive.

This graph asks for approval after each worker pass. Rejection sends feedback to the next pass;
approval ends successfully. The graph routes the request's existing `timeout` error to a failure.
The example is an ordinary custom graph; there is no built-in approval template.

```json title="graph.json"
{
  "profile": "openengine.graph.full/v1",
  "initialInput": {
    "kind": "record",
    "fields": {
      "task": {
        "required": true,
        "type": {
          "kind": "string"
        }
      },
      "text": {
        "required": true,
        "type": {
          "kind": "string"
        }
      },
      "feedback": {
        "required": true,
        "type": {
          "kind": "string"
        }
      }
    }
  },
  "policy": {
    "policy": "policy.native-v2@1",
    "default": "deny"
  },
  "root": {
    "kind": "seq",
    "name": "root",
    "state": {
      "kind": "record",
      "fields": {
        "task": {
          "required": true,
          "type": {
            "kind": "string"
          }
        },
        "text": {
          "required": true,
          "type": {
            "kind": "string"
          }
        },
        "feedback": {
          "required": true,
          "type": {
            "kind": "string"
          }
        }
      }
    },
    "children": [
      {
        "kind": "loop",
        "name": "review",
        "state": {
          "kind": "record",
          "fields": {
            "task": {
              "required": true,
              "type": {
                "kind": "string"
              }
            },
            "text": {
              "required": true,
              "type": {
                "kind": "string"
              }
            },
            "feedback": {
              "required": true,
              "type": {
                "kind": "string"
              }
            }
          }
        },
        "body": {
          "kind": "seq",
          "name": "round",
          "state": {
            "kind": "record",
            "fields": {
              "task": {
                "required": true,
                "type": {
                  "kind": "string"
                }
              },
              "text": {
                "required": true,
                "type": {
                  "kind": "string"
                }
              },
              "feedback": {
                "required": true,
                "type": {
                  "kind": "string"
                }
              }
            }
          },
          "children": [
            {
              "kind": "step",
              "name": "worker",
              "worker": "agent.worker@1",
              "instructions": "Implement the task in the shared repository. Address feedback on later passes and run focused checks. Return null.",
              "input": {
                "kind": "record",
                "fields": {
                  "task": {
                    "required": true,
                    "type": {
                      "kind": "string"
                    }
                  },
                  "feedback": {
                    "required": true,
                    "type": {
                      "kind": "string"
                    }
                  }
                }
              },
              "output": {
                "kind": "null"
              },
              "inputBindings": [
                {
                  "target": ["task"],
                  "value": {
                    "source": "state",
                    "path": ["task"]
                  }
                },
                {
                  "target": ["feedback"],
                  "value": {
                    "source": "state",
                    "path": ["feedback"]
                  }
                }
              ],
              "writeBindings": [],
              "attempts": 1
            },
            {
              "kind": "verifier",
              "name": "approval",
              "worker": "builtin.request.ask@1",
              "input": {
                "kind": "record",
                "fields": {
                  "text": {
                    "required": true,
                    "type": {
                      "kind": "string"
                    }
                  }
                }
              },
              "output": {
                "kind": "record",
                "fields": {
                  "feedback": {
                    "required": true,
                    "type": {
                      "kind": "string"
                    }
                  }
                }
              },
              "signals": {
                "decision": ["approved", "rejected"]
              },
              "diagnostic": {
                "kind": "record",
                "fields": {
                  "message": {
                    "required": true,
                    "type": {
                      "kind": "string"
                    }
                  }
                }
              },
              "inputBindings": [
                {
                  "target": ["text"],
                  "value": {
                    "source": "state",
                    "path": ["text"]
                  }
                }
              ],
              "writeBindings": [
                {
                  "value": {
                    "node": "approval",
                    "channel": "out",
                    "path": ["feedback"]
                  },
                  "target": ["feedback"]
                }
              ],
              "attempts": 1
            }
          ],
          "promotedStatePaths": [["feedback"]]
        },
        "until": {
          "kind": "any",
          "guards": [
            {
              "kind": "in",
              "value": {
                "name": "approval",
                "source": "signal",
                "field": "decision"
              },
              "labels": ["approved"]
            },
            {
              "kind": "in",
              "value": {
                "name": "approval",
                "source": "error",
                "field": null
              },
              "labels": ["timeout"]
            }
          ]
        },
        "maxIterations": 20,
        "promotedStatePaths": [["feedback"]]
      },
      {
        "kind": "choice",
        "name": "result",
        "state": {
          "kind": "record",
          "fields": {
            "task": {
              "required": true,
              "type": {
                "kind": "string"
              }
            },
            "text": {
              "required": true,
              "type": {
                "kind": "string"
              }
            },
            "feedback": {
              "required": true,
              "type": {
                "kind": "string"
              }
            }
          }
        },
        "branches": [
          {
            "when": {
              "kind": "in",
              "value": {
                "name": "approval",
                "source": "error",
                "field": null
              },
              "labels": ["timeout"]
            },
            "node": {
              "kind": "fail",
              "name": "expired",
              "reason": "approval_expired"
            }
          },
          {
            "when": {
              "kind": "in",
              "value": {
                "name": "approval",
                "source": "signal",
                "field": "decision"
              },
              "labels": ["approved"]
            },
            "node": {
              "kind": "succeed",
              "name": "approved",
              "output": {
                "kind": "null"
              },
              "bindings": []
            }
          }
        ],
        "otherwise": {
          "kind": "fail",
          "name": "unapproved",
          "reason": "approval_exhausted"
        },
        "promotedStatePaths": []
      }
    ],
    "promotedStatePaths": []
  }
}
```

```json title="runtime.json"
{
  "harness": "codex",
  "provider": "openai",
  "nodes": {
    "worker": {
      "kind": "agent",
      "model": "gpt-5.3-codex",
      "sessionScope": "execution",
      "connections": {}
    },
    "approval": {
      "kind": "request"
    }
  }
}
```

```json title="input.json"
{
  "task": "Handle empty search results.",
  "text": "Approve the change, or reject with feedback.",
  "feedback": ""
}
```

```json title="answer.json"
{
  "signals": {
    "decision": "rejected"
  },
  "output": {
    "feedback": "Handle the empty case."
  }
}
```

Validate the files, then start the graph in your repository:

```bash
zeroshot run --title "Handle empty search results" --graph graph.json --input input.json --runtime-config runtime.json --validate-only
zeroshot run --title "Handle empty search results" --graph graph.json --input input.json --runtime-config runtime.json
zeroshot inbox list --limit 50
zeroshot inbox show RUN_ID
zeroshot answer RUN_ID --input answer.json
```

For approval, change the answer's decision to `approved` and give empty feedback. An answer must
provide every declared signal and every required output field. Unknown fields, signals and labels
are refused as `invalid_request`, naming the path, without consuming the request. A second answer
names the accepted successor. `resume`, including checkpoint selection, refuses requests and names
`answer` before resolving credentials.

Every request has a deadline: `timeoutMs` from creation, 7 days when omitted, at most 30 days.
Acceptance is strictly before the deadline. At or after it, `answer` checks expiry before content,
records an `expiry` vector and starts the `timeout` successor, then exits 1 naming it. Local and
direct targets have no request timer. Inbox reads report overdue requests as `expired` without
changing the suspended run or starting anything.

`inbox list` returns `requests` and optional `nextAfter` in run ID order. Pass that cursor with
`--after`; `--limit` accepts 1 through 100, default 50. Answered and force-stopped requests drop
from the list. `inbox show` retains their `answered`, `expired` or `stopped` disposition and the
successor link. It shows only the question that run made, never an inherited question.

Use `vectors RUN_ID` for bounded summaries and `vectors show RUN_ID ID` for full requests and
answers. Successors inherit the ordered record. `checkpoints` includes entry to the request node.
The run viewer and list display the question, deadline and successor; answering uses the CLI or SDK.

Request nodes use `builtin.request.ask@1` with `{"kind":"request"}`. They must be verifiers with
exactly one required string input `text`, a flat record output of boolean, integer, number, string
or enum fields, the fixed message diagnostic, no instructions and one attempt. They may appear
in sequences, choices and loops, including repeated nodes, but never below parallel or map groups.
Workspace checkpoints must be enabled. Stack profiles cannot contain requests.

Add `--target NAME` to `answer` and either inbox command for a named target. Discovery must advertise
`request_ask: {"kind":"openengine.request-ask/v1"}`. Answers use the same resume route and fresh
connection authorization as `resume`; `--forward-gh-token` has the same meaning. Hosted inbox
routes use the separate `hosted_inbox` extension (`openengine.hosted-inbox/v1`). A target without
those routes refuses inbox reads. Vector reads still require only `openengine.run-vectors/v1`.

The Python SDK exposes `Client.inbox(after, limit)`, `Run.request()` and `Run.answer(answer)`;
`RunStatus.request` carries the question. See the [Python SDK guide](python-sdk.md).

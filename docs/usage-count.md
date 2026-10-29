# Usage count

rozi counts how many machines use it each week, so the project knows how many people it serves
and which platforms and install channels matter. This page lists exactly what is sent, what is
kept, and how to opt out.

## What is sent

Once a week, one rozi client on each machine sends this to `https://usage.tui-lipan.dev`:

| Field | Example | Meaning |
| --- | --- | --- |
| `schema` | `1` | Version of this report format |
| `app` | `rozi` | The program reporting |
| `version` | `0.0.37` | The running rozi release |
| `channel` | `release` | `release` or `nightly` |
| `os` | `linux` | The operating system family |
| `arch` | `x86_64` | The processor architecture |
| `install` | `managed` | How rozi was installed: `managed`, `cargo`, `mise`, `homebrew`, `scoop`, `winget`, `system`, or `unknown` |
| `age` | `2` | How long this machine has been counted: `1` first week, `2` up to four weeks, `3` up to 24 weeks, `4` longer |

Nothing else is sent: no ID, host or user name, path, session, pane content, or configuration.
Because the report carries no ID, the count cannot follow a machine from one week to the next.

## What is kept

The counter keeps one number per week for each combination of the fields above, such as
"week 44, rozi 0.0.37, release, linux, x86_64, managed, age 2: 118 machines". It does not store
the request, your IP address, or the time it arrived. Your address is used only to limit how often
it can send.

The totals are public: <https://usage.tui-lipan.dev/v1/stats?app=rozi>. The counter's source is
[`tools/usage-counter`](../tools/usage-counter/) in the rozi repository.

## When it is sent

The count rides on the [update check](installation.md#update-notifications) rozi already makes:
shortly after a client starts, and every `interval_hours` while it stays open. A machine is counted
at most once per week (Monday to Sunday, UTC), however many clients it runs.

The first time rozi runs on a machine, it sends nothing and shows a notice titled
**Anonymous usage count** instead. The first report goes out on a later day, so you can opt out
before anything is sent.

## Opt out

Any of these keeps a machine out of the count:

- Set `count_usage = false` in [`[updates]`](configuration.md#updates):

  ```toml
  [updates]
  count_usage = false
  ```

- Set `check = false` in `[updates]`. That also stops the update check.
- Set the `DO_NOT_TRACK` environment variable to `1` in the environment rozi starts from.

rozi never counts itself when the `CI` environment variable is set, or when it was built in debug
mode with `cargo run` or `cargo build`.

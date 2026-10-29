# Usage count

rozi counts how many installs are in use each week, so the project knows how many people it
serves and which platforms and install channels matter. This page lists exactly what is sent, what
is kept, and how to opt out.

An install here is one user account on one computer: two people with their own accounts on the
same computer count as two installs.

## What is sent

Once a week, one rozi client for each install sends this to `https://usage.tui-lipan.dev`:

| Field | Example | Meaning |
| --- | --- | --- |
| `schema` | `1` | Version of this report format |
| `app` | `rozi` | The program reporting |
| `version` | `0.0.37` | The running rozi release |
| `channel` | `release` | `release` or `nightly` |
| `os` | `linux` | The operating system family |
| `arch` | `x86_64` | The processor architecture |
| `install` | `managed` | How rozi was installed: `managed`, `cargo`, `mise`, `homebrew`, `scoop`, `winget`, `system`, or `unknown` |
| `age` | `2` | How long this install has been counted: `1` first week, `2` up to four weeks, `3` up to 24 weeks, `4` longer |
| `token` | `9f2c…e41a` | A random number made new for each week's report, so a report sent twice is counted once |
| `week` | `2026-W44` | The week the report is for, so the counter can tell a report that crossed into the next week |

Nothing else is sent: no host or user name, path, session, pane content, or configuration. There
is no install ID. Each week's token is new and unrelated to the last, so the count cannot follow
an install from one week to the next.

## What is kept

The counter keeps one number per week for each combination of the fields above except the token
and week,
such as "week 44, rozi 0.0.37, release, linux, x86_64, managed, age 2: 118 installs". It keeps the
tokens it has seen in a separate list, without the other fields, and deletes each one two weeks
later. It does not store the request, your IP address, or the time it arrived. Your address is
used only to limit how often it can send.

The totals are public: <https://usage.tui-lipan.dev/v1/stats?app=rozi>. The counter's source is
[`tools/usage-counter`](../tools/usage-counter/) in the rozi repository.

## When it is sent

The count rides on the [update check](installation.md#update-notifications) rozi already makes:
shortly after a client starts, and every `interval_hours` while it stays open. An install is
counted at most once per week (Monday to Sunday, UTC, by the counter's clock), however many
clients it runs. On each update check, rozi also asks the counter which week it is, to learn
whether a report is due; that request sends nothing. A report that gets no answer is sent again at
a later check.

The first time rozi runs for an install, it sends nothing and shows a notice titled
**Anonymous usage count** instead. If rozi exits before the notice appears, it shows the notice
the next time it starts. The first report goes out on a later day than the notice, so you can opt
out before anything is sent.

## Opt out

Any of these keeps an install out of the count:

- Set `count_usage = false` in [`[updates]`](configuration.md#updates):

  ```toml
  [updates]
  count_usage = false
  ```

- Set `check = false` in `[updates]`. That also stops the update check.
- Set the `DO_NOT_TRACK` environment variable to `1` in the environment rozi starts from.

rozi never counts itself when the `CI` environment variable is set, or when it was built in debug
mode with `cargo run` or `cargo build`.

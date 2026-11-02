# Test loop

Runs a project's tests whenever an agent finishes a turn. If they fail, it asks the agent to fix
them, up to a limit, and tells you how it ended. It lives in the session, so it keeps working with
no UI attached, on this machine or on a remote host.

It needs `python3` on the host that runs the session.

## Set it up

On the host that runs the session, say which projects it may run in and what to run. Nothing runs
until you do. These settings are not read from the project, so cloning a repository cannot choose a
command for this to run.

```toml
[extensions.test-loop]
roots = ["~/Projects/app"]
command = ["cargo", "test"]
max_attempts = 3
```

| Setting | Default | Meaning |
| --- | --- | --- |
| `roots` | none | Directories it runs tests in. An agent working elsewhere is left alone. |
| `command` | none | The test command, as a program and its arguments. |
| `max_attempts` | `3` | How many times to ask an agent to fix a failure. `0` only reports it. |

Then install the extension where you run `rozi`, and deploy it to a session:

```sh
rozi extensions install --link examples/extensions/test-loop
rozi extensions deploy test-loop --session work
rozi extensions deploy test-loop --session work --remote workbox   # or on another machine
rozi extensions status --session work
```

Redeploy after changing the settings, so the session reads them again.

## What it does

When an agent finishes a turn in one of your `roots`:

- If the tests pass, you get a notification.
- If they fail, the agent gets a one-line request naming the log file, so a test's output is never
  typed into it.
- After `max_attempts` requests, you get one notification with the log's path, and the agent gets no
  more requests until the tests pass.

An agent that is working or waiting for approval is left alone, and its tests run when it stops. If
the agent can't be asked, or the tests can't be run, you get a notification saying why.

## Run the tests yourself

The service uses the `run-tests` action, which you can also call:

```sh
rozi --session work invoke test-loop.run-tests --args-json '{"cwd":"/home/me/Projects/app"}'
```

Only one run happens at a time in a session. Another call fails with `action-busy`, and a run stops
after 5 minutes.

## After a restart

The service notices a turn that ended while it was restarting, and doesn't repeat one it already
handled. A session server restart starts the count again for every agent, because each agent is a
new run. Notifications not yet seen by a UI don't survive a server restart. The latest output for
each project stays in `logs/` in the extension's state directory.

## Test it

```sh
python3 -m unittest discover -s tests
```

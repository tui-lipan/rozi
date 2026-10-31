# Remote reconnect polling

The launch reconnect workload adds about 0.68% of one core on h0p3-z and 1.52 KB/s of TCP
payload across three clients. This meets the proposed budget of about 1% of one core and a few
KB/s, so `remote.reconnect_on_launch` defaults to `true`.

## Setup

- Local host: AMD Ryzen 7 9700X, Linux. Client built with `cargo build --release` from the launch
  reconnect implementation, with reconnect explicitly enabled for the monitored runs.
- Remote host: h0p3-z, Intel Core Ultra 5 338H, Linux, installed rozi 0.0.40. Both ends use the
  existing session protocol 41. No remote upgrade or protocol change was needed.
- Three local client windows, three local named session servers, and three remote named session
  servers. Every named session has one pane with a real, detected Codex process. The agents were
  idle at their initial sign-in screen; no model turns were submitted.
- Each client runs its normal local metadata sweep. The monitored phase adds one independent
  `sessions watch` channel per client. SSH multiplexes these over one shared connection, with
  compression disabled. No shared monitoring daemon was used.
- HOME, config, state, cache, data, Codex home, runtime directories, and working directories are
  isolated under temporary lab roots on each machine. The terminal that holds the three clients
  uses a separate runtime directory, so the local sweep sees exactly the three intended sessions.
- Release checks, usage reporting, notifications, sounds, and animations are disabled in the lab.
  All compilation finished before sampling.

## Method and results

CPU is the change in process user plus system ticks, divided by elapsed monotonic time and the
100 Hz clock tick rate. Percentages refer to one core, without dividing by the machine's core
count. Remote server and monitor processes were identified by the lab environment and their
actual command arguments. Local totals include all three clients and their three session servers.

The baseline ran for 40.36 seconds. Two monitored CPU samples ran for 40.39 seconds each, followed
by a 20.41-second sample with validated TCP counters. The table shows the range of the monitored
samples, rather than hiding their variation.

| Processes | Reconnect disabled | Three monitor channels |
| --- | ---: | ---: |
| Local clients and local session servers | 0.694% | 0.588–0.743% |
| Remote session servers | 2.354% | 2.798–2.841% |
| Remote `sessions watch` processes | 0% | 0.196–0.223% |
| Added remote CPU, including extra server work | 0% | 0.667–0.691% |

The remote server baseline is existing agent detection work, not reconnect overhead. Added CPU
is the monitored server total plus monitor CPU, minus the remote server baseline. The added
remote work is comparable to the existing local sweep's 0.694% total on the local machine;
these are different processors, so this is a budget comparison rather than a speed comparison.

`ss -tinp` supplied cumulative TCP payload counters for the exact lab SSH master PID. Sampling
reset the selected PID at every socket header, including closing sockets, and checked that both
counters increased. Earlier samples with invalid traffic counters were excluded from traffic
results. The final 20.41-second interval spans ten monitor polls per client:

| Direction | All three clients | Mean per client |
| --- | ---: | ---: |
| Sent | 52.9 B/s | 17.6 B/s |
| Received | 1,463.7 B/s | 487.9 B/s |
| Total | 1,516.6 B/s | 505.5 B/s |

These are TCP payload bytes, including SSH framing, rather than Ethernet, IP, or Tailscale
encapsulation overhead. Per-client figures are the mean over the three equal polling channels
on the shared connection. The disabled baseline started no SSH process.

## Live checks

The checks used real clients in server-owned PTYs and the real remote session servers. Captures
were inspected outside the repository and are not committed.

- A connected host opened from Remote hosts wrote reconnect intent. Opening it and choosing a
  session attached normally. The client quit with exit code zero; a fresh bare launch with
  `startup = "last"` resumed that remote session without an SSH or installer dialog.
- With the lab SSH wrapper returning `No route to host`, a fresh launch showed the host tab,
  `Unreachable`, cached sessions marked `Last seen`, and `Ctrl+X`. Restoring the wrapper's real
  SSH transport let all three monitors recover and replace the cached rows. This injected an
  unavailable transport without changing the remote machine's network configuration.
- A separate host entry required password authentication by disabling public-key authentication
  and requesting password authentication. It showed `SSH login rejected`, opened no SSH dialog,
  and stopped after one failed monitor round. The shell-family probe made two SSH invocations;
  the count remained unchanged after the retry interval. Both invocations used `BatchMode=yes`.
- `Ctrl+X` in one window removed durable intent and stopped that window's monitor. The other two
  monitors and their live session lists remained. A fresh fourth window started no monitor for
  the removed host.

The measurement covers three sessions with one agent each and stable metadata. More panes,
published agent rows, or changing project summaries can increase the payload and query work.

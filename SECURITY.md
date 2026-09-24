# Security policy

This policy explains how to report a suspected vulnerability in rozi, what to expect after you
report it, how to test safely, and what is in scope.

## Reporting a vulnerability

Do not open a public issue for a suspected vulnerability. Email
[security@tui-lipan.dev](mailto:security@tui-lipan.dev) so maintainers can investigate before
public disclosure. To encrypt your report, ask for a public key in your first email.

A working exploit is not required. Include enough detail to investigate:

- the plausible attack path and the attacker access it requires;
- the trust boundary the behavior crosses;
- the credible impact on confidentiality, integrity, or availability;
- the affected rozi version, operating system, and installation method;
- steps, commands, configuration, logs, or a minimal proof of concept, when available;
- any mitigations you have already identified.

State your assumptions and any missing evidence. A source-level report is useful when it identifies
a reachable path and a concrete impact, even if you stopped before exploitation.

You can expect:

- acknowledgement within 72 hours;
- an initial assessment within 7 days;
- a fix or mitigation plan within 30 days for a confirmed issue;
- credit in the release notes, unless you ask to remain anonymous.

## Safe testing

Test only systems, accounts, sessions, and data you own or have permission to use. Use isolated test
sessions and synthetic data. Stop once you have demonstrated the minimum access or effect needed to
support the report.

Do not:

- access, retain, or disclose another person's data;
- disrupt shared services or run denial-of-service tests against public infrastructure;
- publish a payload or details that would expose users before a fix is available;
- modify repository releases, tags, signing configuration, update metadata, or public installer
  endpoints;
- leave persistence, active credentials, or a modified installation behind.

If the only way to test would affect a third party or a production service, contact the security
address before testing.

## Scope

This policy covers rozi, its launcher, bundled shell and editor integrations, session and control
protocols, extension integration, and the signed update path published from this repository.

Examples of in-scope issues:

- cross-user access to session or control endpoints;
- authentication, endpoint discovery, or filesystem validation bypasses;
- command injection through pane launch, remote sessions, shell integration, or extension
  execution;
- release signature, manifest verification, update, or rollback bypasses;
- crafted terminal or protocol input that causes code execution, data disclosure, or denial of
  service;
- clipboard or OSC 52 behavior that bypasses configured controls.

An extension, hook, command, or configuration that the user deliberately installed is trusted code,
and malicious behavior contained in it is out of scope. An injection flaw in rozi is still in scope
when untrusted paths, terminal output, protocol data, manifest fields, or other external input can
alter an extension command, its arguments, environment, executable selection, or supervised service
without the user's informed intent.

Issues in upstream dependencies are in scope only when they are reachable through rozi and affect a
rozi trust boundary.

## Supported versions

While rozi is on `0.x.y`, security fixes target the latest minor version only. This policy will
state explicit backport support after `1.0.0`.

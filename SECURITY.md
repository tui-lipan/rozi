# Security policy

## Reporting a vulnerability

Do not open a public issue for a suspected vulnerability. Email
[security@tui-lipan.dev](mailto:security@tui-lipan.dev) so maintainers can investigate before
public disclosure.

A working exploit is not required. Include enough detail to investigate:

- the plausible attack path and required attacker access;
- the trust boundary that the behavior crosses;
- the credible impact on confidentiality, integrity, or availability;
- the affected Rozi version, operating system, and installation method;
- steps, commands, configuration, logs, or a minimal proof of concept when available;
- mitigations you have already identified.

State assumptions and missing evidence. A source-level report is useful when it identifies a
reachable path and concrete impact, even if you stopped before exploitation.

You can expect:

- acknowledgement within 72 hours;
- an initial assessment within 7 days;
- a fix or mitigation plan within 30 days for a confirmed issue;
- release-note credit unless you ask to remain anonymous.

## Safe testing

Test only systems, accounts, sessions, and data you own or have permission to use. Use isolated
test sessions and synthetic data. Stop after demonstrating the minimum access or effect needed to
support the report.

Do not:

- access, retain, or disclose another person's data;
- disrupt shared services or run denial-of-service tests against public infrastructure;
- publish a payload or details that would expose users before a fix is available;
- modify repository releases, tags, signing configuration, update metadata, or public installer
  endpoints;
- leave persistence, active credentials, or a modified installation behind.

Contact the security address before testing when the only available path would affect a third party
or production service.

## Scope

This policy covers Rozi, its launcher, bundled shell and editor integrations, session and control
protocols, extension integration, and the signed update path published from this repository.

Examples include:

- cross-user access to session or control endpoints;
- authentication, endpoint discovery, or filesystem validation bypasses;
- command injection through pane launch, remote sessions, shell integration, or extension
  execution;
- release signature, manifest verification, update, or rollback bypasses;
- crafted terminal or protocol input that causes code execution, data disclosure, or denial of
  service;
- clipboard or OSC52 behavior that bypasses configured controls.

An extension, hook, command, or configuration deliberately installed by the user is trusted code.
Malicious behavior contained in that code is outside this policy. A Rozi injection flaw remains in
scope when untrusted paths, terminal output, protocol data, manifest fields, or other external input
can alter an extension command, arguments, environment, executable selection, or supervised
service without the user's informed intent.

Upstream dependency issues are in scope only when they are reachable through Rozi and affect a Rozi
trust boundary. Reports that lack a complete proof of concept are welcome when they describe a
plausible reachable path and credible impact.

## Supported versions

Before the first public release, security fixes land on `master`. While Rozi is on `0.x.y`,
security fixes target the latest minor version only. This policy will state explicit backport
support after `1.0.0`.

## Encrypted reports

To encrypt a report, ask for a public key in the initial email.

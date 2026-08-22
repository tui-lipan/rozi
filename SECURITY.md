# Security Policy

## Reporting a Vulnerability

If you believe you have found a security vulnerability in **rozi**, please do
not open a public GitHub issue. Report it privately so the issue can be triaged
and patched before disclosure.

**Email:** [security@tui-lipan.dev](mailto:security@tui-lipan.dev)

Please include:

- A description of the vulnerability and its potential impact
- Steps to reproduce, ideally with a minimal configuration or command sequence
- The rozi version, operating system, and installation method where you observed it
- Any suggested mitigations, if you have them

You can expect:

- An acknowledgement within **72 hours**
- A first assessment and triage within **7 days**
- A fix or mitigation plan communicated within **30 days** for confirmed issues
- Credit in the release notes, unless you prefer to remain anonymous

## Scope

This policy covers rozi, its launcher, bundled shell and editor integrations,
the session and control protocols, and the signed update path published from
this repository.

In-scope examples include:

- Cross-user access to session or control endpoints
- Authentication, endpoint-discovery, or filesystem-validation bypasses
- Command injection through pane launch, remote sessions, shell integration, or extensions
- Release signature, manifest verification, update, or rollback bypasses
- Crafted terminal or protocol input that causes code execution, data disclosure, or denial of service
- Clipboard or OSC52 behavior that bypasses its configured controls

Out of scope:

- Malicious commands, extensions, hooks, or configuration deliberately installed by the user
- Issues in upstream dependencies that do not manifest through rozi
- Theoretical issues without a working proof of concept

## Supported Versions

Before the first public release, security fixes land on `master`. While rozi is
on `0.x.y`, security fixes are released against the latest minor version only.
This policy will be updated with explicit backporting support after `1.0.0`.

## GPG / Signed Reports

If you would like to encrypt your report, mention this in your initial email
and we will exchange a public key.

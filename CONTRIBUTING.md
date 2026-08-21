# Contributing to rozi

## License and the DCO

rozi is licensed under **MPL-2.0** (see [LICENSE](LICENSE)).

Contributions follow **inbound = outbound**: unless you state otherwise, any
contribution you intentionally submit for inclusion is licensed under the same
**MPL-2.0** as the project, with no additional terms. You retain the copyright
in your contributions - rozi does **not** ask you to assign copyright or sign a
CLA.

Instead, we use the [Developer Certificate of Origin](DCO) (DCO): a lightweight,
one-line attestation that you wrote the change (or otherwise have the right to
submit it) and agree to license it under MPL-2.0. Sign off each commit by adding
a `Signed-off-by` trailer:

```bash
git commit -s -m "fix: describe the change"
```

This appends a line like:

```
Signed-off-by: Your Name <you@example.com>
```

The name and email must be real and match your Git identity. If you forget,
`git commit --amend -s` (or `git rebase --signoff` for a series) adds it.

> **Why DCO over a CLA?** A CLA could let the project relicense your code under
> proprietary terms later. We deliberately do not ask for that power. The DCO
> records provenance; it does not assign copyright or grant relicensing rights.
> Contributed code remains under MPL-2.0.

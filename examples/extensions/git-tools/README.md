# Git tools extension

This is a complete first-party example implemented only with the public extension manifest,
environment, and `rozi pick` protocol. It provides:

- `git-tools.branches` — switch, create, delete, and refresh local branches;
- `git-tools.worktrees` — inspect worktrees and open one in a focused pane.

It requires Git and Python 3. Copy or clone this directory anywhere below Rozi's user extension
directory, then run:

```bash
rozi check-extension ./git-tools
rozi run-action reload-config
rozi run-action git-tools.branches
```

The installation directory may be renamed; `id = "git-tools"` remains the public identity.

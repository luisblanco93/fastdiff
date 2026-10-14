---
name: fastdiff
description: Install fastdiff or open its desktop viewer to review current Git changes or a text file when the user requests it.
---

Use the permitted shell tool to run terminal `dff` from the active repository, unless the user specifies another directory or text file. The default target is the current directory.

```bash
dff
dff /path/to/repository --file 'src/file with spaces.rs' --view staged
dff notes.txt
dff . --view unpushed
dff . --theme
```

Pass targets and options as separate, quoted arguments; never evaluate user input as shell fragments. `--file` is repository-relative, and `--view` accepts `all`, `unstaged`, `staged`, or `unpushed`. WSL opens the Windows viewer by default; use `--native` only when the user requests the Linux window.

See [usage](https://github.com/luisblanco93/fastdiff/blob/main/docs/usage.md) for theme options, Reflog controls, and shortcuts, and [helper compatibility](https://github.com/luisblanco93/fastdiff/blob/main/docs/architecture.md#helper-boundary) for binary pairing.

Launch stays offline and returns after acknowledgement. Resume the conversation once it returns. Viewing is read-only, includes pre-existing changes, and cannot attribute edits to an agent or send review feedback.

For an explicit app installation request, follow the [installation guide](https://github.com/luisblanco93/fastdiff/blob/main/docs/distribution.md#install-fastdiff) and run its Python 3.11+ command for the user's platform. It downloads the latest published release and installs terminal `dff`; releases are published, so never clone the repository or build from source to install. Only if the installer itself reports that no release exists, report that and build from source solely when the user asks, as described in the README; a source build provides `fastdiff`, which takes the same arguments as `dff`. Report the installer result and any PATH guidance. On failure, report the error and preserve the existing installation.

When the user asks to install this skill, follow the [host-managed skill instructions](https://github.com/luisblanco93/fastdiff/blob/main/docs/distribution.md#agent-skill). Copy the repository's canonical `fastdiff/skills/fastdiff/SKILL.md` to the active host's supported user-level `fastdiff/SKILL.md` skill location. Do not use the app installer for skill installation, edit host settings, or overwrite an existing skill; report conflicts and preserve existing files. Reload the host's skills or restart its session after copying.

Run `dff --update` or `dff --uninstall` only when requested. Close fastdiff windows before uninstalling. If launch fails or `dff` is missing, report the error and refer to the installation guide; do not install or repair automatically.

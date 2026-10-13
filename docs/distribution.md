# fastdiff installation

This is the single installation guide for users and coding agents. Use the released packages: no repository clone, source archive, Rust toolchain, or development tests are needed. If no release has been published yet, the installer cannot succeed; build from source as described in the [README](../README.md#build-from-source) instead. Follow the command block for the current platform; the current installer handles the compiled application independently of app releases.

For an agent handling an app installation request: execute this recipe directly and report the installer result. For a skill-only request, follow [Agent skill](#agent-skill); do not run this app installer. Do not create a separate installation plan or perform repository development work.

**Rename:** Releases up to v0.1.7 shipped as `diffcode`. From the rename onward, the product, binaries, and environment variables are `fastdiff`, `fastdiff-gui`, and `FASTDIFF_*`. The installer removes legacy `diffcode` binaries, and a pre-rename `dff --update` (which passes its `.../diffcode` root) is redirected to the `fastdiff` root so it migrates; there is no compatibility shim for command or binary names. Legacy `DIFFCODE_*` and `DIFFVIEWER_*` environment variables are not read; rename them to `FASTDIFF_*`. The launcher's acceptance acknowledgement is not aliased; keep launcher and GUI from the same release. Installations made before the project moved to `luisblanco93/fastdiff` still update from the previous repository; rerun the installer below once to switch. There is no automatic cross-repository redirect.

## Install fastdiff

Requires Python 3.11+ and HTTPS access to `luisblanco93/fastdiff` and its [releases](https://github.com/luisblanco93/fastdiff/releases). The repository is public, so no GitHub account, token, or GitHub CLI is needed. The installer downloads the latest stable release; until the first release is published, build from source (see the [README](../README.md#build-from-source)). Preserve existing installations on download failure. Supported environments are Linux x64 with glibc and X11/Wayland plus OpenGL, native Windows x64, and WSL2 with Windows interoperability, `powershell.exe`, and `wslpath`. Git is needed only for reviewing repositories.

Run one command for your platform to install terminal `dff`.

### Windows PowerShell

```powershell
py -3 -c "import pathlib, subprocess, sys, tempfile, urllib.request
with tempfile.TemporaryDirectory(prefix='fastdiff-') as stage:
    installer = pathlib.Path(stage) / 'install.py'
    urllib.request.urlretrieve('https://raw.githubusercontent.com/luisblanco93/fastdiff/main/install.py', installer)
    subprocess.run([sys.executable, str(installer), *sys.argv[1:]], check=True)"
```

Open a new terminal after setup, then run `dff` from your project. For the current PowerShell session, use `& "$env:LOCALAPPDATA\fastdiff\bin\dff.cmd"`.

### Linux or WSL2

```bash
python3 -c 'import pathlib, subprocess, sys, tempfile, urllib.request
with tempfile.TemporaryDirectory(prefix="fastdiff-") as stage:
    installer = pathlib.Path(stage) / "install.py"
    urllib.request.urlretrieve("https://raw.githubusercontent.com/luisblanco93/fastdiff/main/install.py", installer)
    subprocess.run([sys.executable, str(installer), *sys.argv[1:]], check=True)'
```

If setup prints PATH guidance, apply it to the current shell:

```bash
export PATH="$HOME/.local/bin:$PATH"
```

Then run `dff` from your project. WSL setup installs the matching Linux helper and Windows app; launching uses the Windows window by default. Use `dff . --native` for a Linux window in a graphical Linux/WSLg session.

### What installation changes

The stable root `install.py` resolves `main` to one commit and downloads the three app runtime modules from `scripts/runtime/`. It downloads the compiled app archives and `SHA256SUMS` directly from the [latest stable release](https://github.com/luisblanco93/fastdiff/releases/latest). Archive validation, checksums, staged installation, rollback, downgrade protection, and Windows/WSL pairing remain enforced.

Installer and skill fixes become available after they are pushed to `main`; they do not require an app version bump or rebuilt binaries. Linux and Windows archives contain binaries and the bundled Material icons license. The canonical agent skill stays in this repository at `skills/fastdiff/SKILL.md`.
| Item | Windows | Linux/WSL |
|---|---|---|
| App and runtime | `%LOCALAPPDATA%\fastdiff` | `${XDG_DATA_HOME:-$HOME/.local/share}/fastdiff` |
| Terminal command | `%LOCALAPPDATA%\fastdiff\bin\dff.cmd` | `~/.local/bin/dff` |

Rerunning the command refreshes the installer/runtime, even if the app version is unchanged. Valid installed binaries are reused without downloading their archives again. Existing installations need one rerun of this installer to adopt this update path.

Setup adds its command directory to Windows user PATH. It does not edit POSIX shell startup files, agent skill directories, settings, or plugin registrations. The terminal command records absolute executable paths. Ordinary launch, help, and version remain offline.

## Agent skill

The single shared [fastdiff skill](../skills/fastdiff/SKILL.md) works through any agent's shell tool. The host agent installs it when asked, following its own skill-directory conventions. Copy `fastdiff/skills/fastdiff/SKILL.md` from the repository checkout to the host's supported user-level `fastdiff/SKILL.md` skill location. For example, Claude Code uses `~/.claude/skills/fastdiff/SKILL.md`; Codex and OpenCode support `~/.agents/skills/fastdiff/SKILL.md`. Other agents should use their documented skill location. Do not edit agent settings or overwrite an existing skill; report a conflict and preserve the user's files. Reload the host's skills or restart its session after installation.

The skill is available in a checkout at `skills/fastdiff/SKILL.md` or from [public file access](https://raw.githubusercontent.com/luisblanco93/fastdiff/main/skills/fastdiff/SKILL.md). Terminal `dff` also works while the agent is idle or closed. The app installer manages only the application and terminal command.

For an older host-specific installation, remove its plugin registration through the agent's settings and remove obsolete `dff-setup` or `review-diff` skill copies before installing the shared skill. Personal agent configuration is preserved by application installation and removal.

## Inspect, update, and remove

```bash
dff --help       # Also --h or -h; offline usage.
dff --version    # Also --v; installed app version, offline.
dff --update     # Fetch the current installer; refresh app and runtime.
# Close fastdiff windows first.
dff --uninstall
```

Use lifecycle flags alone, without a repository or launch options. `dff --update` fetches the current installer from `main`, installs the latest stable app, and refreshes the runtime even when the app version is unchanged. It reuses valid binaries, rejects downgrades, and preserves a usable installation on download or verification failure. It does not modify agent skills.

Uninstall removes recognized app/runtime folders, the receipt, the owned command, and its Windows PATH entry. It preserves unrelated files and agent skills. Locked files, permission errors, ownership conflicts, and symlink/junction roots are reported without killing processes. WSL removes only its Linux installation. If Windows setup was also run, uninstall that installation separately in Windows; removing it can break WSL pairing until setup is rerun.

WSL setup alone creates Windows app files without a Windows receipt or `dff` command. Before uninstalling WSL, note the `windows_gui` path in its `receipt.json`. After closing fastdiff, remove only the corresponding `versions/<version>-windows-<id>` directory under Windows `%LOCALAPPDATA%\fastdiff` if no remaining installation uses it. Preserve separately installed Windows files and PATH entries.

Remove the copied `fastdiff` skill folder separately through your agent's skill controls. Removing the skill leaves terminal `dff` installed.

## Troubleshooting

- **Missing Python:** report the error and stop. Do not substitute source archives.
- **No published release (404 on the latest release):** report that no release exists yet. Build from source only when the user asks, following the [README](../README.md#build-from-source).
- **Network or access failure:** report the error and preserve existing installations.
- **Download, checksum, or package error:** preserve the existing app and configuration; report the failed download or validation.
- **`dff` not found:** open a new Windows terminal, apply POSIX PATH guidance, or use the absolute command from the installed-paths table. Rerun the installation command if the wrapper is missing.
- **Setup required or invalid receipt:** rerun the installation command. Launch does not download or repair automatically.
- **Conflicting terminal command:** preserve the user-owned or locally edited file and resolve the reported collision before rerunning installation.
- **Skill unavailable:** ask the host agent to install the canonical `skills/fastdiff/SKILL.md` in its supported skills directory, then reload its skills. Terminal `dff` remains available independently.
- **Skill conflict:** preserve the existing folder and resolve the conflict using the host agent's skill controls.
- **WSL Windows launch fails:** confirm Windows interoperability, `powershell.exe`, `wslpath`, and the matching managed installation. Use a normal WSL terminal. For manual packages, follow the helper instructions below.

## Manual app archives

For a standalone application without the managed `dff` command, use the compiled `fastdiff-<version>-linux-x64.tar.gz` or `fastdiff-<version>-windows-x64.zip` asset and matching `SHA256SUMS` from the same stable release. Verify the archive before extracting. Keep `fastdiff` and `fastdiff-gui` together in the extracted `bin` directory; Windows names end in `.exe`. Add that directory to PATH and launch `fastdiff`.

Linux packages target x86_64 glibc systems; CI uses Ubuntu 22.04. Windows packages target x86_64; CI and local Windows checks use native MSVC. Manual app archives need neither Python nor GitHub CLI at launch.

See [usage](usage.md) for launch options, including `--theme`.

### Windows and WSL2

Extract the Windows package to Windows-local storage, for example `C:\Tools\fastdiff`. For manual WSL setup, install its paired helper inside each distribution where you review repositories:

```bash
mkdir -p "$HOME/.local/bin"
install -m 755 /mnt/c/Tools/fastdiff/wsl/fastdiff "$HOME/.local/bin/fastdiff"
export PATH="$HOME/.local/bin:$PATH"
export FASTDIFF_WINDOWS_EXE='/mnt/c/Tools/fastdiff/bin/fastdiff-gui.exe'
fastdiff /path/to/project
```

Alternatively, use the matching Linux package's launcher as the helper. Follow [helper compatibility](architecture.md#helper-boundary). The Windows GUI starts the helper through `wsl.exe`; Linux files stay in WSL. Explicit `FASTDIFF_WINDOWS_EXE` takes precedence over PATH lookup. See [WSL interoperability](https://learn.microsoft.com/en-us/windows/wsl/filesystems). These helper-copy steps are for manual archives; the managed installer already pairs the applications.

### Remove a manual archive installation

Close fastdiff, remove the archive's extracted directory and helper copies you installed, and remove only its PATH entry and any `FASTDIFF_WINDOWS_EXE` setting. Preserve unrelated files and agent configuration. Manual archives have no managed receipt or `dff` wrapper, so `dff --uninstall` does not manage them. Clean Windows and each WSL distribution separately.

## Launch this checkout's existing builds

For source development, use existing matching binaries or build them with the [packaging recipes](#produce-packages-locally). Keep the launcher and GUI together. Check each launcher with `--version` and follow [helper compatibility](architecture.md#helper-boundary); never bypass an incompatible-helper error.

### Windows from WSL using the existing debug build

Copy `target/debug/fastdiff.exe` and `fastdiff-gui.exe` to Windows-local storage. Keep the matching Linux helper in WSL and point the Linux launcher at the copied GUI:

```bash
export FASTDIFF_WINDOWS_EXE='/mnt/c/Tools/fastdiff-debug/fastdiff-gui.exe'
./target/debug/fastdiff --version
./target/debug/fastdiff /path/to/repository
```

Check the copied Windows launcher's `--version` from PowerShell too. Launcher exit 0 confirms request acceptance; verify that the Windows window opens and the requested diff loads. Machine-specific retained-build workarounds belong in local agent instructions.

### Release builds

Use verified matching archives or build from source with the recipes below. For a native Linux window in a graphical session:

```bash
./target/linux-release/release/fastdiff --native /path/to/repository
```

See [validation](validation.md) for recorded release checks and their limits.

## Produce packages locally

Build with the committed lockfile. [Cargo's `--locked` option](https://doc.rust-lang.org/cargo/commands/cargo-build.html) rejects dependency resolution changes.

Linux, creating a new release build directory:

```bash
CARGO_TARGET_DIR=target/linux-release cargo build --release --locked --workspace
bash scripts/package-linux.sh target/linux-release/release dist
```

For a Windows GNU cross-build from Linux/WSL, install the Rust Windows GNU target and an x86_64 MinGW toolchain first. With `x86_64-w64-mingw32-gcc` on PATH:

```bash
rustup target add x86_64-pc-windows-gnu
CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc \
  CARGO_TARGET_DIR=target/windows-release \
  cargo build --release --locked --workspace --target x86_64-pc-windows-gnu
```

This produces a Windows pair for a new Windows-local launch. The Windows CI package recipe below uses MSVC.

Windows, from PowerShell after a native MSVC release build:

```powershell
cargo build --release --locked --workspace
./scripts/package-windows.ps1 -WslHelper C:\artifacts\wsl-helper\fastdiff
```

ZIP entries use forward slashes so the installer can safely extract them on either platform.

Application archives contain binaries and the bundled icon license: Linux has `bin/fastdiff` and `bin/fastdiff-gui`; Windows has `bin/fastdiff.exe`, `bin/fastdiff-gui.exe`, and `wsl/fastdiff`. Both include `licenses/material-icons-LICENSE`, copied unchanged from `crates/fastdiff-gui/assets/material/LICENSE`; the icons themselves are embedded in the GUI executable. The canonical agent skill and documentation stay in this repository. Releases contain three assets: the Linux archive, the Windows archive, and `SHA256SUMS`.

The focused packaging checks are `python3 -m unittest discover -s scripts -p 'test_app_package.py'` on Linux and `./scripts/test-package-windows.ps1` on Windows; they check exact archive contents and the bundled license contents using fixtures without compiling the app.

The matching WSL helper must be built on Linux from the same source revision. After checksum verification, the installer creates local `build-info.json` metadata using the version, platform, and helper SHA-256 for pairing. Older archives containing source-revision metadata remain supported. The GitHub Actions workflow tests/builds each platform and produces paired archives; it uploads workflow artifacts and does not publish a release or alter any user configuration.

Compressed-package target: ≤40 MiB per platform. Actual archive sizes and native smoke results belong in [validation](validation.md); packaging alone does not establish the performance targets.

## Maintainer versioning and release

For maintainers preparing a new release, replace `VERSION` with the new stable version and run from the checkout root:

```bash
python3 scripts/bump-version.py VERSION
```

The script checks current metadata consistency and updates the workspace version and its package entries in `Cargo.lock`. It leaves dependency versions alone, restores originals after failed writes, rejects lower versions, and treats repeating the current version as a no-op.

Review the diff and run the release checks, then commit the versioned implementation. Create and push a fresh matching `v<version>` tag to run tagged CI. Do not move or recreate an existing release tag. Tagged GitHub CI builds the two paired binary archives and `SHA256SUMS` as the `release-assets` workflow artifact; it does not publish them. Download and review that matching artifact, then separately publish all three files to the matching stable GitHub Release. Setup availability follows publication of the complete release, not the version bump or tag alone.

After both app archives are in `dist/`, run `python3 scripts/package-release.py --tag "v<version>"`, replacing the placeholder, to prepare checksums for the two binary archives. The script checks workspace and lockfile package versions against the tag. Use a new version for corrections to published releases; preserve old tags and assets.

Installer, skill, and documentation changes published to `main` require no app release. They are maintained in the repository independently of the binary archives.

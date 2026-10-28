# Language servers

A module may name a language server for its languages (the `lsp` section of `module.json`). Go to Declaration,
Find Usages, Rename, completion, the hover popup, Reformat Code and the problems in the code then come from that
server; without one, they come from the text as before. This page is the design: what runs where, how a server is
found or installed, and what is checked.

## Where a server runs

A server runs where the project runs, like its run and check commands:

| Light runs on | Project runs | Started as |
|---|---|---|
| Windows | in a WSL distribution | `wsl.exe -d <distro> --cd <project> -e env sh -lc '<command>' <marker>` |
| Windows | on Windows | `cmd /D /Q /C "<command>"` |
| Linux, macOS | here | `sh -c '<command>'` |

The server talks over its stdin and stdout. Paths are translated for WSL (`E:\p\a.c` is `/mnt/e/p/a.c`). Stopping
asks the server to shut down and waits; what is left is ended by force (the process tree on Windows, the marked
processes inside WSL).

### PATH of a desktop launch

An app started from the macOS Finder or Dock, or from a Linux desktop menu, gets a short PATH
(`/usr/bin:/bin:/usr/sbin:/sbin` on macOS) without Homebrew, npm or the user's own folders. Light started that way
(no `TERM`) asks the user's login shell for its PATH once at start, with a time limit, and puts those folders first.
This serves run commands, checks, the terminal and servers alike.

## Which server serves a language

`lsp` is a list; a language may have several servers, best first. The first that starts serves it. One that is not
installed (the shell says 127, `cmd` says 9009) gives way to the next, and the user is told only when none is there.

A server is looked for in Light's own folder of installed servers first (below), then on PATH: a server the user
installed with apt, Homebrew or by hand works as it is.

Each server says which LSP language id each of its languages has (`jsx` is `javascriptreact`).

## Servers Light installs

A server that is not installed can be installed by Light, at the user's click, into a folder of its own:

- this computer: `<local data>/Light/servers/<server>-<version>/` (`%LOCALAPPDATA%`, `~/Library/Application Support`,
  `~/.local/share`);
- a WSL distribution: `~/.local/share/light/servers/<server>-<version>/` of the project user.

Nothing is installed system-wide, nothing needs administrator rights, and removing the folder removes the server.
A stamp file written last marks an install as complete; a half-finished one is installed again.

Every recipe pins exact versions and checks what it downloads:

| Recipe | Source | Checked by |
|---|---|---|
| `node` runtime | nodejs.org release archive | SHA-256 pinned in the module (the release's `SHASUMS256.txt`) |
| `npm` | registry.npmjs.org | `package-lock.json` shipped in the module, `npm ci` checks every package's SHA-512 |
| `archive` | a release download (GitHub) | SHA-256 pinned in the module, per platform |

`npm ci` runs with `--ignore-scripts`: no package code runs while installing. npm servers run on a Node runtime
Light installs the same way (the current LTS), so they do not depend on the Node the system has (Ubuntu 24.04 has 18,
the servers want 20 or 22).

The module's own files (lockfiles included) are checked against the registry's `index.json` like every other module
file.

### What the user sees

When a file of a language whose servers are all missing is opened, a banner says what would be gained and offers
**Install**. Before anything is downloaded the dialog says what (name, version, source, size). The install runs in
the background with a log, like the Linux developer tools; the server starts when it is done. Servers can be removed
from Settings.

## The servers

| Module | Server | Licence | Recipe |
|---|---|---|---|
| C and C++ | clangd | Apache-2.0 WITH LLVM-exception | system (apt, Homebrew, LLVM installer) |
| Python | basedpyright | MIT | node + npm |
| JavaScript, JSX | typescript-language-server with typescript | Apache-2.0 | node + npm |
| Dockerfile | dockerfile-language-server-nodejs | MIT | node + npm |
| Compose | @microsoft/compose-language-service | MIT | node + npm |
| Assembler | asm-lsp | BSD-2-Clause | archive (Linux x86-64, macOS); elsewhere from PATH |

Light ships none of them: they are downloaded to the user's computer by the user's click, from their publishers.

## What is checked

- Unit tests: the protocol, positions, the answers of servers, recipes and the commands made of them.
- `crates/ide-core/tests/lsp_clangd.rs` against a real clangd (`LIGHT_TEST_CLANGD`, `LIGHT_TEST_CLANGD_WSL`).
- UI tests with a fake server (`tests/fake_lsp.py`): every editor feature, a server that sees only part of the
  project, installs.
- Each server for real, installed by its recipe, in WSL and on Windows, before it goes into a module.
- CI builds and tests on Linux, macOS and Windows.

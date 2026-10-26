# Redistribution materials

Light's original code is MIT licensed. Dependencies, fonts, icons, runtime modules
and language artwork keep their original licenses. Their notices travel with the
binary distribution, not only with this repository.

## Updating dependencies

Run `python tools/license_bundle.py refresh` after changing Cargo.lock. Review and
commit `assets/licenses/rust/`. The catalogue records the exact package versions,
chosen SPDX branches, original notices, their source URLs and SHA-256 hashes.
`AND` conditions are cumulative. Build and CI reject stale catalogues, unknown
licenses, missing files and changed texts. Standard-library notices are taken from
the active Rust toolchain's own copyright material.

Some crate archives omit upstream workspace licenses. The collector retrieves
them from exact upstream revisions. The egui 0.37.0 preparation commit is no
longer public, so its license source is explicitly pinned to the annotated 0.37.0
release's resolved commit. objc2 and dispatch publish MIT declarations rather
than complete MIT texts; their original declarations and author credits are
preserved alongside the standard MIT terms. No copyright owner is invented.

The unmodified `option-ext` MPL source archive is included in `sources/`, verified
against Cargo.lock's checksum. The notices explain how to unpack it. This remains
available even when a target's linker removes that crate's code.

## Runtime and native libraries

Release builds use reviewed SBCL 2.6.9, with a pinned source tarball. Windows uses
that exact Chocolatey/runtime version; Linux and macOS build it into the workspace
using an installed SBCL as bootstrap. Nothing is installed system-wide by the
preparation script. Runtime COPYING, contrib notices, ASDF/UIOP notices and
copyright comment blocks are retained; the complete source archive is included.

Newly copied private macOS libraries require review before release. Zstandard
1.5.7 has its original BSD license and imported xxHash copyright notice. Unknown
library names or versions stop packaging instead of silently omitting notices.

## Portable Git

The complete upstream Portable Git tree is preserved. `tools/git-sources.lock.json`
maps every distinct entry in its package-version list to a checksum-pinned source
package. It also fingerprints the binary tree. Missing, changed or mismatched
sources stop packaging. Refresh the lock with:

```
python tools/git_sources.py refresh path/to/extracted/PortableGit
```

This only reads recipes; it never executes downloaded PKGBUILD scripts. Original
makepkg source packages come from MSYS2 and Git for Windows. The Xpdf UCRT build
did not publish an all-source package, so its source package is reconstructed
from the exact PKGBUILD: its hash must match the binary package's BUILDINFO, and
each original source/patch must match that recipe's hash. The actual pdftotext
binary is compared with the binary package. Provenance and original BUILDINFO
are included in the reconstructed archive.

Every Windows release publishes `Git-<version>-sources.zip` alongside the installer
and portable ZIP, and links it in release notes and in the installed notices.
The source kit includes the complete package materials (including original
licenses, source archives/VCS trees, patches and build recipes), a component
manifest and checksums. Some permissive packages' official makepkg inputs are
prebuilt upstream assets; their original recipe identifies those upstream assets.
Redistributors must preserve access to the matching source kit. Moving only an
installer to another download site does not transfer that responsibility away.

## Independently installed modules

Every module includes LICENSE and THIRD_PARTY_NOTICES.md in its hashed registry
entry. HTML5 also carries CC BY 3.0, W3C attribution and a description of visual
changes. CSS and Markdown identify their CC0 originals; Python identifies PSF
and its permitted freely distributed-tool use. C++ and shell covers use local
typographic labels. Generated template code and grammar are Light's MIT material;
the artwork exceptions are described separately in each module's notices.

## Verification before publishing

```
python -m unittest discover -s tools -p test_licenses.py
python tools/license_bundle.py check
python tools/package.py --installers
python tools/verify_archives.py target/dist
```

LEGAL_MANIFEST.json hashes shipped legal material, source archives and registry
files. The archive verifier extracts the actual produced portable archive and
checks it; the installer smoke test verifies the installed/mounted payload on
each target OS. Private macOS libraries are checked after relocation, before
signing. The release workflow requires these checks before publishing.

A manual release run can specify `release_tag` and enable `publish`; publication
requires all four OS/architecture builds to pass and targets that exact workflow
commit. Without `publish`, manual runs only produce downloadable CI artifacts.

This is a redistribution control, not a guarantee of independent authorship or
legal clearance in every jurisdiction. A new dependency or resource still needs
provenance and license review.

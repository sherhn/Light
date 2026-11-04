# Modules and source builds

The official module sources, artwork, catalog builder and module releases live in [sherhn/Light-Modules](https://github.com/sherhn/Light-Modules). The IDE repository pins an offline snapshot in `modules.lock.json`, including the release URL and SHA-256. Generated `registry/` is ignored by Git.

Before compiling or running tests from a fresh checkout:

```sh
python tools/prepare_modules.py
cargo build --workspace --locked
```

The same preparation runs in CI and before packaging. It verifies the snapshot and every indexed file before replacing the existing local registry. You can supply a previously downloaded snapshot with `--archive /path/to/light-modules-1.0.0.zip`; the pinned checksum is still required. Installers include these modules for offline installation, and the Lisp module remains available from the first launch.

The online catalog is independent of the IDE release. Catalog entries specify `min_ide`; incompatible packages cannot be installed. A module update downloads an HTTPS ZIP from a versioned release, verifies its size and SHA-256, validates all file paths and hashes, and checks the manifest id/version before replacing the installed version. Failed downloads or validation preserve the installed module. Legacy registries containing loose files remain supported.

To update the bundled modules, publish and verify a new Light-Modules catalog first, update `modules.lock.json` to its versioned snapshot and SHA-256, run preparation, then run Rust and packaging tests before publishing the IDE.

## Your own registry

Use the manifests in Light-Modules as examples. A registry is an `index.json` plus the `modules/<id>/` folders listed in it. Each entry lists file names, sizes and SHA-256 values. Version 2 can additionally specify `min_ide` and an `archive` object with `url`, `size` and `sha256`. Archives contain the module files directly at the root, with no parent folder, links or extra files. Include the module's license and any third-party notices.

In Settings > Modules, enter the HTTPS registry address or choose a local registry folder. `LIGHT_REGISTRY` can override that source for testing. Changing sources does not remove installed modules. Modules can define shell commands, so choose a source you trust.

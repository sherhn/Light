# Updates

Light checks for stable IDE and official module updates in the background, six hours apart. Downloads do not block editing. Settings → Updates controls IDE and module downloads independently; each module can also be excluded from automatic updates in its menu.

The first version with this updater must be installed manually. Older releases have no background update client.

## Activation and recovery

- Windows installations, portable bundles and macOS applications activate a prepared update after all full IDE windows using that installation have closed. Nothing forces an editor restart. A busy installation is left untouched and retried after a later close.
- Debian/Ubuntu installations download a signed `.deb` in the background and show its location. Installation goes through the system package manager, with its usual permissions. Light never replaces files owned by dpkg itself.
- Official modules activate before modules are loaded at the next full IDE start. Disabled modules stay disabled. Another IDE instance using the same store postpones activation.
- Source builds, locally modified program files, modified modules and custom registries are excluded from automatic replacement. A new program file that conflicts with a user-added file also stops replacement.

Packages are downloaded completely before activation. Ed25519 signatures, platform, stable version, expiry, SHA-256, sizes and archive paths are checked. Prepared files are checked against the signed archive again immediately before activation. The signing key lives in encrypted release-workflow secrets; clients contain only the public verification key.

The new executable passes an isolated startup check before and after replacement. A failed check restores the original directory. The previous installation remains in a sibling `.Light.previous-VERSION` directory (the name follows the installation folder); settings, projects, notes and module state remain outside program replacement. User-added files inside the program folder are preserved. Interrupted swaps retain their old-directory backup; if an operating-system or power failure leaves the installation path unavailable, restore that backup or run the installer again. No project data needs to be removed.

Settings → Updates offers **Restore previous IDE version after closing**. A manually rejected version is not automatically reinstalled; **Download update now** explicitly allows it again. A module's menu offers **Restore previous version**, applied on the next start, and pauses automatic updates for that module. Backup directories are retained for recovery and can be removed manually when no longer needed.

Quick View performs no update checks, downloads or module initialization. Release self-checks run before normal application setup and use an isolated configuration directory.

Errors remain fully selectable and copyable in Settings → Updates. Network failures leave the installed version usable. The updater does not run unsigned packages or silently elevate permissions.

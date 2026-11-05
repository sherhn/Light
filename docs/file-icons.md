# File icons

Project trees, editor tabs, search results, the file browser, New File and Quick View use the same SVG file marks. The catalog covers the pinned module languages, their filename patterns (Dockerfile, Makefile, `.env.*`, Compose, etc.) and common source/text files. Recognizing a file icon does not install or enable a module. Unknown custom languages use a code icon when their language module is enabled.

Settings → Appearance → File icons offers Colored (the default), Monochrome and Standard. Monochrome keeps each language's shape in a neutral color appropriate to the theme; Standard uses the normal document icon for all files. The preference is applied live and saved, including for Quick View. Folder and tool-window icons are unchanged.

On Windows, the installer registers individual icon families and all supported extensions in Open With and Default Apps. Existing default applications and protected UserChoice settings are preserved. To use Light's file icons in Explorer, choose Light for those file types in Windows Default Apps / Open With. Existing associations with the legacy shared `Light.Source` type keep its shared application icon until a per-type association is chosen.

Windows file-type registration belongs to the installer. Portable copies, background updates and Quick View do not rewrite system associations. To add newly supported Open With types to an existing Windows installation, run its updated installer. Icons inside the IDE update with the application.

The catalog is generated from the pinned registry with `python tools/file_types.py`; `--check` verifies coverage. After editing the SVG mark set, rebuild the native multi-size ICOs with `cargo run -p light --example export_file_icons`. The original SVG licenses accompany packaged icons under `licenses/module-marks`.

# Path completion

Press Tab in path fields or the Terminal's Commands input to insert the first matching name, sorted alphabetically. Directories get a trailing separator; directory pickers offer directories only. Spaces in literal fields stay literal. Command arguments are quoted when necessary. Shift+Tab still goes to the previous field; a field with no match advances to the next control.

The browser's address can be edited with Ctrl+L. Relative addresses are resolved from its displayed folder. Windows paths, forward slashes, Unicode names, home paths and absolute Linux paths in WSL are supported. The Linux shell terminal forwards Tab to the shell, preserving its native completion and terminal applications.

Directory reads and WSL home lookup run on a worker. Results are discarded if the text, caret, selection, destination or focus changed, or the dialog was closed. A completion never executes a command or writes a file.

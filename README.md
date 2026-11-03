# Light

Light is an open-source modular IDE built with Rust and egui for Windows, macOS and Linux.
Add the languages and tools your project needs.

[Website](https://getlight.sherhn.space) · [Downloads](https://github.com/sherhn/Light/releases) · [Issues](https://github.com/sherhn/Light/issues)

## Features

- Language modules: JavaScript/JSX, JSON/JSONC, TOML, YAML, INI, Environment (.env), Python, C/C++, Lisp, Assembler, HTML, CSS, Markdown, reStructuredText, Shell and Make; Dockerfile and Compose tools; an HTTP client for `.http` files.
- Code highlighting, completion, navigation and project search.
- Integrated Git: merge conflict resolution, blame, file history, stashes, cherry-pick, branch comparison and the interactive rebase; terminal and run scenarios.
- Code coverage in the gutter, a CPU profiler with a flame graph, and a Services window for running programs and Docker containers.
- A Tasks window for package.json scripts and make targets; run buttons for Vitest, Jest and node:test tests.
- A debugger for Node.js (breakpoints with conditions and logpoints, stepping, variables, a console), set up on first use, and GDB for C and C++ with disassembly and memory.
- C and C++: CMake tasks and presets, GoogleTest, Catch2 and doctest tests through CTest, AddressSanitizer and Valgrind runs.
- Lisp: errors stop in a debugger with restarts and the frames' variables; an inspector, who calls and trace.
- Linux through WSL on Windows.
- Local history, document previews (Markdown, reStructuredText, SVG), an image viewer (PNG, JPEG, GIF, BMP, WebP, ICO) and customizable themes.

Modules are installed inside the IDE. Compilers and interpreters are configured separately.

Licensed under [MIT](LICENSE).
Third-party components retain their own licenses. Downloads include full notices,
runtime/MPL sources, and a separate matching source kit for bundled Git; see [redistribution details](docs/licensing.md).

## Русский

Light — открытая модульная IDE на Rust и egui для Windows, macOS и Linux.
Подключайте нужные языки и инструменты под свой проект.

[Сайт](https://getlight.sherhn.space) · [Сборки](https://github.com/sherhn/Light/releases) · [Обратная связь](https://github.com/sherhn/Light/issues)

### Возможности

- Языковые модули: JavaScript/JSX, JSON/JSONC, TOML, YAML, INI, Environment (.env), Python, C/C++, Lisp, Assembler, HTML, CSS, Markdown, reStructuredText, Shell и Make; инструменты Dockerfile и Compose; HTTP-клиент для файлов `.http`.
- Подсветка кода, автодополнение, навигация и поиск по проекту.
- Встроенный Git: разрешение конфликтов, blame, история файла, stash, cherry-pick, сравнение веток и интерактивный rebase; терминал и сценарии запуска.
- Покрытие кода у строк, профилировщик с flame graph и окно «Сервисы» для запущенных программ и Docker-контейнеров.
- Окно задач для скриптов package.json и целей make; кнопки запуска тестов Vitest, Jest и node:test.
- Отладчик Node.js (точки останова с условиями и logpoints, шаги, переменные, консоль), который настраивается при первом запуске, и GDB для C и C++ с дизассемблером и памятью.
- C и C++: задачи и пресеты CMake, тесты GoogleTest, Catch2 и doctest через CTest, запуск с AddressSanitizer и под Valgrind.
- Lisp: ошибка останавливается в отладчике с рестартами и переменными кадров; инспектор, «кто вызывает» и trace.
- Linux через WSL при работе в Windows.
- Локальная история, предпросмотр документов (Markdown, reStructuredText, SVG), просмотр изображений (PNG, JPEG, GIF, BMP, WebP, ICO) и настраиваемые темы.

Модули устанавливаются внутри IDE. Компиляторы и интерпретаторы настраиваются отдельно.

Лицензия — [MIT](LICENSE).
Сторонние компоненты сохраняют собственные лицензии. Сборки содержат уведомления,
исходники runtime/MPL-компонентов; рядом публикуется соответствующий исходный комплект Git.
Подробнее — [в описании распространения](docs/licensing.md).

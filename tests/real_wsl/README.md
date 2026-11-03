# Scripts for the real WSL

These UI scripts need a real Ubuntu-24.04 distribution with gcc, g++, cmake and gdb 14 or newer, and a project the
scripts name; `tests/run_e2e.py` does not run them (its WSL is a fake). Run one by hand:

```
LIGHT_CONFIG_DIR=E:/light-e2e-tmp/cfg LIGHT_REGISTRY=E:/Projects/Cds/registry LIGHT_PRELOAD_MODULES=lisp,c-cpp \
  target/debug/light.exe --script tests/real_wsl/cpp_tools.json --size 1280x800 --no-restore
```

`cpp_tools.json` expects `E:/light-research/cpp-e2e`: a CMake project with `main.cpp` (`add`, `main`), `calc_test.cpp`
(`TEST(Math, Adds)` passing, `TEST(Math, Fails)` failing) built with GoogleTest from FetchContent and `gtest_discover_tests`.

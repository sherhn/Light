from common import call, match, module, outline, region, words

C_KEYWORDS = """if else for while do switch case default break continue return goto sizeof typedef struct union enum static extern const
volatile inline register restrict auto asm __asm__ __attribute__ __volatile__ __inline__ _Alignas _Alignof _Atomic _Generic _Noreturn
_Static_assert _Thread_local alignas alignof""".split()

CPP_KEYWORDS = """class namespace template typename public private protected virtual override final new delete this try catch throw using
operator friend explicit constexpr consteval constinit noexcept static_assert decltype mutable export concept requires co_await co_return
co_yield dynamic_cast static_cast reinterpret_cast const_cast typeid""".split()

TYPES = """void char short int long float double signed unsigned bool _Bool _Complex size_t ssize_t ptrdiff_t intptr_t uintptr_t intmax_t
uintmax_t int8_t int16_t int32_t int64_t uint8_t uint16_t uint32_t uint64_t wchar_t char8_t char16_t char32_t FILE va_list off_t time_t
string vector map set unique_ptr shared_ptr""".split()

CONSTANTS = "NULL true false nullptr EOF stdin stdout stderr __FILE__ __LINE__ __func__ __cplusplus".split()


def grammar(cpp):
    kw = C_KEYWORDS + (CPP_KEYWORDS if cpp else [])
    return [
        region(r"/\*", r"\*/", "comment"),
        match(r"//.*$", "comment"),
        match(r'(?:u8|[LuU])?"(?:[^"\\\n]|\\.)*"?', "string"),
        match(r"(?:u8|[LuU])?'(?:[^'\\\n]|\\.)*'?", "char"),
        # `#include <header>`: the directive and the header in two colours
        match(r"^[ \t]*(#[ \t]*include(?:_next)?)[ \t]*(<[^>\n]*>)?", captures={1: "preproc", 2: "string"}),
        match(r"^[ \t]*#[ \t]*(?:define|undef|if|ifdef|ifndef|else|elif|elifdef|elifndef|endif|error|warning|pragma|line|import)\b", "preproc"),
        match(r"\b(?:0[xX][0-9a-fA-F']+|0[bB][01']+|\d[\d']*(?:\.\d*)?(?:[eE][+-]?\d+)?|\.\d+(?:[eE][+-]?\d+)?)[uUlLfF]*\b", "number"),
        words(kw, "keyword"),
        words(TYPES, "type"),
        words(CONSTANTS, "constant"),
        call(),
    ]


FUNCTION_DEF = r"^[A-Za-z_][\w \t\*&:<>~,]*?\b(\w+)[ \t]*\([^;(){}]*\)\s*(?:const\s*)?(?:noexcept\s*)?\{"
OUTLINE = [
    outline(FUNCTION_DEF, "function"),
    outline(r"^(?:typedef[ \t]+)?(?:struct|union)[ \t]+(\w+)", "struct"),
    outline(r"^(?:typedef[ \t]+)?enum(?:[ \t]+class)?[ \t]+(\w+)", "type"),
    outline(r"^[ \t]*(?:class|namespace)[ \t]+(\w+)", "class"),
    outline(r"^[ \t]*#[ \t]*define[ \t]+(\w+)", "macro"),
]

COMMON = dict(
    line_comment="//",
    block_comment=["/*", "*/"],
    brackets=["()", "[]", "{}"],
    check=["brackets"],
    indent={"mode": "braces", "size": 4},
    fold="braces",
    outline=OUTLINE,
)

MODULE = module(
    version="1.1.1",
    id="c-cpp",
    name="C and C++",
    category="Languages",
    summary="Highlighting, brackets, folding and an outline for C, C++ and headers; compile and run a file; the compilers and debuggers in Linux.",
    description="C and C++ for Light: syntax highlighting with preprocessor and types, matching brackets, folding of blocks, the outline of functions, structs and macros, Enter after a brace indents, and a command to compile and run the current file. The Linux side brings gcc, g++, make, cmake, ninja, gdb, clang, clangd, valgrind and friends - installed from inside the IDE.",
    languages=[
        dict(id="c", name="C", extensions=["c", "h", "ld", "lds"], filenames=[], grammar=grammar(False), **COMMON),
        dict(id="cpp", name="C++", extensions=["cc", "cpp", "cxx", "c++", "hpp", "hh", "hxx", "ipp", "inl", "tpp"], filenames=[], grammar=grammar(True), **COMMON),
    ],
    run=[
        dict(language="c", name="Compile and run file", command='cc -g -Wall -Wextra "{file}" -o "{dir}/{stem}" && "{dir}/{stem}"', windows='gcc -g -Wall -Wextra "{file}" -o "{dir}/{stem}.exe" && "{dir}/{stem}.exe"'),
        dict(language="cpp", name="Compile and run file", command='c++ -g -Wall -Wextra -std=c++17 "{file}" -o "{dir}/{stem}" && "{dir}/{stem}"', windows='g++ -g -Wall -Wextra -std=c++17 "{file}" -o "{dir}/{stem}.exe" && "{dir}/{stem}.exe"'),
    ],
    tools=[
        dict(name="C and C++", required=["make", "gcc"], programs=["make", "gcc", "g++", "ld", "gdb", "cmake", "ninja", "pkg-config"],
             apt="build-essential gdb cmake ninja-build pkg-config autoconf automake libtool ccache bear gcc-multilib g++-multilib"),
        dict(name="Clang and LLVM", programs=["clang", "clangd", "lldb", "clang-format"], apt="clang clangd lldb clang-format clang-tidy"),
        dict(name="Debugging and analysis", programs=["valgrind", "strace", "ltrace", "gdb-multiarch", "xxd"], apt="valgrind strace ltrace gdb-multiarch xxd"),
    ],
    detect=dict(extensions=["c", "h", "cc", "cpp", "cxx", "hpp", "hh", "hxx"], files=["CMakeLists.txt", "meson.build", "Makefile", "makefile", "GNUmakefile"]),
    templates=[
        dict(name="C program", description="main.c and a makefile", open="main.c",
             files={"main.c": '#include <stdio.h>\n\nint main(void) {\n    printf("Hello, Light\\n");\n    return 0;\n}\n',
                    "makefile": 'CFLAGS = -g -Wall -Wextra\n\nall: main\n\nmain: main.c\n\t$(CC) $(CFLAGS) main.c -o main\n\nrun: main\n\t./main\n\nclean:\n\trm -f main\n\n.PHONY: all run clean\n'}),
        dict(name="C++ program", description="main.cpp and a CMake project", open="main.cpp",
             files={"main.cpp": '#include <iostream>\n\nint main() {\n    std::cout << "Hello, Light\\n";\n}\n',
                    "CMakeLists.txt": 'cmake_minimum_required(VERSION 3.16)\nproject(hello CXX)\nset(CMAKE_CXX_STANDARD 17)\nadd_executable(hello main.cpp)\n'}),
    ],
)

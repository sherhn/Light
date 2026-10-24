from common import match, module, outline, region, words

REGS32 = "eax ebx ecx edx esi edi esp ebp eip eflags".split()
REGS16 = "ax bx cx dx si di sp bp ip al ah bl bh cl ch dl dh cs ds es fs gs ss".split()
REGS64 = ["rax", "rbx", "rcx", "rdx", "rsi", "rdi", "rsp", "rbp", "rip", "rflags"] + [f"r{n}{s}" for n in range(8, 16) for s in ("", "d", "w", "b")]
REGS_SPECIAL = ["cr0", "cr2", "cr3", "cr4", "dr0", "dr1", "dr2", "dr3", "dr6", "dr7", "gdtr", "idtr", "ldtr", "tr", "st0", "st1", "st2", "st3", "st4", "st5", "st6", "st7"]
SIMD = [r"[xyz]mm\d{1,2}", r"mm[0-7]"]

MNEMONICS = """mov movzx movsx movs movsb movsw movsd movsq lea push pop pusha popa pushad popad pushf popf pushfd popfd xchg add adc sub sbb mul imul
div idiv inc dec neg not and or xor test cmp shl shr sal sar rol ror rcl rcr jmp je jne jz jnz ja jae jb jbe jg jge jl jle jc jnc js jns jo
jno jp jnp jecxz loop loope loopne call ret retf iret iretd int into cli sti hlt nop in out ins outs insb insw insd outsb outsw outsd
cld std lodsb lodsw lodsd stosb stosw stosd scasb scasw scasd cmpsb cmpsw cmpsd rep repe repne repz repnz lgdt lidt lldt ltr sgdt sidt
cpuid rdtsc rdmsr wrmsr invlpg wbinvd syscall sysret sysenter sysexit bt bts btr btc bsf bsr cbw cwd cwde cdq cdqe cqo setcc cmov cmove
cmovne cmovz cmovnz cmova cmovb leave enter xlat lahf sahf cmc clc stc fld fst fstp fadd fsub fmul fdiv fxch fcom fcomp fchs fabs fsqrt
fild fist fistp fldz fld1 finit fwait fninit ud2 pause lfence sfence mfence prefetch movaps movups movdqa movdqu movd movq paddb paddw
paddd paddq psubb psubw psubd psubq pxor pand por pcmpeqb pshufd addps addss subps subss mulps mulss divps divss sqrtps cvtsi2ss cvttss2si
xadd cmpxchg bswap""".split()

DIRECTIVES = """section segment global extern common bits org default cpu use16 use32 use64 db dw dd dq dt do dy dz resb resw resd resq rest reso
resy resz equ times incbin align alignb struc endstruc istruc iend at absolute static required import export group uppercase""".split()

SIZES = "byte word dword qword tword oword yword zword ptr short near far strict nosplit".split()

NASM_NUMBER = r"\b(?:0[xX][0-9a-fA-F_]+|[0-9][0-9a-fA-F_]*[hH]|0[bB][01_]+|[01_]+[bB]|0[oO][0-7_]+|\d[\d_]*(?:\.\d*)?(?:[eE][+-]?\d+)?)\b|\$[0-9][0-9a-fA-F]*\b"

GAS_REGS = r"%[a-z][a-z0-9]*"

MODULE = module(
    id="assembler",
    name="Assembler",
    category="Languages",
    summary="NASM and GNU assembler: instructions, registers, directives, labels and an outline of labels; nasm and QEMU in Linux.",
    description="x86 assembly for Light, in Intel (NASM) and AT&T (GNU as) syntax: mnemonics, registers, directives, labels, numbers in every base, strings and comments, with an outline of the labels. The Linux side brings nasm, QEMU, xorriso and the GRUB tools for writing an operating system - installed from inside the IDE.",
    languages=[
        dict(
            id="nasm", name="Assembly (NASM)", extensions=["asm", "nasm", "inc", "fasm"], filenames=[],
            line_comment=";", brackets=["[]", "()"], indent={"mode": "keep", "size": 8}, fold="none",
            grammar=[
                match(r";.*$", "comment"),
                match(r'"[^"\n]*"?|\'[^\'\n]*\'?|`(?:[^`\\\n]|\\.)*`?', "string"),
                match(r"^[ \t]*%[A-Za-z_]\w*", "preproc"),
                match(r"^[ \t]*([A-Za-z_.$?@][\w.$?@#~]*)[ \t]*:", captures={1: "label"}, consume=1),
                words(DIRECTIVES, "preproc", ignore_case=True),
                match(r"\b(?:" + "|".join(SIMD) + r")\b", "register", ignore_case=True),
                words(REGS32 + REGS16 + REGS64 + REGS_SPECIAL, "register", ignore_case=True),
                words(SIZES, "type", ignore_case=True),
                words(MNEMONICS, "keyword", ignore_case=True),
                match(NASM_NUMBER, "number"),
            ],
            outline=[outline(r"^([A-Za-z_][\w$?@#~]*):", "function")],
        ),
        dict(
            id="gas", name="Assembly (GNU)", extensions=["s"], filenames=[],
            line_comment="#", block_comment=["/*", "*/"], brackets=["()"], indent={"mode": "keep", "size": 8}, fold="none",
            grammar=[
                region(r"/\*", r"\*/", "comment"),
                match(r"(?://|#).*$", "comment"),
                match(r'"(?:[^"\\\n]|\\.)*"?', "string"),
                match(r"^[ \t]*(\.[A-Za-z_]\w*)", captures={1: "preproc"}),
                match(r"^[ \t]*([A-Za-z_.$][\w.$]*):", captures={1: "label"}, consume=1),
                match(GAS_REGS, "register"),
                match(r"\$-?(?:0[xX][0-9a-fA-F]+|\d+|[A-Za-z_.][\w.]*)", "number"),
                words(MNEMONICS + [m + s for m in ("mov", "add", "sub", "cmp", "push", "pop", "lea", "and", "or", "xor", "test", "inc", "dec", "shl", "shr") for s in ("b", "w", "l", "q")], "keyword"),
                match(r"\b(?:0[xX][0-9a-fA-F]+|\d+)\b", "number"),
            ],
            outline=[outline(r"^([A-Za-z_][\w$]*):", "function")],
        ),
    ],
    run=[
        dict(language="nasm", name="Assemble (ELF32)", command='nasm -f elf32 -g "{file}" -o "{dir}/{stem}.o"'),
        dict(language="gas", name="Assemble", command='as --32 -g "{file}" -o "{dir}/{stem}.o"'),
    ],
    tools=[dict(name="OS development", programs=["nasm", "qemu-system-i386", "qemu-system-x86_64", "qemu-img", "xorriso", "grub-mkrescue"],
                apt="nasm qemu-system-x86 qemu-utils xorriso mtools grub-pc-bin")],
    detect=dict(extensions=["asm", "nasm", "s"], files=[]),
    templates=[
        dict(name="Boot sector", description="a 512-byte sector for QEMU", open="boot.asm",
             files={"boot.asm": "; nasm -f bin boot.asm -o boot.bin && qemu-system-i386 -drive format=raw,file=boot.bin\n[org 0x7c00]\nbits 16\n\nstart:\n    mov si, message\n.next:\n    lodsb\n    or al, al\n    jz .done\n    mov ah, 0x0e\n    int 0x10\n    jmp .next\n.done:\n    hlt\n    jmp .done\n\nmessage db \"Hello, Light\", 13, 10, 0\n\ntimes 510 - ($ - $$) db 0\ndw 0xaa55\n"}),
    ],
)

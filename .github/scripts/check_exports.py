"""Checks that every binding declares exactly the C ABI the core exports.

The export list is read from the core's source: every function the `export!` macro declares in
rust/src/kernel/exports.rs (the rule .github/hyper.json gives the Forge), less
`hypertabular_canary`, the no-panic proof's negative control, which no binding calls; cbindgen
generates include/hypertabular.h from the same file. Each binding's declarations are then read
from its own source by the patterns in SITES, and any difference fails the check: a missing
export means a call nobody added to that binding, and an extra one means the binding still
declares an export the core dropped or renamed. Both of those otherwise surface late, in one
binding's suite on whichever leg makes the call.

The Python, Ruby (Magnus) and TypeScript extensions are not C ABI consumers: they link the core
as a Rust crate and call its Rust API, so they have no site; napi_catch_unwind holds every
TypeScript entry point to `catch_unwind`. The PHP extension (rust/src/php_ext/) calls the Rust
API too, and php_guard holds every PHP-callable function in it to `guard(`, and php_link its extern blocks to the php8/php8ts pair.

No dependencies and no build: CI runs it in core-checks, and so can a dev loop.

usage: python .github/scripts/check_exports.py
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

CORE = ["rust/src/kernel/exports.rs"]
VERSION = "hypertabular_version"
CANARY = "hypertabular_canary"


def core_exports():
    names = set()
    for path in CORE:
        text = (ROOT / path).read_text(encoding="utf-8")
        names |= set(re.findall(r"\bfn (hypertabular_\w+)\(", text))
    return names - {CANARY}


def napi_catch_unwind(path):
    """Every napi function, getter and factory must carry `catch_unwind`: without it a panic
    unwinds into Node-API and aborts the process instead of throwing. The attribute may span
    lines and may sit in a macro_rules! template; attributes above consts, structs, enums and
    impls are not functions and are skipped."""
    path = Path(path)
    if not path.is_absolute():
        path = ROOT / path
    text = path.read_text(encoding="utf-8")
    bad, functions = [], 0
    for match in re.finditer(r"#\[napi(\((?:[^()]|\([^()]*\))*\))?\]", text):
        rest = text[match.end() :]
        item = re.match(r"(?:\s*#\[[^\]]*\])*\s*(?:pub(?:\([^)]*\))? )?(?:const |unsafe |async )*fn ([\w$]+)", rest)
        if not item:
            continue
        functions += 1
        if "catch_unwind" not in (match.group(1) or ""):
            line = text.count("\n", 0, match.start()) + 1
            shown = path.relative_to(ROOT) if path.is_relative_to(ROOT) else path
            bad.append(f"{shown}:{line} {item.group(1)}")
    if not functions:
        sys.exit(f"check_exports: no #[napi] function found in {path}")
    if bad:
        sys.exit("check_exports: napi functions without catch_unwind:\n  " + "\n  ".join(bad))


def camel(name):
    """The JNI natives' Java spelling of an export: `hypertabular_delimited_fill` is
    `hypertabularDelimitedFill`."""
    head, *rest = name.split("_")
    return head + "".join(part[:1].upper() + part[1:] for part in rest)


def jni_no_panic(path):
    """Every JNI entry point must carry the no-panic attribute: the example's link-time proof
    covers only the fns that have it, so one dropped attribute would pass every site."""
    path = Path(path)
    if not path.is_absolute():
        path = ROOT / path
    lines = path.read_text(encoding="utf-8").splitlines()
    bad = []
    for number, line in enumerate(lines):
        match = re.match(r'\s*pub unsafe extern "system" fn (\$?\w+)', line)
        if not match:
            continue
        attrs, above = [], number - 1
        while above >= 0 and lines[above].lstrip().startswith("#["):
            attrs.append(lines[above])
            above -= 1
        if not any("no_panic::no_panic" in attr for attr in attrs):
            bad.append(f"{match.group(1)} (line {number + 1})")
    if bad:
        sys.exit(f"check_exports: {path.name} has JNI functions without no_panic: " + ", ".join(bad))


_RUST_NOISE = re.compile(
    r"""//[^\n]*|/\*.*?\*/|b?"(?:\\.|[^"\\])*"|'(?:\\.[^']*|[^\\'])'""", re.DOTALL
)


def _mask_rust(text):
    """Blanks comments, string literals and char literals (newlines kept) so that braces and
    parentheses inside them don't confuse the matching below."""
    return _RUST_NOISE.sub(lambda m: re.sub(r"[^\n]", " ", m.group(0)), text)


def _match_close(text, open_at, open_char, close_char):
    """Index of the bracket closing the one at `open_at`, or -1."""
    depth = 0
    for index in range(open_at, len(text)):
        if text[index] == open_char:
            depth += 1
        elif text[index] == close_char:
            depth -= 1
            if depth == 0:
                return index
    return -1


def _fn_body(masked, name_end):
    """The body text (between the braces) of the fn whose name ends at `name_end`, or None
    for a declaration without one."""
    paren = masked.find("(", name_end)
    if paren < 0:
        return None
    close = _match_close(masked, paren, "(", ")")
    if close < 0:
        return None
    # The body's `{` or a `;` ends the signature only outside every bracket: `[u8; 16]` and
    # `Foo<[u8; 4]>` in a return type carry a `;`. `->` is an arrow, not a closing angle.
    depth, index, start = 0, close + 1, -1
    while index < len(masked):
        char = masked[index]
        if char in "[<(":
            depth += 1
        elif char in "])" or (char == ">" and masked[index - 1] != "-"):
            depth -= 1
        elif depth <= 0 and char in "{;":
            start = index if char == "{" else -1
            break
        index += 1
    if start < 0:
        return None
    end = _match_close(masked, start, "{", "}")
    return masked[start + 1 : end] if end >= 0 else None


def php_guard(directory):
    """Every PHP-callable Rust function in the extension must start with `guard(`: a panic
    that unwinds through the Zend engine's C frames aborts the PHP process instead of
    throwing. PHP-callable means every fn in a `#[php_impl]` block, every `#[php_function]`,
    and every handler registered with `FunctionBuilder::new("name", handler)` (the value
    classes' methods, a hand-written `extern "C" fn`, which must call `guard(` somewhere
    inside, since `run_handler(` surrounds it).

    Every other `extern "C" fn` the engine calls (the lifecycle callbacks: MINIT, RINIT,
    RSHUTDOWN, post-deactivate, post-startup) has no PHP caller to throw to, so it must contain
    `catch_unwind(` or `run_handler(` instead."""
    directory = Path(directory)
    if not directory.is_absolute():
        directory = ROOT / directory
    files = sorted(directory.glob("*.rs"))
    if not files:
        sys.exit(f"check_exports: {directory} matches no file")
    bad, unwinding, checked = [], [], 0
    fn_head = re.compile(r"\bfn\s+(\w+)")
    for path in files:
        raw = path.read_text(encoding="utf-8")
        masked = _mask_rust(raw)  # same length as `raw`, so offsets carry across
        for match in re.finditer(r"#\[php_impl\b[^\]]*\]", masked):
            brace = masked.find("{", match.end())
            end = _match_close(masked, brace, "{", "}")
            block = masked[brace + 1 : end]
            for fn in fn_head.finditer(block):
                if block.count("{", 0, fn.start()) != block.count("}", 0, fn.start()):
                    continue  # a fn nested in a method body
                body = _fn_body(block, fn.end())
                checked += 1
                if body is None or not body.lstrip().startswith("guard("):
                    bad.append(fn.group(1))
        for match in re.finditer(r"#\[php_function\b[^\]]*\]", masked):
            fn = fn_head.search(masked, match.end())
            body = _fn_body(masked, fn.end()) if fn else None
            checked += fn is not None
            if fn and (body is None or not body.lstrip().startswith("guard(")):
                bad.append(fn.group(1))
        # The handler names survive masking: only the method's name is a string literal.
        for handler in re.findall(r"FunctionBuilder::new\(\s*\"[^\"]*\"\s*,\s*(\w+)\s*\)", raw):
            found = re.search(rf'extern "C" fn {handler}\b', raw)
            inner = _fn_body(masked, found.end()) if found else None
            checked += 1
            if inner is None or "guard(" not in inner:
                bad.append(handler)
        for found in re.finditer(r'\bextern\s+"C"\s+fn\s+(\w+)', raw):
            body = _fn_body(masked, found.end())
            if body is None or not ("catch_unwind(" in body or "run_handler(" in body):
                unwinding.append(found.group(1))
    if not checked:
        sys.exit("check_exports: php_ext has no PHP-callable functions to check")
    if bad:
        sys.exit("check_exports: php_ext has PHP-callable functions without guard: " + ", ".join(bad))
    if unwinding:
        sys.exit(
            'check_exports: php_ext has extern "C" functions without catch_unwind or run_handler: '
            + ", ".join(unwinding)
        )


C_PROTOTYPE = r"(?m)^(?:u?int\d+_t|uintptr_t|void) (\w+)\("
EXPORT = r"hypertabular_[a-z_]+"

# (binding, [(file glob, pattern)], what it must declare, names it may declare besides).
# `all` is every export, and a function maps each export to the binding's own name.
SITES = [
    (
        "C# (P/Invoke)",
        [("csharp/HyperTabular/*.cs", r'LibraryImport\("hypertabular", EntryPoint = "(\w+)"\)')],
        "all",
        (),
    ),
    (
        "C# (Blazor WebAssembly)",
        [("csharp/HyperTabular/*.cs", r'LibraryImport\("\*", EntryPoint = "(\w+)"\)')],
        "all",
        (),
    ),
    (
        "C# (iOS and Mac Catalyst)",
        [("csharp/HyperTabular/*.cs", r'LibraryImport\("__Internal", EntryPoint = "(\w+)"\)')],
        "all",
        (),
    ),
    (
        "Java (FFM)",
        [("java/src/main/java25/io/github/skunkwerkx/hypertabular/Native.java", r'export\("(\w+)"')],
        "all",
        (),
    ),
    # The JDK 21 base: the Java declarations, the JNI library's registration table, its
    # no-panic proof (snake case, so not `camel`) and its own C imports of the core.
    (
        "Java (JNI natives)",
        [("java/src/main/java/io/github/skunkwerkx/hypertabular/Native.java", r"static native \w+ (\w+)\(")],
        camel,
        ("jniNatives",),
    ),
    (
        "Java (JNI table)",
        [("java/jni/src/lib.rs", r'(?m)^\s*native!\(\s*"(\w+)",')],
        camel,
        ("jniNatives",),
    ),
    (
        "Java (JNI no-panic)",
        [("java/jni/examples/no_panic.rs", r"hypertabular_jni::(?:native_(\w+)|(JNI_OnLoad))\(")],
        "all",
        ("jni_natives", "JNI_OnLoad"),
    ),
    (
        "Java (JNI imports)",
        [("java/jni/src/lib.rs", rf"(?m)^\s*fn ({EXPORT})\(")],
        "all",
        (),
    ),
    (
        # The core translated to Go by wasm2go (go/internal/core/core.go, regenerated by `go
        # generate` in go/): every export is a method on the generated Module.
        "Go (translated core)",
        [("go/internal/core/core.go", rf"func \(m \*Module\) X({EXPORT})\(")],
        "all",
        (),
    ),
    # The binding's use of the translated core: every export is called in go/core.go.
    ("Go (binding calls)", [("go/core.go", rf"\.X({EXPORT})\(")], "all", ()),
    # Generated by cbindgen (rust/tests/header.rs), which also holds the Swift bundle's copy
    # byte-identical to it.
    ("C (include/hypertabular.h)", [("include/hypertabular.h", C_PROTOTYPE)], "all", ()),
    # The header-only C++ binding: every export is named qualified.
    ("C++ (include/hypertabular.hpp)", [("include/hypertabular.hpp", rf"::({EXPORT})\(")], "all", ()),
    # The R package's C shim calls the core through the header vendor.sh stages from include/.
    ("R (C shim)", [("r/src/hypertabular.c", rf"\b({EXPORT})\(")], "all", ()),
    ("C (smoke)", [("c/smoke/main.c", rf"\b({EXPORT})\(")], "all", ()),
    ("Ruby (Fiddle)", [("ruby/lib/hypertabular/runtime.rb", rf"(?m)^\s*({EXPORT}):\s")], "all", ()),
    ("Swift", [("swift/Sources/HyperTabular/*.swift", rf"\b({EXPORT})\(")], "all", ()),
]


# Sites whose names must each appear once: a table that registers a name twice is a bug a
# set comparison cannot see.
UNIQUE_SITES = ("Java (JNI table)",)


PHP_LINK_NTS = '#[cfg_attr(all(windows, not(php_zts)), link(name = "php8"))]'
PHP_LINK_TS = '#[cfg_attr(all(windows, php_zts), link(name = "php8ts"))]'


def php_link(directory):
    """Every extern block in the extension that names the engine's Windows import library names
    it through the pair PHP_LINK_NTS, PHP_LINK_TS, on adjacent lines and opening the block (other
    attributes may follow). A thread-safe Windows PHP is php8ts.dll, with php8ts.lib and no
    php8.lib: a bare `link(name = "php8")` fails to link there, or links a DLL PHP cannot load.
    build.rs sets `php_zts` from the PHP ext-php-rs builds against."""
    directory = Path(directory)
    if not directory.is_absolute():
        directory = ROOT / directory
    files = sorted(directory.glob("*.rs"))
    if not files:
        sys.exit(f"check_exports: {directory} matches no file")
    bad = []
    for path in files:
        raw = path.read_text(encoding="utf-8").split("\n")
        masked = _mask_rust("\n".join(raw)).split("\n")
        paired = set()
        for index, line in enumerate(raw):
            if not masked[index].lstrip().startswith("#[") or not re.search(r'\blink\(\s*name\s*=\s*"php8', line):
                continue
            if index in paired:
                continue
            rest = masked[index + 2 :]
            opens = next((code.strip() for code in rest if not code.strip().startswith("#[")), "")
            if (
                line.strip() == PHP_LINK_NTS
                and index + 1 < len(raw)
                and raw[index + 1].strip() == PHP_LINK_TS
                and re.match(r"(unsafe\s+)?extern\b", opens)
            ):
                paired.add(index + 1)
                continue
            bad.append(f"{path.name}:{index + 1}")
    if bad:
        sys.exit(
            "check_exports: php_ext names the engine's library without the php8/php8ts pair "
            "(build.rs's php_zts): " + ", ".join(bad)
        )


def declared_list(sources):
    names = []
    for glob, pattern in sources:
        files = sorted(ROOT.glob(glob))
        if not files:
            sys.exit(f"check_exports: {glob} matches no file")
        for path in files:
            for match in re.finditer(pattern, path.read_text(encoding="utf-8"), re.MULTILINE):
                names.extend(group for group in match.groups() if group)
    return names


def declared(sources):
    return set(declared_list(sources))


def main():
    exports = core_exports()
    if VERSION not in exports or len(exports) < 2:
        sys.exit(f"check_exports: read {sorted(exports)} from {CORE}, which is not the core's ABI")
    print(f"core: {len(exports)} exports")
    jni_no_panic("java/jni/src/lib.rs")
    php_guard("rust/src/php_ext")
    php_link("rust/src/php_ext")
    napi_catch_unwind("typescript/src/lib.rs")
    failed = False
    for binding, sources, wants, extra in SITES:
        if wants == "all":
            expected = set(exports)
        elif wants == "doors":
            expected = exports - {VERSION}
        else:
            expected = {name for name in map(wants, exports) if name}
        expected |= set(extra)
        found = declared_list(sources)
        got = set(found)
        repeats = sorted({name for name in found if found.count(name) > 1}) if binding in UNIQUE_SITES else []
        missing, stale = sorted(expected - got), sorted(got - expected)
        if missing or stale or repeats:
            failed = True
            print(f"FAIL {binding}")
            for name in repeats:
                print(f"     repeats {name}")
            for name in missing:
                print(f"     missing {name}")
            for name in stale:
                print(f"     declares {name}, which the core does not export")
        else:
            print(f"ok   {binding}: {len(got)}")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()

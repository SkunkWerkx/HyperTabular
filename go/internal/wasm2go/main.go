// Command wasm2go regenerates go/internal/core/core.go: the Rust core built for
// wasm32-wasip1 by the rustc pinned in rust-toolchain.toml beside this file, translated to
// one Go source file by the wasm2go pinned in this module's go.mod. It is run by
// `go generate` in go/ and by the regeneration tests, and never ships: this module is a tool
// module, not part of the published binding.
//
// Exit status 2 means a prerequisite is missing (cargo, rustup, the pinned toolchain or its
// wasm32-wasip1 target) and the install line is on stderr; 1 means cargo or wasm2go itself
// failed and their output is on stderr. The tests tell the two apart by the status, never
// by parsing the text.
package main

import (
	"flag"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"runtime"
	"strconv"
	"strings"
)

// defaultStack is the guest's shadow stack in bytes, passed to the linker as
// -zstack-size. rust-lld's default is 1 MiB; an instance must be small enough to pool (the
// binding keeps one primary instance and a pool of them), and the core's deepest path needs
// far less. The margin is measured, not proved: regenerating at halving sizes and running
// the whole suite, the corpus replay included, at each, 4096 bytes is the smallest size that
// passes everything and 2048 is the first that fails, with a bounds panic from the generated
// code (the stack pointer runs below zero and the next access traps), never a wrong verdict
// (2026-11-01, Apple M6, rustc 1.99.1). 65536 is 16x the smallest passing size, against a
// floor of 8x, and it is HyperUuid's size too. The flag goes on the command line here, never
// into rust/.cargo/config.toml, so nothing else built for wasm32-wasip1 changes.
const defaultStack = 65536

const target = "wasm32-wasip1"

var channelLine = regexp.MustCompile(`(?m)^channel = "([^"]+)"`)

func main() {
	os.Exit(run())
}

func run() int {
	_, self, _, ok := runtime.Caller(0)
	if !ok {
		fmt.Fprintln(os.Stderr, "wasm2go: cannot locate this source file")
		return 1
	}
	toolDir := filepath.Dir(self)
	defaultRoot := filepath.Clean(filepath.Join(toolDir, "..", "..", ".."))

	root := flag.String("root", defaultRoot, "repository root (holds rust/ and go/)")
	out := flag.String("o", "", "output file (default <root>/go/internal/core/core.go)")
	stack := flag.Int("stack", defaultStack, "guest stack size in bytes (-zstack-size)")
	flag.Parse()
	if *out == "" {
		*out = filepath.Join(*root, "go", "internal", "core", "core.go")
	}

	pin, err := pinnedToolchain(toolDir)
	if err != nil {
		fmt.Fprintln(os.Stderr, "wasm2go:", err)
		return 1
	}
	install := fmt.Sprintf("rustup toolchain install %s --profile minimal --target %s", pin, target)

	// Prerequisites: exit 2 with the install line.
	for _, tool := range []string{"cargo", "rustup"} {
		if _, err := exec.LookPath(tool); err != nil {
			fmt.Fprintf(os.Stderr, "wasm2go: %s is not on PATH; install Rust, then run\n  %s\n", tool, install)
			return 2
		}
	}
	if msg, err := exec.Command("rustup", "run", pin, "rustc", "-V").CombinedOutput(); err != nil {
		fmt.Fprintf(os.Stderr, "wasm2go: toolchain %s is not installed (%s); run\n  %s\n", pin, strings.TrimSpace(string(msg)), install)
		return 2
	}
	if msg, err := exec.Command("rustup", "target", "list", "--installed", "--toolchain", pin).Output(); err != nil || !strings.Contains(string(msg), target) {
		fmt.Fprintf(os.Stderr, "wasm2go: target %s is not installed for %s; run\n  %s\n", target, pin, install)
		return 2
	}

	// Build the core. `cargo cdylib` is the crate's alias (rust/.cargo/config.toml): the
	// no_std shared library every other binding loads, here for wasm32-wasip1. The arguments
	// after `--` reach rustc. `-l c` links wasi-libc, which the pinned toolchain ships for
	// this target: the core calls memcmp, memcpy and memset, which a no_std build leaves to
	// the C library, and lib.rs names that library for unix and MSVC targets only. wasi-libc's
	// string functions import nothing, so the module still imports nothing at all, and the
	// generated Module takes no host functions.
	rustDir := filepath.Join(*root, "rust")
	targetDir := filepath.Join(rustDir, "target")
	cargo := exec.Command("cargo", "+"+pin, "cdylib", "--target", target, "--target-dir", targetDir,
		"--", "-C", "link-arg=-zstack-size="+strconv.Itoa(*stack), "-l", "c")
	cargo.Dir = rustDir
	if msg, err := cargo.CombinedOutput(); err != nil {
		fmt.Fprintf(os.Stderr, "wasm2go: cargo failed in %s: %v\n%s", rustDir, err, msg)
		return 1
	}
	wasm := filepath.Join(targetDir, target, "release", "hypertabular.wasm")

	// Translate. `go tool wasm2go` resolves through this module's go.mod, so the version is
	// the pinned one wherever the caller's working directory is.
	if err := os.MkdirAll(filepath.Dir(*out), 0o755); err != nil {
		fmt.Fprintln(os.Stderr, "wasm2go:", err)
		return 1
	}
	translate := exec.Command("go", "tool", "wasm2go", "-pkg", "core", "-o", *out, wasm)
	translate.Dir = toolDir
	if msg, err := translate.CombinedOutput(); err != nil {
		fmt.Fprintf(os.Stderr, "wasm2go: translation failed: %v\n%s", err, msg)
		return 1
	}
	return 0
}

func pinnedToolchain(toolDir string) (string, error) {
	data, err := os.ReadFile(filepath.Join(toolDir, "rust-toolchain.toml"))
	if err != nil {
		return "", fmt.Errorf("reading the toolchain pin: %w", err)
	}
	m := channelLine.FindSubmatch(data)
	if m == nil {
		return "", fmt.Errorf("rust-toolchain.toml has no channel line")
	}
	return string(m[1]), nil
}

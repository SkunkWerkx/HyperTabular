//go:build cgo && !tinygo && !hypertabular_local && (darwin || linux || windows) && !ios && !android

// The core freshness check. go/staticlib is committed, and refreshed only when a release
// stages new binaries, so a plain `go test` after a change to rust/ would test the core as it
// was at the last staging and say nothing about it. Before any test runs, this compares the
// core's inputs (rust/src, rust/Cargo.toml, rust/Cargo.lock) at the commit the archives were
// built from with the working tree's, and stops the run when they differ.
//
// It passes without looking when CI is set (every CI leg builds its own core and writes it
// over go/staticlib first), under the hypertabular_local tag (the archive local-core.sh
// builds from the checkout), outside a git work tree (the iOS and Android runs, which are
// excluded by the constraint anyway, and the Alpine suite's copy), when the staging commit or
// its source is missing (a shallow clone), and when go/staticlib itself has working-tree
// changes other than to a README (someone wrote fresh archives into it).
//
// go test replays a cached pass without running TestMain, and it tracks no file outside this
// module, so a pass recorded before a change to rust/ is replayed after it: -count=1 runs the
// check again.

package hypertabular

import (
	"fmt"
	"os"
	"os/exec"
	"regexp"
	"strings"
	"testing"
)

func TestMain(m *testing.M) {
	if message := staleCore(); message != "" {
		fmt.Fprintln(os.Stderr, message)
		os.Exit(1)
	}
	os.Exit(m.Run())
}

// stagedFrom is the source commit at the end of a staging commit's subject.
var stagedFrom = regexp.MustCompile(`\(([0-9a-f]{40})\)$`)

// staleCore returns the message to stop on, or "" when the linked core can be trusted.
func staleCore() string {
	const tree = "go/staticlib"
	if os.Getenv("CI") != "" {
		return ""
	}
	if _, err := exec.LookPath("git"); err != nil {
		return ""
	}
	top, status := git("", "rev-parse", "--show-toplevel")
	if status != 0 || top == "" {
		return ""
	}
	staging, status := git(top, "log", "-1", "--format=%H", "--grep=^chore: stage native binaries", "HEAD")
	if status != 0 || staging == "" {
		return ""
	}
	source, _ := git(top, "log", "-1", "--format=%(trailers:key=Release-Source-SHA,valueonly)", staging)
	if source == "" {
		subject, _ := git(top, "log", "-1", "--format=%s", staging)
		match := stagedFrom.FindStringSubmatch(subject)
		if match == nil {
			return ""
		}
		source = match[1]
	}
	if _, status := git(top, "cat-file", "-e", source+"^{commit}"); status != 0 {
		return ""
	}
	readmes := ":(exclude,glob)" + tree + "/**/README.md"
	if _, status := git(top, "diff", "--quiet", "HEAD", "--", tree, readmes); status == 1 {
		return ""
	}
	_, changed := git(top, "diff", "--quiet", source, "--", "rust/src", "rust/Cargo.toml", "rust/Cargo.lock")
	untracked, _ := git(top, "ls-files", "--others", "--exclude-standard", "--", "rust/src")
	if changed != 1 && untracked == "" {
		return ""
	}
	return tree + " was built from rust/ at " + source[:12] + ", and this checkout's rust/ differs from it, " +
		"so these tests would run against a stale core. Build the checkout's own with " +
		".github/scripts/local-core.sh (FORGE=<path> if the SkunkWerkx/.github checkout is not " +
		"../.github or ../HyperForge), then run go test -tags hypertabular_local ./... instead; " +
		"CI builds its own core and skips this check."
}

// git runs git in dir ("" for the working directory) and returns its trimmed output and its
// exit status (-1 when it did not run to an exit).
func git(dir string, args ...string) (string, int) {
	cmd := exec.Command("git", args...)
	cmd.Dir = dir
	out, err := cmd.Output()
	status := 0
	if err != nil {
		status = -1
		if exit, ok := err.(*exec.ExitError); ok {
			status = exit.ExitCode()
		}
	}
	return strings.TrimSpace(string(out)), status
}

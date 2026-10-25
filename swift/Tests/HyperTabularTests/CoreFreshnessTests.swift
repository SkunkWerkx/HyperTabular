// The core freshness check. The artifact bundle is committed, and refreshed only when a
// release stages new binaries, so a plain `swift test` after a change to rust/ would test the
// core as it was at the last staging and say nothing about it. This compares the core's
// inputs (rust/src, rust/Cargo.toml, rust/Cargo.lock) at the commit the bundle was built from
// with the working tree's, and fails by name when they differ: XCTest cannot stop the run
// before the other tests, so the rest still run, against the stale core.
//
// It passes without looking when CI is set (every CI leg builds its own core and writes it
// over the bundle first), under HYPERTABULAR_LOCAL_CORE (the bundle local-core.sh builds from
// the checkout), outside a git work tree (the Linux and WebAssembly suites' copy), when the
// staging commit or its source is missing (a shallow clone), and when the bundle itself has
// working-tree changes other than to a README (someone wrote fresh archives into it). It is
// compiled only where a test can start a process: not on iOS, Mac Catalyst, Android or WASI.
#if !HYPERTABULAR_LOCAL_CORE && (os(macOS) || os(Linux) || os(Windows))
    import Foundation
    import XCTest

    final class CoreFreshnessTests: XCTestCase {
        func testTheLinkedCoreWasBuiltFromThisCheckout() {
            if let message = Self.staleCore() {
                XCTFail(message)
            }
        }

        /// The message to fail with, or nil when the linked core can be trusted.
        private static func staleCore() -> String? {
            let tree = "swift/HyperTabularCore.artifactbundle"
            let environment = ProcessInfo.processInfo.environment
            if let ci = environment["CI"], !ci.isEmpty {
                return nil
            }
            guard let git = findGit(environment) else { return nil }
            let here = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            guard let (top, status) = run(git, in: here, "rev-parse", "--show-toplevel"), status == 0, !top.isEmpty
            else { return nil }
            let root = URL(fileURLWithPath: top, isDirectory: true)
            guard
                let (staging, status) = run(
                    git, in: root, "log", "-1", "--format=%H", "--grep=^chore: stage native binaries", "HEAD"),
                status == 0, !staging.isEmpty
            else { return nil }
            var source =
                run(git, in: root, "log", "-1", "--format=%(trailers:key=Release-Source-SHA,valueonly)", staging)?.0
                ?? ""
            if source.isEmpty {
                let subject = run(git, in: root, "log", "-1", "--format=%s", staging)?.0 ?? ""
                guard let sha = sourceInSubject(subject) else { return nil }
                source = sha
            }
            guard run(git, in: root, "cat-file", "-e", "\(source)^{commit}")?.1 == 0 else { return nil }
            let readmes = ":(exclude,glob)\(tree)/**/README.md"
            if run(git, in: root, "diff", "--quiet", "HEAD", "--", tree, readmes)?.1 == 1 {
                return nil
            }
            let changed = run(
                git, in: root, "diff", "--quiet", source, "--", "rust/src", "rust/Cargo.toml", "rust/Cargo.lock")?.1
            let untracked = run(git, in: root, "ls-files", "--others", "--exclude-standard", "--", "rust/src")?.0 ?? ""
            if changed != 1 && untracked.isEmpty {
                return nil
            }
            return "\(tree) was built from rust/ at \(source.prefix(12)), and this checkout's rust/ differs from it, "
                + "so these tests would run against a stale core. Build the checkout's own with "
                + ".github/scripts/local-core.sh (FORGE=<path> if the SkunkWerkx/.github checkout is not "
                + "../.github or ../HyperForge), then run HYPERTABULAR_LOCAL_CORE=1 swift test instead; "
                + "CI builds its own core and skips this check."
        }

        /// The 40-hex source commit in parentheses at the end of a staging commit's subject.
        private static func sourceInSubject(_ subject: String) -> String? {
            guard subject.hasSuffix(")"), let open = subject.lastIndex(of: "(") else { return nil }
            let sha = subject[subject.index(after: open)..<subject.index(before: subject.endIndex)]
            let hex = Set("0123456789abcdef")
            return sha.count == 40 && sha.allSatisfy { hex.contains($0) } ? String(sha) : nil
        }

        /// git on PATH, or nil when there is none.
        private static func findGit(_ environment: [String: String]) -> URL? {
            #if os(Windows)
                let path = environment["Path"] ?? environment["PATH"] ?? ""
                let (separator, names): (Character, [String]) = (";", ["git.exe"])
            #else
                let path = environment["PATH"] ?? ""
                let (separator, names): (Character, [String]) = (":", ["git"])
            #endif
            for dir in path.split(separator: separator) where !dir.isEmpty {
                for name in names {
                    let candidate = URL(fileURLWithPath: String(dir), isDirectory: true).appendingPathComponent(name)
                    if FileManager.default.isExecutableFile(atPath: candidate.path) {
                        return candidate
                    }
                }
            }
            return nil
        }

        /// Runs git in a directory: its trimmed output and exit status, or nil when it did not start.
        private static func run(_ git: URL, in directory: URL, _ arguments: String...) -> (String, Int32)? {
            let process = Process()
            process.executableURL = git
            process.arguments = arguments
            process.currentDirectoryURL = directory
            let output = Pipe()
            process.standardOutput = output
            process.standardError = FileHandle.nullDevice
            do {
                try process.run()
            } catch {
                return nil
            }
            let data = output.fileHandleForReading.readDataToEndOfFile()
            process.waitUntilExit()
            let text = String(decoding: data, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
            return (text, process.terminationStatus)
        }
    }
#endif

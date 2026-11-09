import Foundation
import XCTest

@testable import HyperTabular

/// A source calling back into the reader it feeds: what it can reach, and what each says.
final class ReentryTests: XCTestCase {
    /// Where the source finds its reader once there is one.
    private final class Holder {
        var reader: DelimitedReader?
    }

    private struct Thrown: Error {}

    private struct Row: Equatable {
        let id: Int32
        let name: String
        let line: Int
    }

    /// A header and rows of `id,name-id`, the fifth name 40 bytes long, so that a small buffer grows in
    /// the middle of the read.
    private static func text(_ rows: Int) -> [UInt8] {
        var text = "id,name\n"
        for i in 0..<rows {
            text += "\(i),\(name(i))\n"
        }
        return Array(text.utf8)
    }

    private static func name(_ i: Int) -> String {
        i == 5 ? String(repeating: "x", count: 40) : "name-\(i)"
    }

    private static func collect(_ batch: Batch, into rows: inout [Row]) {
        let ids = batch.values(0, as: Int32.self)
        for row in 0..<batch.rows {
            rows.append(Row(id: ids[row], name: batch.string(1, row: row) ?? "", line: batch.line(row)))
        }
    }

    /// A source handing over `chunk` bytes a call, which on call `at` after the header runs `hook`.
    private static func reader(
        _ data: [UInt8], encoding: TextEncoding, chunk: Int, bufferBytes: Int, at: Int,
        hook: @escaping (DelimitedReader) throws -> Void
    ) throws -> DelimitedReader {
        let holder = Holder()
        var position = 0
        var calls = 0
        var fireAt = Int.max
        let source: DelimitedReader.Source = { room in
            calls += 1
            if calls == fireAt, let reader = holder.reader {
                try hook(reader)
            }
            let count = min(room.count, chunk, data.count - position)
            guard count > 0 else { return 0 }
            data.withUnsafeBytes { room.baseAddress!.copyMemory(from: $0.baseAddress! + position, byteCount: count) }
            position += count
            return count
        }
        var dialect = Dialect.csv
        dialect.encoding = encoding
        let reader = try DelimitedReader(
            reading: source, dialect: dialect, plan: [.i32(0), .text(1)], batchRows: 1, bufferBytes: bufferBytes)
        // The header was read before there was a reader to call: count from after it.
        fireAt = calls + at
        holder.reader = reader
        return reader
    }

    /// A read or a bind from the source, in the middle of a read, throws `busy`; the read it was called
    /// from, and every one after it, delivers each row once, in order, from the right line. Before the
    /// reader marked a read in progress, the inner read freed the buffer the outer source was writing
    /// into (AddressSanitizer: heap-use-after-free) or delivered a row from the wrong line.
    func testASourceCallingBackIntoItsReaderIsRefused() throws {
        let rows = 100
        let data = Self.text(rows)
        let whole = (0..<rows).map { Row(id: Int32($0), name: Self.name($0), line: $0 + 2) }
        for encoding in [TextEncoding.utf8, .latin1] {
            for chunk in [1, 7, 64] {
                for bufferBytes in [8, 64] {
                    for at in [2, 3, 5, 8, 13] {
                        let label = "\(encoding), \(chunk)-byte chunks, \(bufferBytes)-byte buffer, call \(at)"
                        var fired = false
                        var read: [Row] = []
                        let reader = try Self.reader(
                            data, encoding: encoding, chunk: chunk, bufferBytes: bufferBytes, at: at
                        ) { reader in
                            fired = true
                            XCTAssertThrowsError(try reader.read(), label) {
                                XCTAssertEqual($0 as? PlanError, .busy, label)
                            }
                            XCTAssertThrowsError(try reader.bind([.i32(0)]), label) {
                                XCTAssertEqual($0 as? PlanError, .busy, label)
                            }
                            XCTAssertThrowsError(try reader.forEachRow { _ in }, label) {
                                XCTAssertEqual($0 as? PlanError, .busy, label)
                            }
                        }
                        while let batch = try reader.read() {
                            Self.collect(batch, into: &read)
                        }
                        XCTAssertTrue(fired, "\(label): the source was never called that often")
                        XCTAssertEqual(read, whole, label)
                    }
                }
            }
        }
    }

    /// The same from inside `readAsync()`, which a source-backed reader can be read with too.
    func testASourceCallingBackFromInsideReadAsyncIsRefused() async throws {
        let rows = 100
        var fired = false
        var refused = true
        let reader = try Self.reader(Self.text(rows), encoding: .utf8, chunk: 7, bufferBytes: 8, at: 5) { reader in
            fired = true
            do {
                _ = try reader.read()
                refused = false
            } catch {
                refused = refused && error as? PlanError == .busy
            }
            do {
                try reader.bind([.i32(0)])
                refused = false
            } catch {
                refused = refused && error as? PlanError == .busy
            }
        }
        var read: [Row] = []
        while let batch = try await reader.readAsync() {
            Self.collect(batch, into: &read)
        }
        XCTAssertTrue(fired && refused)
        XCTAssertEqual(read, (0..<rows).map { Row(id: Int32($0), name: Self.name($0), line: $0 + 2) })
    }

    /// A source that throws leaves no read in progress: the next read goes on from it.
    func testASourceThatThrowsLeavesTheReaderIdle() throws {
        let reader = try Self.reader(Self.text(10), encoding: .utf8, chunk: 7, bufferBytes: 8, at: 3) { _ in
            throw Thrown()
        }
        var read: [Row] = []
        var thrown = 0
        while true {
            do {
                guard let batch = try reader.read() else { break }
                Self.collect(batch, into: &read)
            } catch is Thrown {
                thrown += 1
            }
        }
        XCTAssertEqual(thrown, 1)
        XCTAssertEqual(read.map(\.id), (0..<10).map(Int32.init))
    }

    #if os(macOS) || os(Linux)
        /// A read in the body of `forEachRow` moves the reader under the walk: the walk traps at its next
        /// row, as a batch's own iterator does past the next read, and never answers for the next batch.
        func testAReadInTheBodyOfForEachRowTraps() {
            let input = Array("a\n1\n2\n3\n4\n".utf8)
            XCTAssertEqual(
                Support.ending {
                    let reader = try! DelimitedReader(bytes: input, dialect: .csv, plan: [.i32(0)], batchRows: 4)
                    try! reader.forEachRow { _ in _ = try reader.read() }
                }, .trapped)
            XCTAssertEqual(
                Support.ending {
                    let reader = try! DelimitedReader(bytes: input, dialect: .csv, plan: [.i32(0)], batchRows: 4)
                    try! reader.forEachRow { _ in }
                }, .returned)
        }
    #endif
}

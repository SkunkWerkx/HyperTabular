import Foundation
import XCTest

/// Every key either corpus holds, listed by the kind of object that holds it, as the replays read
/// them: a key that a corpus gains and no replay reads is a failure here, since `Decodable` would
/// pass over it in silence.
final class CorpusKeysTests: XCTestCase {
    private typealias Keys = Set<String>

    private static let format: Keys = ["decimal_sep", "group_sep", "flags", "currency"]
    private static let plan: Keys = [
        "door", "ordinal", "precision", "order", "epoch", "format", "tokens", "options", "constraints",
    ]
    private static let dateOptions: Keys = ["flags", "pivot", "months"]
    private static let constraints: Keys = [
        "minimum", "exclusiveMinimum", "maximum", "exclusiveMaximum", "multipleOf", "rounding", "minLength",
        "maxLength", "enum", "trim", "caseSensitive",
    ]
    private static let cell: Keys = [
        "expect", "fault", "raw", "raw_hex", "detail", "origin", "value", "text", "magnitude", "negative", "scale",
        "seconds", "nanos", "year", "month", "day", "nanos_of_day", "keyword", "adjusted", "enum_index", "bits",
    ]
    private static let problem: Keys = [
        "row", "line", "plan_column", "source_column", "reason", "detail", "origin", "fault", "expected", "found",
        "keyword",
    ]
    private static let failure: Keys = ["kind", "record", "line", "byte", "expected", "found"]
    private static let suggest: Keys = ["columns", "truncated", "sample_rows", "format"]
    private static let suggested: Keys = ["door", "param", "status", "matched", "sampled"]
    private static let delimited: Keys = [
        "name", "input", "input_hex", "dialect", "plan", "header", "rows", "problems", "failure", "suggest",
    ]
    private static let dialect: Keys = [
        "separator", "quoting", "has_header", "skip_blank_lines", "skip", "header_row", "short_rows", "long_rows",
        "tokens", "encoding",
    ]
    private static let workbook: Keys = [
        "name", "note", "file", "format", "epoch", "sheets", "sheet", "options", "plan", "header", "rows", "numbers",
        "problems", "failure", "suggest",
    ]
    private static let sheet: Keys = ["name", "hidden"]
    private static let sheetOptions: Keys = [
        "has_header", "skip_empty_rows", "skip", "header_row", "range", "tokens",
    ]

    private static func check(_ entry: Any?, _ path: String, _ known: Keys, _ unknown: inout [String]) {
        guard let entry = entry as? [String: Any] else { return }
        unknown += entry.keys.filter { !known.contains($0) }.map { "\(path).\($0)" }
    }

    private static func each(_ list: Any?) -> [Any] {
        list as? [Any] ?? []
    }

    private static func unknownDelimited(_ cases: [[String: Any]]) -> [String] {
        var unknown = [String]()
        for entry in cases {
            let name = entry["name"] as? String ?? "a case with no name"
            check(entry, name, delimited, &unknown)
            check(entry["dialect"], "\(name).dialect", dialect, &unknown)
            shared(entry, name, &unknown)
        }
        return unknown
    }

    private static func unknownWorkbook(_ cases: [[String: Any]]) -> [String] {
        var unknown = [String]()
        for entry in cases {
            let name = entry["name"] as? String ?? "a case with no name"
            check(entry, name, workbook, &unknown)
            check(entry["options"], "\(name).options", sheetOptions, &unknown)
            for listed in each(entry["sheets"]) {
                check(listed, "\(name).sheets[]", sheet, &unknown)
            }
            shared(entry, name, &unknown)
        }
        return unknown
    }

    private static func shared(_ entry: [String: Any], _ name: String, _ unknown: inout [String]) {
        for column in each(entry["plan"]) {
            check(column, "\(name).plan[]", plan, &unknown)
            let column = column as? [String: Any] ?? [:]
            check(column["format"], "\(name).plan[].format", format, &unknown)
            check(column["options"], "\(name).plan[].options", dateOptions, &unknown)
            check(column["constraints"], "\(name).plan[].constraints", constraints, &unknown)
        }
        for row in each(entry["rows"]) {
            for value in each(row) {
                check(value, "\(name).rows[][]", cell, &unknown)
            }
        }
        for listed in each(entry["problems"]) {
            check(listed, "\(name).problems[]", problem, &unknown)
        }
        check(entry["failure"], "\(name).failure", failure, &unknown)
        if let said = entry["suggest"] as? [String: Any] {
            check(said, "\(name).suggest", suggest, &unknown)
            check(said["format"], "\(name).suggest.format", format, &unknown)
            for column in each(said["columns"]) {
                check(column, "\(name).suggest.columns[]", suggested, &unknown)
            }
        }
    }

    private func cases(_ file: String) throws -> [[String: Any]] {
        guard let directory = CorpusTests.corpusDirectory else {
            throw XCTSkip("the conformance corpus is not reachable from a WASI sandbox")
        }
        let data = try Data(contentsOf: directory.appendingPathComponent(file))
        return try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [[String: Any]])
    }

    /// `value` with a made-up key put in each place below it that holds an object, once per kind of
    /// place: every one the walk has not met before gets its turn.
    private func withJunk(_ value: Any, at place: String, _ places: inout Set<String>, _ visit: (Any) -> Void) {
        if let list = value as? [Any] {
            for (index, item) in list.enumerated() {
                withJunk(item, at: place + "[]", &places) { changed in
                    var copy = list
                    copy[index] = changed
                    visit(copy)
                }
            }
        } else if let entry = value as? [String: Any] {
            if places.insert(place).inserted {
                var copy = entry
                copy["junk_key"] = 1
                visit(copy)
            }
            for (key, item) in entry {
                withJunk(item, at: "\(place).\(key)", &places) { changed in
                    var copy = entry
                    copy[key] = changed
                    visit(copy)
                }
            }
        }
    }

    private func aJunkKeyIsSeenEverywhere(_ file: String, _ unknown: ([[String: Any]]) -> [String]) throws {
        let cases = try cases(file)
        XCTAssertEqual(unknown(cases), [], "the file as it stands")
        var places = Set<String>()
        for entry in cases {
            withJunk(entry, at: "", &places) { changed in
                let found = unknown([changed as? [String: Any] ?? [:]])
                XCTAssertEqual(found.count, 1, "\(found)")
                XCTAssertTrue(found.first?.hasSuffix(".junk_key") ?? false, "\(found)")
            }
        }
        XCTAssertGreaterThan(places.count, 5)
    }

    func testEveryDelimitedCorpusKeyIsRead() throws {
        try aJunkKeyIsSeenEverywhere("delimited.json", Self.unknownDelimited)
    }

    func testEveryWorkbookCorpusKeyIsRead() throws {
        try aJunkKeyIsSeenEverywhere("workbook.json", Self.unknownWorkbook)
    }
}

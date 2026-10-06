import Foundation
import HyperTabular

// Every native entry point the binding declares, crossed through the public API, with the
// answers checked: the version probe, the header, the fill through all twenty-two doors
// (from a stream read through a buffer too small for the record), the unescape behind a
// raw read, a structural failure, and every workbook entry point. HyperCast's own door is asked the same question
// once, so both linked-in cores are called. Exits non-zero on the first thing that is
// wrong, so the build-and-run is the test.

func check(_ condition: Bool, _ what: String) {
    guard condition else {
        print("FAILED: \(what)")
        exit(1)
    }
}

func text(_ bytes: UnsafeRawBufferPointer) -> String {
    String(decoding: bytes, as: UTF8.self)
}

do {
    check(Tabular.isAvailable, "isAvailable")
    let version = Tabular.nativeVersion()
    check(version.split(separator: ".").count == 3, "nativeVersion is major.minor.patch, got \(version)")

    let names = [
        "flag", "i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "f32", "f64", "money", "id", "at",
        "unix", "serial", "day", "us", "civil", "time", "span", "\"say \"\"hi\"\"\"",
    ]
    let cells = [
        "yes", "-128", "32767", "(1234)", "9223372036854775807", "255", "65535", "4294967295",
        "18446744073709551615", "2.5", "25.5%", "1234.50", "6ba7b810-9dad-11d1-80b4-00c04fd430c8",
        "2026-01-02T15:04:05+05:00", "1700000000123", "45292.75", "2024-01-31", "1/7/2026",
        "1/7/2026 3:04 PM", "15:04:05.5", "PT1H30M", "\"a \"\"quoted\"\" cell\"",
    ]
    let input = Array((names.joined(separator: ",") + "\n" + cells.joined(separator: ",") + "\n").utf8)
    let plan: [Column] = [
        .bool(0), .i8(1), .i16(2), .i32(3), .i64(4), .u8(5), .u16(6), .u32(7), .u64(8), .f32(9),
        .f64(10), .decimal(11), .uuid(12), .timestamp(13), .unix(14, precision: .milliseconds),
        .excelSerial(15, epoch: .y1900), .date(16), .date(17, order: .monthDayYear),
        .dateTime(18, order: .monthDayYear), .time(19), .duration(20), .text(21), .i32(21),
    ]

    var position = 0
    let reader = try DelimitedReader(
        reading: { buffer in
            let count = min(buffer.count, input.count - position)
            buffer.copyBytes(from: input[position..<position + count])
            position += count
            return count
        }, dialect: .csv, plan: plan, batchRows: 8, bufferBytes: 16)

    check(reader.header?.count == 22 && reader.header?.last == "say \"hi\"", "header")
    guard let batch = try reader.read() else {
        check(false, "read")
        exit(1)
    }
    check(batch.rows == 1 && batch.line(0) == 2, "read")
    check(batch.get(0, row: 0, as: Bool.self) == .success(true), "bool")
    check(batch.get(1, row: 0, as: Int8.self) == .success(-128), "i8")
    check(batch.get(2, row: 0, as: Int16.self) == .success(32767), "i16")
    check(batch.get(3, row: 0, as: Int32.self) == .success(-1234), "i32, accounting negative")
    check(batch.get(4, row: 0, as: Int64.self) == .success(.max), "i64 max")
    check(batch.get(5, row: 0, as: UInt8.self) == .success(255), "u8")
    check(batch.get(6, row: 0, as: UInt16.self) == .success(65535), "u16")
    check(batch.get(7, row: 0, as: UInt32.self) == .success(4_294_967_295), "u32")
    check(batch.get(8, row: 0, as: UInt64.self) == .success(.max), "u64 max")
    check(batch.get(9, row: 0, as: Float.self) == .success(2.5), "f32")
    check(batch.get(10, row: 0, as: Double.self) == .success(0.255), "f64, percent")
    check(batch.get(11, row: 0, as: Decimal.self) == .success(Decimal(string: "1234.5")!), "decimal is exact")
    check(
        batch.get(12, row: 0, as: UUID.self) == .success(UUID(uuidString: "6ba7b810-9dad-11d1-80b4-00c04fd430c8")!),
        "uuid")
    check(batch.get(13, row: 0, as: Date.self) == .success(Date(timeIntervalSince1970: 1_767_348_245)), "timestamp")
    check(batch.get(14, row: 0, as: Date.self) == .success(Date(timeIntervalSince1970: 1_700_000_000.123)), "unix")
    check(batch.get(15, row: 0, as: Date.self) == .success(Date(timeIntervalSince1970: 1_704_132_000)), "excel serial")
    check(
        batch.get(16, row: 0, as: DateComponents.self) == .success(DateComponents(year: 2024, month: 1, day: 31)),
        "date")
    check(
        batch.get(17, row: 0, as: DateComponents.self) == .success(DateComponents(year: 2026, month: 1, day: 7)),
        "ordered date")
    check(
        batch.get(18, row: 0, as: DateComponents.self)
            == .success(DateComponents(year: 2026, month: 1, day: 7, hour: 15, minute: 4, second: 0, nanosecond: 0)),
        "civil date-time")
    check(
        batch.get(19, row: 0, as: DateComponents.self)
            == .success(DateComponents(hour: 15, minute: 4, second: 5, nanosecond: 500_000_000)),
        "time")
    check(batch.get(20, row: 0, as: Duration.self) == .success(.seconds(5400)), "duration")
    check(batch.string(21, row: 0) == "a \"quoted\" cell", "text, unescaped")
    check(
        batch.values(3, as: Int32.self).first == -1234 && batch.verdicts(3).first?.isOk == true,
        "a column as a buffer")

    // A cell that does not cast: the union's fault case, its text still to hand, and
    // HyperCast's own door — the other linked-in core — saying the same of the same text.
    let raw = batch.raw(22, row: 0)
    check(text(raw) == "a \"quoted\" cell", "raw text of a cell that failed to cast")
    let judged = try Cast.i32(raw, format: .invariant)
    check(batch.get(22, row: 0, as: Int32.self) == judged, "the cell's verdict is HyperCast's")
    check(judged == .fault(Fault(reason: .malformed, offset: 0, length: 1)), "a fault says where")
    check(try reader.read() == nil, "end of input")

    // A structural failure is an error, after the intact rows.
    let broken = try DelimitedReader(bytes: Array("a,b\n1,2\n3\n".utf8), dialect: .csv, plan: [.i32(0)])
    check(try broken.read()?.rows == 1, "the intact row before the break")
    do {
        _ = try broken.read()
        check(false, "a record of the wrong width was accepted")
    } catch let error as TabularError {
        check(
            error == TabularError(kind: .columnCount, record: 2, line: 3, byte: 8, expected: 2, found: 1),
            "structural failure, got \(error)")
    }

    // A workbook: opened, its sheets listed, its strings and styles loaded, a sheet
    // positioned on, its header read and its rows filled — every workbook entry point,
    // through the same batch.
    let book = try Workbook(bytes: basicOds)
    check(book.format == .ods && book.sheets.map(\.name) == ["Data", "Second"], "workbook")
    let sheet = try book.sheet(named: "Data", plan: [.i64(0), .text(8), .decimal(1)])
    check(sheet.header?.first == "id", "sheet header")
    var rows = 0
    while let batch = try sheet.read() {
        rows += batch.rows
    }
    check(rows == 3, "sheet rows (\(rows))")
    do {
        _ = try Workbook(bytes: Array("not a zip".utf8))
        check(false, "not a workbook was opened")
    } catch let error as TabularError {
        check(error.kind == .notAZip, "not a workbook, got \(error)")
    }

    print("hypertabular \(version) smoke test passed")
} catch {
    print("FAILED: \(error)")
    exit(1)
}

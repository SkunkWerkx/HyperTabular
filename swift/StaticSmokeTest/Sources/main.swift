import Foundation
import HyperTabular

// Every native entry point the binding declares, crossed through the public API, with the
// answers checked: the version probe, the header, the fill through all twenty-two doors
// (from a stream read through a buffer too small for the record), the unescape behind a
// raw read, and a structural failure. HyperCast's own door is asked the same question
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
    check(try reader.read() && reader.rows == 1, "read")
    check(reader.bool(0, row: 0) == .success(true), "bool")
    check(reader.i8(1, row: 0) == .success(-128), "i8")
    check(reader.i16(2, row: 0) == .success(32767), "i16")
    check(reader.i32(3, row: 0) == .success(-1234), "i32, accounting negative")
    check(reader.i64(4, row: 0) == .success(.max), "i64 max")
    check(reader.u8(5, row: 0) == .success(255), "u8")
    check(reader.u16(6, row: 0) == .success(65535), "u16")
    check(reader.u32(7, row: 0) == .success(4_294_967_295), "u32")
    check(reader.u64(8, row: 0) == .success(.max), "u64 max")
    check(reader.f32(9, row: 0) == .success(2.5), "f32")
    check(reader.f64(10, row: 0) == .success(0.255), "f64, percent")
    check(reader.decimal(11, row: 0) == .success(Decimal(string: "1234.5")!), "decimal is exact")
    check(
        reader.uuid(12, row: 0) == .success(UUID(uuidString: "6ba7b810-9dad-11d1-80b4-00c04fd430c8")!), "uuid")
    check(reader.timestamp(13, row: 0) == .success(Date(timeIntervalSince1970: 1_767_348_245)), "timestamp")
    check(reader.timestamp(14, row: 0) == .success(Date(timeIntervalSince1970: 1_700_000_000.123)), "unix")
    check(reader.timestamp(15, row: 0) == .success(Date(timeIntervalSince1970: 1_704_132_000)), "excel serial")
    check(reader.date(16, row: 0) == .success(DateComponents(year: 2024, month: 1, day: 31)), "date")
    check(reader.date(17, row: 0) == .success(DateComponents(year: 2026, month: 1, day: 7)), "ordered date")
    check(
        reader.dateTime(18, row: 0)
            == .success(DateComponents(year: 2026, month: 1, day: 7, hour: 15, minute: 4, second: 0, nanosecond: 0)),
        "civil date-time")
    check(
        reader.time(19, row: 0)
            == .success(DateComponents(hour: 15, minute: 4, second: 5, nanosecond: 500_000_000)),
        "time")
    check(reader.duration(20, row: 0) == .success(.seconds(5400)), "duration")
    check(reader.string(21, row: 0) == "a \"quoted\" cell", "text, unescaped")
    check(
        reader.values(3, as: Int32.self).first == -1234 && reader.verdicts(3).first?.isOk == true,
        "a column as a buffer")

    // A cell that does not cast: the union's fault case, its text still to hand, and
    // HyperCast's own door — the other linked-in core — saying the same of the same text.
    let raw = reader.raw(22, row: 0)
    check(text(raw) == "a \"quoted\" cell", "raw text of a cell that failed to cast")
    let judged = try Cast.i32(raw, format: .invariant)
    check(reader.i32(22, row: 0) == judged, "the cell's verdict is HyperCast's")
    check(judged == .fault(Fault(reason: .malformed, offset: 0, length: 1)), "a fault says where")
    check(try !reader.read(), "end of input")

    // A structural failure is an error, after the intact rows.
    let broken = try DelimitedReader(bytes: Array("a,b\n1,2\n3\n".utf8), dialect: .csv, plan: [.i32(0)])
    check(try broken.read() && broken.rows == 1, "the intact row before the break")
    do {
        _ = try broken.read()
        check(false, "a record of the wrong width was accepted")
    } catch let error as TabularError {
        check(
            error == TabularError(kind: .columnCount, record: 2, line: 3, byte: 8, expected: 2, found: 1),
            "structural failure, got \(error)")
    }

    print("hypertabular \(version) smoke test passed")
} catch {
    print("FAILED: \(error)")
    exit(1)
}

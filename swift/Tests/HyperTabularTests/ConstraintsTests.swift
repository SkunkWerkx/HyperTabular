import Foundation
import HyperTabularCore
import XCTest

@testable import HyperTabular

/// A column that declares HyperCast's constraints: what the corpus does not reach of the binding's surface.
final class ConstraintsTests: XCTestCase {
    private static let headerless = Dialect(separator: ",", hasHeader: false)

    private func read(_ csv: String, _ plan: [Column]) throws -> (DelimitedReader, Batch) {
        let reader = try DelimitedReader(bytes: Array(csv.utf8), dialect: Self.headerless, plan: plan)
        return (reader, try XCTUnwrap(try reader.read()))
    }

    func testACheckedColumnReadsAViolation() throws {
        let (reader, batch) = try read("11\n7\nx\n", [.i32(0, constraints: Constraints<Int32>(maximum: 10))])
        defer { _ = reader }
        XCTAssertTrue(batch.columns[0].isChecked)

        XCTAssertEqual(
            batch.checked(0, row: 0, as: Int32.self),
            .rejection(.violation(Violation(keyword: .maximum, value: 11, offset: 0, length: 2))))
        XCTAssertTrue(batch.verdict(0, row: 0).isViolation)
        XCTAssertFalse(batch.verdict(0, row: 0).isOk)
        XCTAssertNil(batch.fault(0, row: 0), "a violation is no fault: the cell cast")
        XCTAssertEqual(batch.values(0, as: Int32.self)[0], 11, "the value as cast, kept")

        XCTAssertEqual(
            batch.checked(0, row: 1, as: Int32.self), .success(Checked(value: 7, adjusted: false, enumIndex: nil)))
        guard case .rejection(.fault(let fault)) = batch.checked(0, row: 2, as: Int32.self) else {
            return XCTFail("text that isn't the type is a fault")
        }
        XCTAssertEqual(fault.reason, .malformed)
        XCTAssertEqual(batch[1].checked(0, as: Int32.self), batch.checked(0, row: 1, as: Int32.self))
    }

    func testARoundedValueIsFlaggedAdjusted() throws {
        let (reader, batch) = try read(
            "1.3\n1.5\n", [.decimal(0, constraints: Constraints<Decimal>(multipleOf: "0.5", rounding: .floor))])
        defer { _ = reader }
        XCTAssertEqual(
            batch.checked(0, row: 0, as: Decimal.self), .success(Checked(value: 1, adjusted: true, enumIndex: nil)))
        XCTAssertEqual(
            batch.checked(0, row: 1, as: Decimal.self),
            .success(Checked(value: Decimal(string: "1.5")!, adjusted: false, enumIndex: nil)))
    }

    func testATextEnumMatchDeliversTheDeclaredEntry() throws {
        let declared = Constraints<String>(enum: ["Female", "Male"])
        let (reader, batch) = try read("MALE\nother\n", [.text(0, constraints: declared)])
        defer { _ = reader }
        XCTAssertEqual(
            batch.checked(0, row: 0, as: String.self), .success(Checked(value: "Male", adjusted: false, enumIndex: 1)))
        XCTAssertEqual(
            batch.checked(0, row: 1, as: String.self),
            .rejection(.violation(Violation(keyword: .enum, value: "other", offset: 0, length: 5))))
    }

    /// An unconstrained column under the checked getter: what cast passes, unadjusted, and a fault is the
    /// same fault. No stale metadata can reach it: an unconstrained column is given no checks array.
    func testAnUnconstrainedColumnKeepsItsFaultsUnderTheCheckedGetter() throws {
        let (reader, batch) = try read("5\n\"\"\nx\n", [.i32(0)])
        defer { _ = reader }
        XCTAssertFalse(batch.columns[0].isChecked)
        XCTAssertEqual(
            batch.checked(0, row: 0, as: Int32.self), .success(Checked(value: 5, adjusted: false, enumIndex: nil)))
        XCTAssertEqual(batch.checked(0, row: 1, as: Int32.self), .rejection(.fault(batch.fault(0, row: 1)!)))
        XCTAssertEqual(batch.fault(0, row: 1)?.reason, .empty)
        XCTAssertEqual(batch.checked(0, row: 2, as: Int32.self), .rejection(.fault(batch.fault(0, row: 2)!)))
        XCTAssertEqual(batch.fault(0, row: 2)?.reason, .malformed)
    }

    #if os(macOS) || os(Linux)
        func testThePlainGettersRefuseAConstrainedColumn() {
            func opened() -> Batch {
                let reader = try! DelimitedReader(
                    bytes: Array("7,a\n".utf8), dialect: Self.headerless,
                    plan: [
                        .i32(0, constraints: Constraints<Int32>(maximum: 10)),
                        .text(1, constraints: Constraints<String>(maxLength: 3)),
                    ])
                return try! reader.read()!
            }
            XCTAssertEqual(Support.ending { _ = opened().get(0, row: 0, as: Int32.self) }, .trapped)
            XCTAssertEqual(Support.ending { _ = opened().text(1, row: 0) }, .trapped)
            XCTAssertEqual(Support.ending { _ = opened().string(1, row: 0) }, .trapped)
            XCTAssertEqual(Support.ending { _ = opened()[0].get(0, as: Int32.self) }, .trapped)
            // The checked getter reads the door's own type, and no other.
            XCTAssertEqual(Support.ending { _ = opened().checked(0, row: 0, as: Int64.self) }, .trapped)
            XCTAssertEqual(Support.ending { _ = opened().checked(1, row: 0, as: Int32.self) }, .trapped)
            // The control: the same reads of the checked getter return.
            XCTAssertEqual(
                Support.ending {
                    let batch = opened()
                    _ = batch.checked(0, row: 0, as: Int32.self)
                    _ = batch.checked(1, row: 0, as: String.self)
                }, .returned)
        }
    #endif

    func testFaultsListAViolationWithItsKeyword() throws {
        let (reader, batch) = try read("11\nx\n", [.i32(0, constraints: Constraints<Int32>(maximum: 10))])
        defer { _ = reader }
        let listed = Array(batch.faults())
        XCTAssertEqual(listed.count, 2)
        XCTAssertEqual(listed[0].keyword, .maximum)
        XCTAssertNil(listed[0].reason)
        XCTAssertEqual([listed[0].offset, listed[0].length], [0, 2])
        XCTAssertNil(listed[1].keyword)
        XCTAssertEqual(listed[1].reason, .malformed)
    }

    func testChecksAreSizedLikeVerdicts() throws {
        let columns = try Columns(
            plan: [.i32(0, constraints: Constraints<Int32>(maximum: 10)), .i32(0)], batchRows: 37)
        XCTAssertNotNil(columns.checkBase[0])
        XCTAssertNil(columns.checkBase[1], "an unconstrained column is given none")
        // Each column's checks are allocated for the same rows as its verdicts: the core writes the
        // 37th check beside the 37th verdict.
        let reader = try DelimitedReader(
            bytes: Array(String(repeating: "11\n", count: 37).utf8), dialect: Self.headerless,
            plan: [.i32(0, constraints: Constraints<Int32>(maximum: 10))], batchRows: 37)
        let batch = try XCTUnwrap(try reader.read())
        XCTAssertEqual(batch.rows, 37)
        XCTAssertEqual(
            batch.checked(0, row: 36, as: Int32.self),
            .rejection(.violation(Violation(keyword: .maximum, value: 11, offset: 0, length: 2))))
    }

    func testTheSameDeclarationMakesEqualColumns() {
        // A column is a value: the same door, ordinal and declaration are the same column, and a
        // declaration makes a column differ from the one with none.
        XCTAssertEqual(
            Column.i32(0, constraints: Constraints<Int32>(maximum: 10)),
            .i32(0, constraints: Constraints<Int32>(maximum: 10)))
        XCTAssertNotEqual(Column.i32(0, constraints: Constraints<Int32>(maximum: 10)), .i32(0))
        XCTAssertNotEqual(
            Column.i32(0, constraints: Constraints<Int32>(maximum: 10)),
            .i32(0, constraints: Constraints<Int32>(maximum: 11)))
    }
}

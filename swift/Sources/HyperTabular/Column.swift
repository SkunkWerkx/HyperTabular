import Foundation
import HyperTabularCore

/// The door a column is cast through: HyperCast's, plus ``text`` for the bytes themselves.
/// Named as HyperCast's own doors are; the raw values are the native core's door codes.
public enum Door: UInt32, Equatable, Sendable {
    /// `Bool`, HyperCast's boolean lexicon.
    case bool = 1
    /// `Int8`.
    case i8 = 2
    /// `Int16`.
    case i16 = 3
    /// `Int32`.
    case i32 = 4
    /// `Int64`.
    case i64 = 5
    /// `UInt8`.
    case u8 = 6
    /// `UInt16`.
    case u16 = 7
    /// `UInt32`.
    case u32 = 8
    /// `UInt64`.
    case u64 = 9
    /// `Float`.
    case f32 = 10
    /// `Double`.
    case f64 = 11
    /// Foundation's `UUID`.
    case uuid = 12
    /// An RFC 3339 instant, as Foundation's `Date`.
    case timestamp = 13
    /// A Unix-epoch integer at a declared precision, as `Date`.
    case unix = 14
    /// A strict `yyyy-MM-dd` date, as `DateComponents`.
    case date = 15
    /// A 24-hour time of day, as `DateComponents`.
    case time = 16
    /// A duration, as Swift's `Duration`.
    case duration = 17
    /// The cell's bytes themselves — no cast.
    case text = 18
    /// An exact Foundation `Decimal`; no float is ever formed.
    case decimal = 19
    /// A separated calendar date under a declared field order, as `DateComponents`.
    case dateOrdered = 20
    /// A zone-less civil date-time under a declared field order, as `DateComponents`.
    case dateTime = 21
    /// An Excel date serial under a declared date system, as `Date`.
    case excelSerial = 22
}

/// One output column of a plan: which source column it reads, the door it casts through,
/// and — for the numeric doors — the notation. A plan is a projection: a forty-column file
/// can be read into five typed columns, in any order, and a source column can be read
/// through more than one door.
///
/// One factory per door, named as HyperCast's `Cast` names them:
///
/// ```swift
/// let plan: [Column] = [.i32(0), .text(1), .decimal(2, format: .invariant), .date(5, order: .dayMonthYear)]
/// ```
public struct Column: Equatable, Sendable {
    /// Zero-based ordinal of the source column. Past a record's last cell reads as empty;
    /// at or past the columns a delimited header gives every record, and 2²⁰ and past, more
    /// columns than any workbook format has, a plan is refused at bind with
    /// ``PlanError/outOfReach(column:)``.
    public let ordinal: Int
    /// The door.
    public let door: Door
    /// The numeric notation, read by the numeric doors.
    public let format: NumFormat
    /// What stands for no value in this column: `nil` takes the dialect's or the sheet's. See
    /// ``withTokens(_:)``.
    public let tokens: [String]?
    /// The forms the date and time doors take beside their strict ones: see ``withDateOptions(_:)``.
    public let dateOptions: DateOptions
    /// What the door declares beside itself, as the core numbers it.
    let declared: UInt32
    /// What the column's cells must also meet, as the core reads it: `nil` declares none.
    let constraints: Declaration?

    /// Whether the column declares HyperCast's constraints, and so is read with
    /// ``Batch/checked(_:row:as:)``.
    public var isChecked: Bool { constraints != nil }

    init(
        _ ordinal: Int, _ door: Door, declared: UInt32 = 0, format: NumFormat = .invariant,
        constraints: Declaration? = nil
    ) {
        precondition(ordinal >= 0, "A column's ordinal is zero-based; got \(ordinal)")
        self.ordinal = ordinal
        self.door = door
        self.declared = declared
        self.format = format
        self.constraints = constraints
        tokens = nil
        dateOptions = .none
    }

    private init(_ column: Column, tokens: [String]?, dateOptions: DateOptions) {
        ordinal = column.ordinal
        door = column.door
        declared = column.declared
        format = column.format
        constraints = column.constraints
        self.tokens = tokens
        self.dateOptions = dateOptions
    }

    /// A declaration of HyperCast's constraints with its type let go, as a column holds it: the
    /// 104 bytes the core reads, the packed `enum`, and at the text door the entries as declared,
    /// which a match hands back. The factories take `Constraints<T>` of the door's own type, so a
    /// column can't pair one door with another's.
    struct Declaration: Equatable, Sendable {
        let raw: [UInt8]
        let packed: [UInt8]
        let entries: [String]?

        init<T>(_ constraints: Constraints<T>) {
            raw = withUnsafeBytes(of: Interop.rawConstraints(constraints)) { Array($0) }
            packed = Interop.packedEnum(constraints)
            entries = T.self == String.self ? constraints.enum?.map(\.description) : nil
        }
    }

    /// This column, with tokens of its own in place of the dialect's or the sheet's: a cell whose
    /// exact, unquoted bytes are one of them is empty, at ``Origin/token``, whatever the door.
    /// Matched as the tokens' UTF-8 bytes. `[]` is none at all, where a list elsewhere would apply;
    /// a typed workbook cell is never a token.
    public func withTokens(_ tokens: [String]) -> Column {
        Column(self, tokens: tokens, dateOptions: dateOptions)
    }

    /// This column, reading the forms `options` allows beside its door's strict ones, in the
    /// doors that take them: ``Door/date``, ``Door/dateOrdered``, ``Door/dateTime``,
    /// ``Door/timestamp`` and ``Door/time``. The others ignore them.
    public func withDateOptions(_ options: DateOptions) -> Column {
        Column(self, tokens: tokens, dateOptions: options)
    }

    /// Whether `door` reads a number under a ``NumFormat``.
    static func isNumeric(_ door: Door) -> Bool {
        switch door {
        case .i8, .i16, .i32, .i64, .u8, .u16, .u32, .u64, .f32, .f64, .decimal: true
        default: false
        }
    }

    /// A `Bool` column.
    public static func bool(_ ordinal: Int) -> Column { Column(ordinal, .bool) }

    /// An `Int8` column under a declared notation — the invariant one unless stated.
    public static func i8(_ ordinal: Int, format: NumFormat = .invariant) -> Column {
        Column(ordinal, .i8, format: format)
    }

    /// An `Int8` column whose cells must also meet `constraints`: read with
    /// ``Batch/checked(_:row:as:)``.
    public static func i8(
        _ ordinal: Int, format: NumFormat = .invariant, constraints: Constraints<Int8>
    ) -> Column {
        Column(ordinal, .i8, format: format, constraints: Declaration(constraints))
    }

    /// An `Int16` column under a declared notation — the invariant one unless stated.
    public static func i16(_ ordinal: Int, format: NumFormat = .invariant) -> Column {
        Column(ordinal, .i16, format: format)
    }

    /// An `Int16` column whose cells must also meet `constraints`: read with
    /// ``Batch/checked(_:row:as:)``.
    public static func i16(
        _ ordinal: Int, format: NumFormat = .invariant, constraints: Constraints<Int16>
    ) -> Column {
        Column(ordinal, .i16, format: format, constraints: Declaration(constraints))
    }

    /// An `Int32` column under a declared notation — the invariant one unless stated.
    public static func i32(_ ordinal: Int, format: NumFormat = .invariant) -> Column {
        Column(ordinal, .i32, format: format)
    }

    /// An `Int32` column whose cells must also meet `constraints`: read with
    /// ``Batch/checked(_:row:as:)``.
    public static func i32(
        _ ordinal: Int, format: NumFormat = .invariant, constraints: Constraints<Int32>
    ) -> Column {
        Column(ordinal, .i32, format: format, constraints: Declaration(constraints))
    }

    /// An `Int64` column under a declared notation — the invariant one unless stated.
    public static func i64(_ ordinal: Int, format: NumFormat = .invariant) -> Column {
        Column(ordinal, .i64, format: format)
    }

    /// An `Int64` column whose cells must also meet `constraints`: read with
    /// ``Batch/checked(_:row:as:)``.
    public static func i64(
        _ ordinal: Int, format: NumFormat = .invariant, constraints: Constraints<Int64>
    ) -> Column {
        Column(ordinal, .i64, format: format, constraints: Declaration(constraints))
    }

    /// A `UInt8` column under a declared notation — the invariant one unless stated.
    public static func u8(_ ordinal: Int, format: NumFormat = .invariant) -> Column {
        Column(ordinal, .u8, format: format)
    }

    /// An `UInt8` column whose cells must also meet `constraints`: read with
    /// ``Batch/checked(_:row:as:)``.
    public static func u8(
        _ ordinal: Int, format: NumFormat = .invariant, constraints: Constraints<UInt8>
    ) -> Column {
        Column(ordinal, .u8, format: format, constraints: Declaration(constraints))
    }

    /// A `UInt16` column under a declared notation — the invariant one unless stated.
    public static func u16(_ ordinal: Int, format: NumFormat = .invariant) -> Column {
        Column(ordinal, .u16, format: format)
    }

    /// An `UInt16` column whose cells must also meet `constraints`: read with
    /// ``Batch/checked(_:row:as:)``.
    public static func u16(
        _ ordinal: Int, format: NumFormat = .invariant, constraints: Constraints<UInt16>
    ) -> Column {
        Column(ordinal, .u16, format: format, constraints: Declaration(constraints))
    }

    /// A `UInt32` column under a declared notation — the invariant one unless stated.
    public static func u32(_ ordinal: Int, format: NumFormat = .invariant) -> Column {
        Column(ordinal, .u32, format: format)
    }

    /// An `UInt32` column whose cells must also meet `constraints`: read with
    /// ``Batch/checked(_:row:as:)``.
    public static func u32(
        _ ordinal: Int, format: NumFormat = .invariant, constraints: Constraints<UInt32>
    ) -> Column {
        Column(ordinal, .u32, format: format, constraints: Declaration(constraints))
    }

    /// A `UInt64` column under a declared notation — the invariant one unless stated.
    public static func u64(_ ordinal: Int, format: NumFormat = .invariant) -> Column {
        Column(ordinal, .u64, format: format)
    }

    /// An `UInt64` column whose cells must also meet `constraints`: read with
    /// ``Batch/checked(_:row:as:)``.
    public static func u64(
        _ ordinal: Int, format: NumFormat = .invariant, constraints: Constraints<UInt64>
    ) -> Column {
        Column(ordinal, .u64, format: format, constraints: Declaration(constraints))
    }

    /// A `Float` column under a declared notation — the invariant one unless stated.
    public static func f32(_ ordinal: Int, format: NumFormat = .invariant) -> Column {
        Column(ordinal, .f32, format: format)
    }

    /// A `Float` column whose cells must also meet `constraints`: read with
    /// ``Batch/checked(_:row:as:)``.
    public static func f32(
        _ ordinal: Int, format: NumFormat = .invariant, constraints: Constraints<Float>
    ) -> Column {
        Column(ordinal, .f32, format: format, constraints: Declaration(constraints))
    }

    /// A `Double` column under a declared notation — the invariant one unless stated.
    public static func f64(_ ordinal: Int, format: NumFormat = .invariant) -> Column {
        Column(ordinal, .f64, format: format)
    }

    /// A `Double` column whose cells must also meet `constraints`: read with
    /// ``Batch/checked(_:row:as:)``.
    public static func f64(
        _ ordinal: Int, format: NumFormat = .invariant, constraints: Constraints<Double>
    ) -> Column {
        Column(ordinal, .f64, format: format, constraints: Declaration(constraints))
    }

    /// An exact `Decimal` column under a declared notation — the invariant one unless stated.
    public static func decimal(_ ordinal: Int, format: NumFormat = .invariant) -> Column {
        Column(ordinal, .decimal, format: format)
    }

    /// An exact `Decimal` column whose cells must also meet `constraints`: read with
    /// ``Batch/checked(_:row:as:)``.
    public static func decimal(
        _ ordinal: Int, format: NumFormat = .invariant, constraints: Constraints<Decimal>
    ) -> Column {
        Column(ordinal, .decimal, format: format, constraints: Declaration(constraints))
    }

    /// A `UUID` column.
    public static func uuid(_ ordinal: Int) -> Column { Column(ordinal, .uuid) }

    /// An RFC 3339 instant column.
    public static func timestamp(_ ordinal: Int) -> Column { Column(ordinal, .timestamp) }

    /// A Unix-epoch column at the declared precision — never guessed from magnitude.
    public static func unix(_ ordinal: Int, precision: UnixPrecision) -> Column {
        Column(ordinal, .unix, declared: precision.rawValue)
    }

    /// An Excel date-serial column under the declared date system.
    public static func excelSerial(_ ordinal: Int, epoch: ExcelEpoch) -> Column {
        Column(ordinal, .excelSerial, declared: epoch.rawValue)
    }

    /// A strict `yyyy-MM-dd` date column.
    public static func date(_ ordinal: Int) -> Column { Column(ordinal, .date) }

    /// A separated-date column under the declared field order.
    public static func date(_ ordinal: Int, order: DateOrder) -> Column {
        Column(ordinal, .dateOrdered, declared: order.rawValue)
    }

    /// A zone-less civil date-time column under the declared field order.
    public static func dateTime(_ ordinal: Int, order: DateOrder) -> Column {
        Column(ordinal, .dateTime, declared: order.rawValue)
    }

    /// A 24-hour time-of-day column.
    public static func time(_ ordinal: Int) -> Column { Column(ordinal, .time) }

    /// A duration column.
    public static func duration(_ ordinal: Int) -> Column { Column(ordinal, .duration) }

    /// A text column: the cell's bytes themselves, untrimmed.
    public static func text(_ ordinal: Int) -> Column { Column(ordinal, .text) }

    /// A text column whose cells must also meet `constraints`: read with
    /// ``Batch/checked(_:row:as:)``, where an `enum` match reads as the entry as declared.
    public static func text(_ ordinal: Int, constraints: Constraints<String>) -> Column {
        Column(ordinal, .text, constraints: Declaration(constraints))
    }

    /// Bytes one value of this column's door takes in a column buffer.
    var valueSize: Int {
        switch door {
        case .bool, .i8, .u8: 1
        case .i16, .u16: 2
        case .i32, .u32, .f32, .date, .dateOrdered: 4
        case .i64, .u64, .f64, .time, .text: 8
        case .uuid, .timestamp, .unix, .excelSerial, .decimal, .dateTime, .duration: 16
        }
    }

    /// The column as the core reads it, the format laid out by HyperCast as its own doors lay
    /// it out (the 92-byte size-tagged layout). What it points at, its tokens and its months,
    /// ``Columns`` holds and adds. `NumFormat` has already refused what the native format cannot
    /// carry when it was made; a format that got past that (a stale grouping word) is what
    /// ``Columns``'s judge refuses, as ``PlanError/refused(column:)``.
    var spec: HyperTabularColumnSpecWith {
        var spec = HyperTabularColumnSpecWith()
        spec.size = columnSpecSize
        spec.ordinal = UInt32(ordinal)
        spec.door = door.rawValue
        spec.param = declared
        var format = Interop.rawFormat(self.format)
        withUnsafeBytes(of: &format) { source in
            withUnsafeMutableBytes(of: &spec.format) { $0.copyMemory(from: source) }
        }
        return spec
    }
}

/// The closed set of types a column's values can be viewed as in place, a whole batch at a
/// time (``Batch/values(_:as:)``): the doors whose value is a primitive the core
/// writes as Swift lays it out — and `UInt64`, too, for ``Door/time``. The conformances are this binding's to declare — one per
/// such door, below — so conforming anything else is unsupported.
public protocol ColumnValue {
    /// The door that writes this type.
    static var columnDoor: Door { get }
}

extension Bool: ColumnValue {
    /// ``Door/bool``.
    public static var columnDoor: Door { .bool }
}

extension Int8: ColumnValue {
    /// ``Door/i8``.
    public static var columnDoor: Door { .i8 }
}

extension Int16: ColumnValue {
    /// ``Door/i16``.
    public static var columnDoor: Door { .i16 }
}

extension Int32: ColumnValue {
    /// ``Door/i32``.
    public static var columnDoor: Door { .i32 }
}

extension Int64: ColumnValue {
    /// ``Door/i64``.
    public static var columnDoor: Door { .i64 }
}

extension UInt8: ColumnValue {
    /// ``Door/u8``.
    public static var columnDoor: Door { .u8 }
}

extension UInt16: ColumnValue {
    /// ``Door/u16``.
    public static var columnDoor: Door { .u16 }
}

extension UInt32: ColumnValue {
    /// ``Door/u32``.
    public static var columnDoor: Door { .u32 }
}

extension UInt64: ColumnValue {
    /// ``Door/u64``.
    public static var columnDoor: Door { .u64 }
}

extension Float: ColumnValue {
    /// ``Door/f32``.
    public static var columnDoor: Door { .f32 }
}

extension Double: ColumnValue {
    /// ``Door/f64``.
    public static var columnDoor: Door { .f64 }
}

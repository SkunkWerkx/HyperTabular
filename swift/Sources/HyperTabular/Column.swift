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
    /// Zero-based ordinal of the source column. Past a record's last cell reads as empty.
    public let ordinal: Int
    /// The door.
    public let door: Door
    /// The numeric notation, read by the numeric doors.
    public let format: NumFormat
    /// What the door declares beside itself, as the core numbers it.
    let declared: UInt32

    private init(_ ordinal: Int, _ door: Door, declared: UInt32 = 0, format: NumFormat = .invariant) {
        precondition(
            ordinal >= 0 && ordinal < Int(Int32.max), "A column's ordinal is zero-based; got \(ordinal)")
        self.ordinal = ordinal
        self.door = door
        self.declared = declared
        self.format = format
    }

    /// A `Bool` column.
    public static func bool(_ ordinal: Int) -> Column { Column(ordinal, .bool) }

    /// An `Int8` column under a declared notation — the invariant one unless stated.
    public static func i8(_ ordinal: Int, format: NumFormat = .invariant) -> Column {
        Column(ordinal, .i8, format: format)
    }

    /// An `Int16` column under a declared notation — the invariant one unless stated.
    public static func i16(_ ordinal: Int, format: NumFormat = .invariant) -> Column {
        Column(ordinal, .i16, format: format)
    }

    /// An `Int32` column under a declared notation — the invariant one unless stated.
    public static func i32(_ ordinal: Int, format: NumFormat = .invariant) -> Column {
        Column(ordinal, .i32, format: format)
    }

    /// An `Int64` column under a declared notation — the invariant one unless stated.
    public static func i64(_ ordinal: Int, format: NumFormat = .invariant) -> Column {
        Column(ordinal, .i64, format: format)
    }

    /// A `UInt8` column under a declared notation — the invariant one unless stated.
    public static func u8(_ ordinal: Int, format: NumFormat = .invariant) -> Column {
        Column(ordinal, .u8, format: format)
    }

    /// A `UInt16` column under a declared notation — the invariant one unless stated.
    public static func u16(_ ordinal: Int, format: NumFormat = .invariant) -> Column {
        Column(ordinal, .u16, format: format)
    }

    /// A `UInt32` column under a declared notation — the invariant one unless stated.
    public static func u32(_ ordinal: Int, format: NumFormat = .invariant) -> Column {
        Column(ordinal, .u32, format: format)
    }

    /// A `UInt64` column under a declared notation — the invariant one unless stated.
    public static func u64(_ ordinal: Int, format: NumFormat = .invariant) -> Column {
        Column(ordinal, .u64, format: format)
    }

    /// A `Float` column under a declared notation — the invariant one unless stated.
    public static func f32(_ ordinal: Int, format: NumFormat = .invariant) -> Column {
        Column(ordinal, .f32, format: format)
    }

    /// A `Double` column under a declared notation — the invariant one unless stated.
    public static func f64(_ ordinal: Int, format: NumFormat = .invariant) -> Column {
        Column(ordinal, .f64, format: format)
    }

    /// An exact `Decimal` column under a declared notation — the invariant one unless stated.
    public static func decimal(_ ordinal: Int, format: NumFormat = .invariant) -> Column {
        Column(ordinal, .decimal, format: format)
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

    /// The column as the core reads it. HyperCast's `NumFormat` has already refused what the
    /// native format cannot carry — equal separators, a symbol over 16 UTF-8 bytes or with
    /// a digit or whitespace in it — so nothing here can fail.
    var spec: hypertabular_column_spec {
        var spec = hypertabular_column_spec()
        spec.ordinal = UInt32(ordinal)
        spec.door = door.rawValue
        spec.param = declared
        spec.format.decimal_sep = format.decimalSeparator.value
        spec.format.group_sep = format.groupSeparator.value
        spec.format.flags = format.styles.rawValue
        withUnsafeMutableBytes(of: &spec.format.currency) { currency in
            var length = 0
            for byte in format.currencySymbol.utf8.prefix(currency.count) {
                currency[length] = byte
                length += 1
            }
            spec.format.currency_len = UInt32(length)
        }
        return spec
    }
}

/// The closed set of types a column's values can be viewed as in place, a whole batch at a
/// time (``DelimitedReader/values(_:as:)``): the doors whose value is a primitive the core
/// writes as Swift lays it out. The conformances are this binding's to declare — one per
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

using HyperCast;
using HyperCast.Interop;

namespace HyperTabular;

/// <summary>The door a column is cast through: HyperCast's, plus <see cref="Text"/> for the bytes themselves.</summary>
public enum Door : byte
{
	/// <summary>Sentinel CLR default — never a door.</summary>
	Unspecified = 0,
	/// <summary><see cref="bool"/>, HyperCast's boolean lexicon.</summary>
	Boolean = 1,
	/// <summary><see cref="sbyte"/>.</summary>
	SByte = 2,
	/// <summary><see cref="short"/>.</summary>
	Int16 = 3,
	/// <summary><see cref="int"/>.</summary>
	Int32 = 4,
	/// <summary><see cref="long"/>.</summary>
	Int64 = 5,
	/// <summary><see cref="byte"/>.</summary>
	Byte = 6,
	/// <summary><see cref="ushort"/>.</summary>
	UInt16 = 7,
	/// <summary><see cref="uint"/>.</summary>
	UInt32 = 8,
	/// <summary><see cref="ulong"/>.</summary>
	UInt64 = 9,
	/// <summary><see cref="float"/>.</summary>
	Single = 10,
	/// <summary><see cref="double"/>.</summary>
	Double = 11,
	/// <summary><see cref="Guid"/>.</summary>
	Uuid = 12,
	/// <summary>An RFC 3339 instant, as <see cref="DateTimeOffset"/>.</summary>
	Timestamp = 13,
	/// <summary>A Unix-epoch integer at a declared precision, as <see cref="DateTimeOffset"/>.</summary>
	Unix = 14,
	/// <summary>A strict <c>yyyy-MM-dd</c> date, as <see cref="DateOnly"/>.</summary>
	Date = 15,
	/// <summary>A 24-hour time of day, as <see cref="TimeOnly"/>.</summary>
	Time = 16,
	/// <summary>A duration, as <see cref="TimeSpan"/>.</summary>
	Duration = 17,
	/// <summary>The cell's bytes themselves — no cast.</summary>
	Text = 18,
	/// <summary>An exact <see cref="decimal"/>; no float is ever formed.</summary>
	Decimal = 19,
	/// <summary>A separated calendar date under a declared field order, as <see cref="DateOnly"/>.</summary>
	DateOrdered = 20,
	/// <summary>A zone-less civil date-time under a declared field order, as <see cref="System.DateTime"/>.</summary>
	DateTime = 21,
	/// <summary>An Excel date serial under a declared date system, as <see cref="DateTimeOffset"/>.</summary>
	ExcelSerial = 22
}

/// <summary>
/// One output column of a plan: which source column it reads, the door it casts through,
/// and — for the numeric doors — the notation. A plan is a projection: a forty-column file
/// can be read into five typed columns, in any order, and a source column can be read
/// through more than one door.
/// </summary>
public readonly record struct Column
{
	Column(int ordinal, Door door, uint declared, NumFormat format)
	{
		ArgumentOutOfRangeException.ThrowIfNegative(ordinal);
		Ordinal = ordinal;
		Door = door;
		Declared = declared;
		Format = format;
	}

	/// <summary>Zero-based ordinal of the source column. Past a record's last cell reads as empty.</summary>
	public int Ordinal { get; }

	/// <summary>The door.</summary>
	public Door Door { get; }

	/// <summary>The numeric notation, read by the numeric doors.</summary>
	public NumFormat Format { get; }

	/// <summary>What the door declares beside itself, as the core numbers it.</summary>
	internal uint Declared { get; }

	/// <summary>A <see cref="bool"/> column.</summary>
	public static Column Boolean(int ordinal) => new(ordinal, Door.Boolean, 0, NumFormat.Invariant);

	/// <summary>An <see cref="sbyte"/> column under the invariant notation.</summary>
	public static Column SByte(int ordinal) => SByte(ordinal, NumFormat.Invariant);
	/// <summary>An <see cref="sbyte"/> column under a declared notation.</summary>
	public static Column SByte(int ordinal, NumFormat format) => new(ordinal, Door.SByte, 0, format);

	/// <summary>A <see cref="short"/> column under the invariant notation.</summary>
	public static Column Int16(int ordinal) => Int16(ordinal, NumFormat.Invariant);
	/// <summary>A <see cref="short"/> column under a declared notation.</summary>
	public static Column Int16(int ordinal, NumFormat format) => new(ordinal, Door.Int16, 0, format);

	/// <summary>An <see cref="int"/> column under the invariant notation.</summary>
	public static Column Int32(int ordinal) => Int32(ordinal, NumFormat.Invariant);
	/// <summary>An <see cref="int"/> column under a declared notation.</summary>
	public static Column Int32(int ordinal, NumFormat format) => new(ordinal, Door.Int32, 0, format);

	/// <summary>A <see cref="long"/> column under the invariant notation.</summary>
	public static Column Int64(int ordinal) => Int64(ordinal, NumFormat.Invariant);
	/// <summary>A <see cref="long"/> column under a declared notation.</summary>
	public static Column Int64(int ordinal, NumFormat format) => new(ordinal, Door.Int64, 0, format);

	/// <summary>A <see cref="byte"/> column under the invariant notation.</summary>
	public static Column Byte(int ordinal) => Byte(ordinal, NumFormat.Invariant);
	/// <summary>A <see cref="byte"/> column under a declared notation.</summary>
	public static Column Byte(int ordinal, NumFormat format) => new(ordinal, Door.Byte, 0, format);

	/// <summary>A <see cref="ushort"/> column under the invariant notation.</summary>
	public static Column UInt16(int ordinal) => UInt16(ordinal, NumFormat.Invariant);
	/// <summary>A <see cref="ushort"/> column under a declared notation.</summary>
	public static Column UInt16(int ordinal, NumFormat format) => new(ordinal, Door.UInt16, 0, format);

	/// <summary>A <see cref="uint"/> column under the invariant notation.</summary>
	public static Column UInt32(int ordinal) => UInt32(ordinal, NumFormat.Invariant);
	/// <summary>A <see cref="uint"/> column under a declared notation.</summary>
	public static Column UInt32(int ordinal, NumFormat format) => new(ordinal, Door.UInt32, 0, format);

	/// <summary>A <see cref="ulong"/> column under the invariant notation.</summary>
	public static Column UInt64(int ordinal) => UInt64(ordinal, NumFormat.Invariant);
	/// <summary>A <see cref="ulong"/> column under a declared notation.</summary>
	public static Column UInt64(int ordinal, NumFormat format) => new(ordinal, Door.UInt64, 0, format);

	/// <summary>A <see cref="float"/> column under the invariant notation.</summary>
	public static Column Single(int ordinal) => Single(ordinal, NumFormat.Invariant);
	/// <summary>A <see cref="float"/> column under a declared notation.</summary>
	public static Column Single(int ordinal, NumFormat format) => new(ordinal, Door.Single, 0, format);

	/// <summary>A <see cref="double"/> column under the invariant notation.</summary>
	public static Column Double(int ordinal) => Double(ordinal, NumFormat.Invariant);
	/// <summary>A <see cref="double"/> column under a declared notation.</summary>
	public static Column Double(int ordinal, NumFormat format) => new(ordinal, Door.Double, 0, format);

	/// <summary>An exact <see cref="decimal"/> column under the invariant notation.</summary>
	public static Column Decimal(int ordinal) => Decimal(ordinal, NumFormat.Invariant);
	/// <summary>An exact <see cref="decimal"/> column under a declared notation.</summary>
	public static Column Decimal(int ordinal, NumFormat format) => new(ordinal, Door.Decimal, 0, format);

	/// <summary>A <see cref="Guid"/> column.</summary>
	public static Column Uuid(int ordinal) => new(ordinal, Door.Uuid, 0, NumFormat.Invariant);

	/// <summary>An RFC 3339 instant column.</summary>
	public static Column Timestamp(int ordinal) => new(ordinal, Door.Timestamp, 0, NumFormat.Invariant);

	/// <summary>A Unix-epoch column at the declared precision — never guessed from magnitude.</summary>
	/// <exception cref="ArgumentOutOfRangeException"><paramref name="precision"/> is undefined.</exception>
	public static Column Unix(int ordinal, UnixPrecision precision) =>
		new(ordinal, Door.Unix, Abi.Code(precision), NumFormat.Invariant);

	/// <summary>An Excel date-serial column under the declared date system.</summary>
	/// <exception cref="ArgumentOutOfRangeException"><paramref name="epoch"/> is undefined.</exception>
	public static Column ExcelSerial(int ordinal, ExcelEpoch epoch) =>
		new(ordinal, Door.ExcelSerial, Abi.Code(epoch), NumFormat.Invariant);

	/// <summary>A strict <c>yyyy-MM-dd</c> date column.</summary>
	public static Column Date(int ordinal) => new(ordinal, Door.Date, 0, NumFormat.Invariant);

	/// <summary>A separated-date column under the declared field order.</summary>
	/// <exception cref="ArgumentOutOfRangeException"><paramref name="order"/> is undefined.</exception>
	public static Column Date(int ordinal, DateOrder order) =>
		new(ordinal, Door.DateOrdered, Abi.Code(order), NumFormat.Invariant);

	/// <summary>A zone-less civil date-time column under the declared field order.</summary>
	/// <exception cref="ArgumentOutOfRangeException"><paramref name="order"/> is undefined.</exception>
	public static Column DateTime(int ordinal, DateOrder order) =>
		new(ordinal, Door.DateTime, Abi.Code(order), NumFormat.Invariant);

	/// <summary>A 24-hour time-of-day column.</summary>
	public static Column Time(int ordinal) => new(ordinal, Door.Time, 0, NumFormat.Invariant);

	/// <summary>A duration column.</summary>
	public static Column Duration(int ordinal) => new(ordinal, Door.Duration, 0, NumFormat.Invariant);

	/// <summary>A text column: the cell's bytes themselves, untrimmed.</summary>
	public static Column Text(int ordinal) => new(ordinal, Door.Text, 0, NumFormat.Invariant);

	/// <summary>Bytes one value of this column's door takes in a column buffer.</summary>
	internal int ValueSize =>
		Door switch
		{
			Door.Boolean or Door.SByte or Door.Byte => 1,
			Door.Int16 or Door.UInt16 => 2,
			Door.Int32 or Door.UInt32 or Door.Single or Door.Date or Door.DateOrdered => 4,
			Door.Int64 or Door.UInt64 or Door.Double or Door.Time or Door.Text => 8,
			_ => 16,
		};
}

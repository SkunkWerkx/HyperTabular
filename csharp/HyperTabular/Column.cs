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
/// <remarks>
/// A numeric or text column may declare HyperCast's <see cref="Constraints{T}"/>, the OpenAPI
/// keywords, through the factories that take them: each cell is then cast and judged, and is read
/// with <see cref="Batch.GetChecked{T}"/>. The constraints' type is the door's, so a column can't
/// pair one door with another's constraints.
/// </remarks>
public readonly record struct Column
{
	internal Column(int ordinal, Door door, uint declared, NumFormat format)
	{
		ArgumentOutOfRangeException.ThrowIfNegative(ordinal);
		Ordinal = ordinal;
		Door = door;
		Declared = declared;
		Format = format;
	}

	/// <summary>Zero-based ordinal of the source column. Past a record's last cell reads as empty; 2²⁰ or more is refused when the plan is bound.</summary>
	public int Ordinal { get; }

	/// <summary>The door.</summary>
	public Door Door { get; }

	/// <summary>The numeric notation, read by the numeric doors.</summary>
	public NumFormat Format { get; }

	/// <summary>What the door declares beside itself, as the core numbers it.</summary>
	internal uint Declared { get; }

	/// <summary>The column's own tokens, in place of its dialect's or its sheet's; <see langword="null"/> to take theirs.</summary>
	public Tokens? Tokens { get; private init; }

	/// <summary>The date options and month vocabulary the date doors read: <c>default</c> for none.</summary>
	public DateOptions DateOptions { get; private init; }

	/// <summary>
	/// The column's <see cref="Constraints{T}"/>, of the door's own type, or <see langword="null"/>
	/// for a column that declares none.
	/// </summary>
	internal object? Constraints { get; private init; }

	/// <summary>
	/// Whether the column declares <see cref="Constraints{T}"/>: its cells are then read with
	/// <see cref="Batch.GetChecked{T}"/>, and <see cref="Batch.Get{T}"/> refuses it.
	/// </summary>
	public bool IsChecked => Constraints is not null;

	/// <summary>The column's constraints as the core reads them: the layout and the packed <c>enum</c>.</summary>
	internal (RawConstraints Raw, ReadOnlyMemory<byte> Enum)? Declaration =>
		Constraints switch
		{
			null => null,
			Constraints<sbyte> c => (c.ToRaw(), c.PackedEnum.ToArray()),
			Constraints<short> c => (c.ToRaw(), c.PackedEnum.ToArray()),
			Constraints<int> c => (c.ToRaw(), c.PackedEnum.ToArray()),
			Constraints<long> c => (c.ToRaw(), c.PackedEnum.ToArray()),
			Constraints<byte> c => (c.ToRaw(), c.PackedEnum.ToArray()),
			Constraints<ushort> c => (c.ToRaw(), c.PackedEnum.ToArray()),
			Constraints<uint> c => (c.ToRaw(), c.PackedEnum.ToArray()),
			Constraints<ulong> c => (c.ToRaw(), c.PackedEnum.ToArray()),
			Constraints<float> c => (c.ToRaw(), c.PackedEnum.ToArray()),
			Constraints<double> c => (c.ToRaw(), c.PackedEnum.ToArray()),
			Constraints<decimal> c => (c.ToRaw(), c.PackedEnum.ToArray()),
			Constraints<string> c => (c.ToRaw(), c.PackedEnum.ToArray()),
			_ => throw new InvalidOperationException("A column holds constraints of no door's type — a binding bug, please report it."),
		};

	/// <summary>The column, declaring <paramref name="constraints"/>.</summary>
	static Column Checked<T>(Column column, Constraints<T> constraints) where T : notnull =>
		column with { Constraints = constraints ?? throw new ArgumentNullException(nameof(constraints)) };

	/// <summary>
	/// The same column answering to these tokens rather than its dialect's or sheet's: a cell whose
	/// exact, unquoted bytes are one of them is empty, whatever its door, and
	/// <see cref="Batch.Origin"/> says <see cref="Origin.Token"/>. No tokens at all means none, so a
	/// column can opt out of its dialect's. On a sheet only a text cell can be one.
	/// </summary>
	/// <param name="tokens">The tokens.</param>
	public Column WithTokens(params ReadOnlySpan<string> tokens) => this with { Tokens = new Tokens(tokens) };

	/// <summary>
	/// The same column reading dates under these options and their month vocabulary: the
	/// <see cref="Door.Timestamp"/>, <see cref="Door.Date"/>, <see cref="Door.DateOrdered"/> and
	/// <see cref="Door.DateTime"/> doors read them, as HyperCast's own detailed doors do, and the
	/// others ignore them. On a sheet they reach only a text cell, since a typed date cell has no
	/// text for them to read. Options HyperCast refuses are the error of whatever binds the plan.
	/// </summary>
	/// <param name="options">The options, with <see cref="DateOptions.Months"/> if the column names its months.</param>
	public Column WithDateOptions(DateOptions options) => this with { DateOptions = options };

	/// <summary>Whether a door reads a <see cref="NumFormat"/>: the integers, the reals and the exact decimal.</summary>
	internal static bool IsNumeric(Door door) => door is (>= Door.SByte and <= Door.Double) or Door.Decimal;

	/// <summary>A <see cref="bool"/> column.</summary>
	public static Column Boolean(int ordinal) => new(ordinal, Door.Boolean, 0, NumFormat.Invariant);

	/// <summary>An <see cref="sbyte"/> column under the invariant notation.</summary>
	public static Column SByte(int ordinal) => SByte(ordinal, NumFormat.Invariant);
	/// <summary>An <see cref="sbyte"/> column under a declared notation.</summary>
	public static Column SByte(int ordinal, NumFormat format) => new(ordinal, Door.SByte, 0, format);
	/// <summary>An <see cref="sbyte"/> column under the invariant notation, judged against <paramref name="constraints"/>.</summary>
	/// <exception cref="ArgumentNullException"><paramref name="constraints"/> is null.</exception>
	public static Column SByte(int ordinal, Constraints<sbyte> constraints) => SByte(ordinal, NumFormat.Invariant, constraints);
	/// <summary>An <see cref="sbyte"/> column under a declared notation, judged against <paramref name="constraints"/>.</summary>
	/// <exception cref="ArgumentNullException"><paramref name="constraints"/> is null.</exception>
	public static Column SByte(int ordinal, NumFormat format, Constraints<sbyte> constraints) => Checked(SByte(ordinal, format), constraints);

	/// <summary>A <see cref="short"/> column under the invariant notation.</summary>
	public static Column Int16(int ordinal) => Int16(ordinal, NumFormat.Invariant);
	/// <summary>A <see cref="short"/> column under a declared notation.</summary>
	public static Column Int16(int ordinal, NumFormat format) => new(ordinal, Door.Int16, 0, format);
	/// <summary>A <see cref="short"/> column under the invariant notation, judged against <paramref name="constraints"/>.</summary>
	/// <exception cref="ArgumentNullException"><paramref name="constraints"/> is null.</exception>
	public static Column Int16(int ordinal, Constraints<short> constraints) => Int16(ordinal, NumFormat.Invariant, constraints);
	/// <summary>A <see cref="short"/> column under a declared notation, judged against <paramref name="constraints"/>.</summary>
	/// <exception cref="ArgumentNullException"><paramref name="constraints"/> is null.</exception>
	public static Column Int16(int ordinal, NumFormat format, Constraints<short> constraints) => Checked(Int16(ordinal, format), constraints);

	/// <summary>An <see cref="int"/> column under the invariant notation.</summary>
	public static Column Int32(int ordinal) => Int32(ordinal, NumFormat.Invariant);
	/// <summary>An <see cref="int"/> column under a declared notation.</summary>
	public static Column Int32(int ordinal, NumFormat format) => new(ordinal, Door.Int32, 0, format);
	/// <summary>An <see cref="int"/> column under the invariant notation, judged against <paramref name="constraints"/>.</summary>
	/// <exception cref="ArgumentNullException"><paramref name="constraints"/> is null.</exception>
	public static Column Int32(int ordinal, Constraints<int> constraints) => Int32(ordinal, NumFormat.Invariant, constraints);
	/// <summary>An <see cref="int"/> column under a declared notation, judged against <paramref name="constraints"/>.</summary>
	/// <exception cref="ArgumentNullException"><paramref name="constraints"/> is null.</exception>
	public static Column Int32(int ordinal, NumFormat format, Constraints<int> constraints) => Checked(Int32(ordinal, format), constraints);

	/// <summary>A <see cref="long"/> column under the invariant notation.</summary>
	public static Column Int64(int ordinal) => Int64(ordinal, NumFormat.Invariant);
	/// <summary>A <see cref="long"/> column under a declared notation.</summary>
	public static Column Int64(int ordinal, NumFormat format) => new(ordinal, Door.Int64, 0, format);
	/// <summary>A <see cref="long"/> column under the invariant notation, judged against <paramref name="constraints"/>.</summary>
	/// <exception cref="ArgumentNullException"><paramref name="constraints"/> is null.</exception>
	public static Column Int64(int ordinal, Constraints<long> constraints) => Int64(ordinal, NumFormat.Invariant, constraints);
	/// <summary>A <see cref="long"/> column under a declared notation, judged against <paramref name="constraints"/>.</summary>
	/// <exception cref="ArgumentNullException"><paramref name="constraints"/> is null.</exception>
	public static Column Int64(int ordinal, NumFormat format, Constraints<long> constraints) => Checked(Int64(ordinal, format), constraints);

	/// <summary>A <see cref="byte"/> column under the invariant notation.</summary>
	public static Column Byte(int ordinal) => Byte(ordinal, NumFormat.Invariant);
	/// <summary>A <see cref="byte"/> column under a declared notation.</summary>
	public static Column Byte(int ordinal, NumFormat format) => new(ordinal, Door.Byte, 0, format);
	/// <summary>A <see cref="byte"/> column under the invariant notation, judged against <paramref name="constraints"/>.</summary>
	/// <exception cref="ArgumentNullException"><paramref name="constraints"/> is null.</exception>
	public static Column Byte(int ordinal, Constraints<byte> constraints) => Byte(ordinal, NumFormat.Invariant, constraints);
	/// <summary>A <see cref="byte"/> column under a declared notation, judged against <paramref name="constraints"/>.</summary>
	/// <exception cref="ArgumentNullException"><paramref name="constraints"/> is null.</exception>
	public static Column Byte(int ordinal, NumFormat format, Constraints<byte> constraints) => Checked(Byte(ordinal, format), constraints);

	/// <summary>A <see cref="ushort"/> column under the invariant notation.</summary>
	public static Column UInt16(int ordinal) => UInt16(ordinal, NumFormat.Invariant);
	/// <summary>A <see cref="ushort"/> column under a declared notation.</summary>
	public static Column UInt16(int ordinal, NumFormat format) => new(ordinal, Door.UInt16, 0, format);
	/// <summary>A <see cref="ushort"/> column under the invariant notation, judged against <paramref name="constraints"/>.</summary>
	/// <exception cref="ArgumentNullException"><paramref name="constraints"/> is null.</exception>
	public static Column UInt16(int ordinal, Constraints<ushort> constraints) => UInt16(ordinal, NumFormat.Invariant, constraints);
	/// <summary>A <see cref="ushort"/> column under a declared notation, judged against <paramref name="constraints"/>.</summary>
	/// <exception cref="ArgumentNullException"><paramref name="constraints"/> is null.</exception>
	public static Column UInt16(int ordinal, NumFormat format, Constraints<ushort> constraints) => Checked(UInt16(ordinal, format), constraints);

	/// <summary>A <see cref="uint"/> column under the invariant notation.</summary>
	public static Column UInt32(int ordinal) => UInt32(ordinal, NumFormat.Invariant);
	/// <summary>A <see cref="uint"/> column under a declared notation.</summary>
	public static Column UInt32(int ordinal, NumFormat format) => new(ordinal, Door.UInt32, 0, format);
	/// <summary>A <see cref="uint"/> column under the invariant notation, judged against <paramref name="constraints"/>.</summary>
	/// <exception cref="ArgumentNullException"><paramref name="constraints"/> is null.</exception>
	public static Column UInt32(int ordinal, Constraints<uint> constraints) => UInt32(ordinal, NumFormat.Invariant, constraints);
	/// <summary>A <see cref="uint"/> column under a declared notation, judged against <paramref name="constraints"/>.</summary>
	/// <exception cref="ArgumentNullException"><paramref name="constraints"/> is null.</exception>
	public static Column UInt32(int ordinal, NumFormat format, Constraints<uint> constraints) => Checked(UInt32(ordinal, format), constraints);

	/// <summary>A <see cref="ulong"/> column under the invariant notation.</summary>
	public static Column UInt64(int ordinal) => UInt64(ordinal, NumFormat.Invariant);
	/// <summary>A <see cref="ulong"/> column under a declared notation.</summary>
	public static Column UInt64(int ordinal, NumFormat format) => new(ordinal, Door.UInt64, 0, format);
	/// <summary>A <see cref="ulong"/> column under the invariant notation, judged against <paramref name="constraints"/>.</summary>
	/// <exception cref="ArgumentNullException"><paramref name="constraints"/> is null.</exception>
	public static Column UInt64(int ordinal, Constraints<ulong> constraints) => UInt64(ordinal, NumFormat.Invariant, constraints);
	/// <summary>A <see cref="ulong"/> column under a declared notation, judged against <paramref name="constraints"/>.</summary>
	/// <exception cref="ArgumentNullException"><paramref name="constraints"/> is null.</exception>
	public static Column UInt64(int ordinal, NumFormat format, Constraints<ulong> constraints) => Checked(UInt64(ordinal, format), constraints);

	/// <summary>A <see cref="float"/> column under the invariant notation.</summary>
	public static Column Single(int ordinal) => Single(ordinal, NumFormat.Invariant);
	/// <summary>A <see cref="float"/> column under a declared notation.</summary>
	public static Column Single(int ordinal, NumFormat format) => new(ordinal, Door.Single, 0, format);
	/// <summary>A <see cref="float"/> column under the invariant notation, judged against <paramref name="constraints"/>.</summary>
	/// <exception cref="ArgumentNullException"><paramref name="constraints"/> is null.</exception>
	public static Column Single(int ordinal, Constraints<float> constraints) => Single(ordinal, NumFormat.Invariant, constraints);
	/// <summary>A <see cref="float"/> column under a declared notation, judged against <paramref name="constraints"/>.</summary>
	/// <exception cref="ArgumentNullException"><paramref name="constraints"/> is null.</exception>
	public static Column Single(int ordinal, NumFormat format, Constraints<float> constraints) => Checked(Single(ordinal, format), constraints);

	/// <summary>A <see cref="double"/> column under the invariant notation.</summary>
	public static Column Double(int ordinal) => Double(ordinal, NumFormat.Invariant);
	/// <summary>A <see cref="double"/> column under a declared notation.</summary>
	public static Column Double(int ordinal, NumFormat format) => new(ordinal, Door.Double, 0, format);
	/// <summary>A <see cref="double"/> column under the invariant notation, judged against <paramref name="constraints"/>.</summary>
	/// <exception cref="ArgumentNullException"><paramref name="constraints"/> is null.</exception>
	public static Column Double(int ordinal, Constraints<double> constraints) => Double(ordinal, NumFormat.Invariant, constraints);
	/// <summary>A <see cref="double"/> column under a declared notation, judged against <paramref name="constraints"/>.</summary>
	/// <exception cref="ArgumentNullException"><paramref name="constraints"/> is null.</exception>
	public static Column Double(int ordinal, NumFormat format, Constraints<double> constraints) => Checked(Double(ordinal, format), constraints);

	/// <summary>An exact <see cref="decimal"/> column under the invariant notation.</summary>
	public static Column Decimal(int ordinal) => Decimal(ordinal, NumFormat.Invariant);
	/// <summary>An exact <see cref="decimal"/> column under a declared notation.</summary>
	public static Column Decimal(int ordinal, NumFormat format) => new(ordinal, Door.Decimal, 0, format);
	/// <summary>An exact <see cref="decimal"/> column under the invariant notation, judged against <paramref name="constraints"/>.</summary>
	/// <exception cref="ArgumentNullException"><paramref name="constraints"/> is null.</exception>
	public static Column Decimal(int ordinal, Constraints<decimal> constraints) => Decimal(ordinal, NumFormat.Invariant, constraints);
	/// <summary>An exact <see cref="decimal"/> column under a declared notation, judged against <paramref name="constraints"/>.</summary>
	/// <exception cref="ArgumentNullException"><paramref name="constraints"/> is null.</exception>
	public static Column Decimal(int ordinal, NumFormat format, Constraints<decimal> constraints) => Checked(Decimal(ordinal, format), constraints);

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

	/// <summary>
	/// A text column judged against <paramref name="constraints"/>: the lengths and <c>enum</c>, after
	/// the <c>trim</c> they declare. An <c>enum</c> match reads as the entry as declared.
	/// </summary>
	/// <exception cref="ArgumentNullException"><paramref name="constraints"/> is null.</exception>
	public static Column Text(int ordinal, Constraints<string> constraints) => Checked(Text(ordinal), constraints);

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

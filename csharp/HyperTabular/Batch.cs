using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Text;
using HyperCast;

namespace HyperTabular;

/// <summary>
/// The rows of one read, column-major: for each column of the plan, the values its door made
/// and a verdict beside each. One type serves delimited text and workbooks alike; the two
/// differ only in where a cell's text lies, which the batch knows and its accessors hide.
/// </summary>
/// <remarks>
/// A batch is a view of its reader's own buffers, nothing copied to make it: a column of
/// integers is the array the core wrote, and a text cell is the input itself (or the
/// workbook's shared strings) wherever its bytes could stay where they were. It is valid
/// until the reader is asked for the next batch — the reader hands out the same instance
/// each time — and every span it gives out is valid as long. Not thread-safe.
/// </remarks>
[SkipLocalsInit]
public sealed unsafe class Batch
{
	readonly Columns _columns;
	readonly bool _workbook;
	int _rows;
	Native.RawSpan[] _cells = [];
	int _perRow;
	/// <summary>Where an unflagged span's bytes are: the input, or the shared strings.</summary>
	byte* _base;
	/// <summary>Where a flagged span's bytes are.</summary>
	byte[] _arena = [];
	byte[] _scratch = [];

	internal Batch(Columns columns, bool workbook)
	{
		_columns = columns;
		_workbook = workbook;
	}

	/// <summary>Points the batch at what the core just wrote.</summary>
	internal Batch Fill(int rows, Native.RawSpan[] cells, int perRow, byte* @base, byte[] arena)
	{
		_rows = rows;
		_cells = cells;
		_perRow = perRow;
		_base = @base;
		_arena = arena;
		return this;
	}

	/// <summary>Ends the batch: the reader is about to reuse what it points into.</summary>
	internal void Clear() => _rows = 0;

	/// <summary>How many rows the batch holds. Never zero while the batch is current.</summary>
	public int Rows => _rows;

	/// <summary>The plan the batch was read through: column <c>i</c> of the batch is <c>Columns[i]</c>.</summary>
	public IReadOnlyList<Column> Columns => _columns.Plan;

	/// <summary>
	/// Where row <paramref name="row"/> came from: for delimited text the 1-based line its
	/// record starts on, for a sheet its 1-based row number.
	/// </summary>
	public int Line(int row)
	{
		CheckRow(row);
		var entry = _cells[row * _perRow + _perRow - 1];
		return (int)(_workbook ? entry.Offset : entry.Len);
	}

	void CheckRow(int row) =>
		ArgumentOutOfRangeException.ThrowIfGreaterThanOrEqual((uint)row, (uint)_rows, nameof(row));

	/// <summary>
	/// A column's verdicts, one per row. A verdict that is not ok is the cell's fault, its
	/// span in the cell's text (<see cref="Raw"/>).
	/// </summary>
	public ReadOnlySpan<CellVerdict> Verdicts(int column) => _columns.Verdicts[column].AsSpan(0, _rows);

	/// <summary>
	/// A whole column as the core wrote it, one value per row — for the doors whose value is
	/// a primitive: <see cref="Door.Boolean"/> as <see cref="bool"/>, the integer and
	/// floating-point doors as their own type. The value of a row whose verdict is not ok is
	/// zero. Columns of other doors are read a cell at a time, with <see cref="Get{T}"/>.
	/// </summary>
	/// <exception cref="InvalidOperationException"><typeparamref name="T"/> is not the type the column's door writes.</exception>
	public ReadOnlySpan<T> Values<T>(int column) where T : unmanaged
	{
		var door = _columns.Plan[column].Door;
		var matches =
			typeof(T) == typeof(bool) ? door == Door.Boolean
			: typeof(T) == typeof(sbyte) ? door == Door.SByte
			: typeof(T) == typeof(short) ? door == Door.Int16
			: typeof(T) == typeof(int) ? door == Door.Int32
			: typeof(T) == typeof(long) ? door == Door.Int64
			: typeof(T) == typeof(byte) ? door == Door.Byte
			: typeof(T) == typeof(ushort) ? door == Door.UInt16
			: typeof(T) == typeof(uint) ? door == Door.UInt32
			: typeof(T) == typeof(ulong) ? door == Door.UInt64
			: typeof(T) == typeof(float) ? door == Door.Single
			: typeof(T) == typeof(double) && door == Door.Double;
		if (!matches)
			throw new InvalidOperationException(
				$"Column {column} is cast through {door}; it has no span of {typeof(T).Name}.");
		return MemoryMarshal.Cast<byte, T>(_columns.Values[column].AsSpan(0, _rows * sizeof(T)));
	}

	/// <summary>
	/// The cell at (<paramref name="column"/>, <paramref name="row"/>) as HyperCast judged it:
	/// the value, or the fault. <typeparamref name="T"/> is the type the column's door makes:
	/// <list type="table">
	/// <item><term><see cref="bool"/></term><description><see cref="Door.Boolean"/></description></item>
	/// <item><term><see cref="sbyte"/> … <see cref="ulong"/>, <see cref="float"/>, <see cref="double"/></term><description>the door of the same name</description></item>
	/// <item><term><see cref="decimal"/></term><description><see cref="Door.Decimal"/>, exact</description></item>
	/// <item><term><see cref="Guid"/></term><description><see cref="Door.Uuid"/></description></item>
	/// <item><term><see cref="DateTimeOffset"/></term><description><see cref="Door.Timestamp"/>, <see cref="Door.Unix"/>, <see cref="Door.ExcelSerial"/></description></item>
	/// <item><term><see cref="DateOnly"/></term><description><see cref="Door.Date"/>, <see cref="Door.DateOrdered"/></description></item>
	/// <item><term><see cref="System.DateTime"/></term><description><see cref="Door.DateTime"/>, of <see cref="DateTimeKind.Unspecified"/></description></item>
	/// <item><term><see cref="TimeOnly"/></term><description><see cref="Door.Time"/></description></item>
	/// <item><term><see cref="TimeSpan"/></term><description><see cref="Door.Duration"/></description></item>
	/// </list>
	/// The temporal types hold 100 ns ticks, so sub-tick nanoseconds truncate. A
	/// <see cref="Door.Text"/> column is read with <see cref="TryGetText"/>.
	/// </summary>
	/// <exception cref="InvalidOperationException">The column's door does not make a <typeparamref name="T"/>.</exception>
	/// <exception cref="ArgumentOutOfRangeException">The row is not in the batch.</exception>
	public Verdict<T> Get<T>(int column, int row) where T : struct
	{
		CheckRow(row);
		var door = _columns.Plan[column].Door;
		if (!Reads<T>(door))
			throw new InvalidOperationException($"Column {column} is cast through {door}, which does not make a {typeof(T).Name}.");
		var verdict = _columns.Verdicts[column][row];
		if (!verdict.IsOk)
			return new Verdict<T>(verdict.ToFault());
		var values = _columns.Values[column];
		if (typeof(T) == typeof(bool))
			return As<bool, T>(values[row] != 0);
		if (typeof(T) == typeof(decimal))
		{
			var value = Read<Native.RawDecimal>(values, row);
			return As<decimal, T>(new decimal((int)value.Lo, (int)(value.Lo >> 32), (int)value.Hi, value.Negative != 0, value.Scale));
		}
		if (typeof(T) == typeof(Guid))
			return As<Guid, T>(new Guid(values.AsSpan(row * 16, 16), bigEndian: true));
		if (typeof(T) == typeof(DateTimeOffset))
		{
			var value = Read<Native.RawTimestamp>(values, row);
			return As<DateTimeOffset, T>(new DateTimeOffset(
				System.DateTime.UnixEpoch.Ticks + value.Seconds * TimeSpan.TicksPerSecond + value.Nanos / 100, TimeSpan.Zero));
		}
		if (typeof(T) == typeof(DateOnly))
		{
			var value = Read<Native.RawDate>(values, row);
			return As<DateOnly, T>(new DateOnly(value.Year, value.Month, value.Day));
		}
		if (typeof(T) == typeof(System.DateTime))
		{
			var value = Read<Native.RawCivil>(values, row);
			return As<System.DateTime, T>(new System.DateTime(value.Year, value.Month, value.Day, 0, 0, 0, DateTimeKind.Unspecified)
				.AddTicks((long)(value.NanosOfDay / 100)));
		}
		if (typeof(T) == typeof(TimeOnly))
			return As<TimeOnly, T>(new TimeOnly((long)(Read<ulong>(values, row) / 100)));
		if (typeof(T) == typeof(TimeSpan))
		{
			var value = Read<Native.RawDuration>(values, row);
			return As<TimeSpan, T>(new TimeSpan(value.Seconds * TimeSpan.TicksPerSecond + value.Nanos / 100));
		}
		// The primitives: the value is the bytes the core wrote.
		return new Verdict<T>(new Success<T>(Unsafe.ReadUnaligned<T>(ref values[row * Unsafe.SizeOf<T>()])));
	}

	/// <summary>Whether a door makes a <typeparamref name="T"/>.</summary>
	static bool Reads<T>(Door door) =>
		typeof(T) == typeof(bool) ? door == Door.Boolean
		: typeof(T) == typeof(sbyte) ? door == Door.SByte
		: typeof(T) == typeof(short) ? door == Door.Int16
		: typeof(T) == typeof(int) ? door == Door.Int32
		: typeof(T) == typeof(long) ? door == Door.Int64
		: typeof(T) == typeof(byte) ? door == Door.Byte
		: typeof(T) == typeof(ushort) ? door == Door.UInt16
		: typeof(T) == typeof(uint) ? door == Door.UInt32
		: typeof(T) == typeof(ulong) ? door == Door.UInt64
		: typeof(T) == typeof(float) ? door == Door.Single
		: typeof(T) == typeof(double) ? door == Door.Double
		: typeof(T) == typeof(decimal) ? door == Door.Decimal
		: typeof(T) == typeof(Guid) ? door == Door.Uuid
		: typeof(T) == typeof(DateTimeOffset) ? door is Door.Timestamp or Door.Unix or Door.ExcelSerial
		: typeof(T) == typeof(DateOnly) ? door is Door.Date or Door.DateOrdered
		: typeof(T) == typeof(System.DateTime) ? door == Door.DateTime
		: typeof(T) == typeof(TimeOnly) ? door == Door.Time
		: typeof(T) == typeof(TimeSpan) && door == Door.Duration;

	static TValue Read<TValue>(byte[] values, int row) where TValue : unmanaged =>
		Unsafe.ReadUnaligned<TValue>(ref values[row * sizeof(TValue)]);

	// Re-types a value as the caller's T once typeof(T) has proven the two identical: an
	// identity reinterpret, never a conversion, and no box.
	static Verdict<T> As<TValue, T>(TValue value) where T : struct =>
		new(new Success<T>(Unsafe.As<TValue, T>(ref value)));

	/// <summary>The bytes a span names: the arena's when the span is flagged, the base's when not.</summary>
	ReadOnlySpan<byte> Bytes(Native.RawSpan span) =>
		span.Flagged
			? _arena.AsSpan((int)span.Offset, span.Length)
			: new ReadOnlySpan<byte>(_base + span.Offset, span.Length);

	/// <summary>
	/// The cell of a <see cref="Door.Text"/> column: its bytes, untrimmed — quotes resolved
	/// for delimited text, and a typed workbook cell said the canonical way (<c>42</c>,
	/// <c>true</c>, <c>2024-01-31T10:30:00</c>, <c>PT1H30M</c>). <see langword="false"/> for a
	/// cell with no bytes at all, which is the one way text fails.
	/// </summary>
	/// <exception cref="InvalidOperationException">The column is not read through <see cref="Door.Text"/>.</exception>
	public bool TryGetText(int column, int row, out ReadOnlySpan<byte> utf8)
	{
		CheckRow(row);
		var door = _columns.Plan[column].Door;
		if (door != Door.Text)
			throw new InvalidOperationException($"Column {column} is cast through {door}, not {Door.Text}.");
		if (!_columns.Verdicts[column][row].IsOk)
		{
			utf8 = default;
			return false;
		}
		utf8 = Bytes(Read<Native.RawSpan>(_columns.Values[column], row));
		return true;
	}

	/// <summary>The cell of a <see cref="Door.Text"/> column as a string, or <see langword="null"/> for an empty cell.</summary>
	public string? GetString(int column, int row) =>
		TryGetText(column, row, out var utf8) ? Encoding.UTF8.GetString(utf8) : null;

	/// <summary>
	/// The text the cell at (<paramref name="column"/>, <paramref name="row"/>) was cast from,
	/// whatever its door and whatever its verdict — what a fault's span indexes, and what to
	/// show for a value that did not cast. For a workbook this is a text cell's own text, and
	/// what a typed cell was said as when it failed its door (or went through the text door);
	/// a typed cell that cast has none, and neither has an empty cell. Valid until the next
	/// call to <see cref="Raw"/>, or the next read.
	/// </summary>
	public ReadOnlySpan<byte> Raw(int column, int row)
	{
		CheckRow(row);
		ArgumentOutOfRangeException.ThrowIfGreaterThanOrEqual((uint)column, (uint)_columns.Plan.Length, nameof(column));
		var at = _workbook ? column : _columns.Plan[column].Ordinal;
		var cell = _cells[row * _perRow + at];
		if (_workbook || !cell.Flagged)
			return _workbook ? Bytes(cell) : new ReadOnlySpan<byte>(_base + cell.Offset, cell.Length);
		// A quoted cell with a doubled quote in it: unescaped, as the core cast it.
		var quoted = new ReadOnlySpan<byte>(_base + cell.Offset, cell.Length);
		if (_scratch.Length < quoted.Length)
			_scratch = new byte[Math.Max(quoted.Length, 256)];
		fixed (byte* source = quoted)
		fixed (byte* target = _scratch)
		{
			var written = Native.hypertabular_delimited_unescape(source, (nuint)quoted.Length, target, (nuint)_scratch.Length);
			return _scratch.AsSpan(0, (int)written);
		}
	}
}

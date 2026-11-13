using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Text;
using System.Text.Unicode;
using HyperCast;
using HyperCast.Interop;

namespace HyperTabular;

/// <summary>Where a cell's verdict came from: the door, or something that spared it being asked. The numbers are the core's codes.</summary>
public enum Origin : ushort
{
	/// <summary>A door's verdict on the cell's text: every cell that cast, and every refusal.</summary>
	Cell = 0,
	/// <summary>A declared token: empty, and no door asked.</summary>
	Token = 1,
	/// <summary>A cell its row does not have, under <see cref="ShortRows.Pad"/> or past a sheet row's last cell: empty, and no door asked.</summary>
	Padded = 2,
	/// <summary>A workbook formula with no cached value: empty, and no door asked.</summary>
	Uncached = 3
}

/// <summary>One entry of <see cref="Batch.Faults"/>: a cell that went wrong, or a row longer than the width.</summary>
/// <param name="Row">The row, from zero, in the batch.</param>
/// <param name="Line">Delimited text: the record's first physical line. A sheet: its absolute row. From one.</param>
/// <param name="PlanColumn">The column of the plan; <see langword="null"/> for an entry about the row as a whole.</param>
/// <param name="SourceColumn">The source column, from zero; a sheet's own, so <c>A</c> is 0 wherever a range starts. <see langword="null"/> for an entry about the row as a whole.</param>
/// <param name="Reason">Why the verdict is what it is: malformed, out of range, or empty for a formula with no cached value. <see langword="null"/> for an entry about the row as a whole, and for a violation, which <see cref="Keyword"/> names.</param>
/// <param name="DetailCode">HyperCast's raw detail code, 0 for none; one this binding does not name is kept here.</param>
/// <param name="Origin">Where the verdict came from. An entry about the row as a whole says <see cref="Origin.Cell"/>.</param>
/// <param name="Offset">Byte offset of the offending span in the cell's decoded, unescaped text; 0 where there is none.</param>
/// <param name="Length">Byte length of the offending span; 0 where there is none.</param>
/// <param name="Expected">A long row's width by its header or first record; otherwise 0.</param>
/// <param name="Found">How many cells a long row had; otherwise 0.</param>
public readonly record struct Problem(
	int Row,
	int Line,
	int? PlanColumn,
	int? SourceColumn,
	CastFailure? Reason,
	uint DetailCode,
	Origin Origin,
	int Offset,
	int Length,
	int Expected,
	int Found)
{
	/// <summary>What failed, named: <see cref="FaultDetail.None"/>, one of HyperCast's members, or <see cref="FaultDetail.Unknown"/>.</summary>
	public FaultDetail Detail => Abi.DetailFrom(DetailCode);

	/// <summary>
	/// For a cell that cast and broke a constraint its column declares, the keyword it broke, its
	/// span in <see cref="Offset"/> and <see cref="Length"/>; <see langword="null"/> otherwise.
	/// </summary>
	public Keyword? Keyword { get; init; }
}

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
	/// <summary>What a plan's ordinal is short of the sheet's own column: a range's start.</summary>
	readonly int _firstColumn;
	int _rows;
	/// <summary>Moves whenever the batch is over, so that a list asked of it never answers for the next.</summary>
	ulong _generation;
	/// <summary>The width a long row is held to, as the read last knew it.</summary>
	uint _expected;
	Native.RawSpan[] _cells = [];
	int _perRow;
	/// <summary>Where an unflagged span's bytes are: the input, or the shared strings.</summary>
	byte* _base;
	/// <summary>The reader or sheet, which holds what <see cref="_base"/> points into: a pointer keeps nothing alive, and a batch can outlive every other reference to them.</summary>
	readonly object _owner;
	/// <summary>Where a flagged span's bytes are.</summary>
	byte[] _arena = [];
	byte[] _scratch = [];
	/// <summary>Where <see cref="GetChars"/> decodes to: made on first use, filled from the front, and started over at each batch.</summary>
	char[] _chars = [];
	int _charsUsed;
	/// <summary>Chars decoded for this batch, in every arena it has had: what the next arena is sized by.</summary>
	int _charsDecoded;

	/// <summary>For the tests: the char arena starts with room for one element and grows only to what a cell needs.</summary>
	[ThreadStatic]
	internal static bool Stingy;

	/// <summary>For the tests: how many times a char arena was made or grown on this thread.</summary>
	[ThreadStatic]
	internal static int CharsGrown;

	internal Batch(object owner, Columns columns, bool workbook, int firstColumn = 0)
	{
		_owner = owner;
		_columns = columns;
		_workbook = workbook;
		_firstColumn = firstColumn;
	}

	/// <summary>Points the batch at what the core just wrote.</summary>
	internal Batch Fill(int rows, Native.RawSpan[] cells, int perRow, byte* @base, byte[] arena, uint expected = 0)
	{
		_generation++;
		_rows = rows;
		_expected = expected;
		_cells = cells;
		_perRow = perRow;
		_base = @base;
		_arena = arena;
		_charsUsed = 0;
		_charsDecoded = 0;
		return this;
	}

	/// <summary>Ends the batch: the reader is about to reuse what it points into.</summary>
	internal void Clear()
	{
		_generation++;
		_rows = 0;
	}

	/// <summary>How many rows the batch holds. Never zero while the batch is current.</summary>
	public int Rows => _rows;

	/// <summary>Row <paramref name="index"/> of the batch, as a view (<see cref="HyperTabular.Row"/>).</summary>
	/// <exception cref="ArgumentOutOfRangeException">The row is not in the batch.</exception>
	public Row Row(int index)
	{
		CheckRow(index);
		return new(this, index);
	}

	/// <summary>
	/// The batch's rows, in order, for <c>foreach (var row in batch)</c>: each a view of the
	/// batch (<see cref="HyperTabular.Row"/>), made without allocating, and valid as long as the batch is.
	/// </summary>
	public RowEnumerator GetEnumerator() => new(this);

	/// <summary>A loop over a batch's rows.</summary>
	public ref struct RowEnumerator
	{
		readonly Batch _batch;
		readonly ulong _generation;
		int _index;

		internal RowEnumerator(Batch batch)
		{
			_batch = batch;
			_generation = batch._generation;
			_index = -1;
		}

		/// <summary>The row the loop is on.</summary>
		/// <exception cref="InvalidOperationException">The reader has read on since the loop began.</exception>
		public readonly Row Current
		{
			get
			{
				_batch.StillCurrent(_generation);
				return new(_batch, _index);
			}
		}

		/// <summary>Moves to the next row.</summary>
		/// <exception cref="InvalidOperationException">The reader has read on since the loop began.</exception>
		public bool MoveNext()
		{
			_batch.StillCurrent(_generation);
			return ++_index < _batch._rows;
		}
	}

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
	/// A column's verdicts, one per row. A verdict that is not ok is the cell's fault, or its
	/// violation (<see cref="CellVerdict.IsViolation"/>), its span in the cell's text (<see cref="Raw"/>).
	/// </summary>
	public ReadOnlySpan<CellVerdict> Verdicts(int column) => _columns.Verdicts[column].AsSpan(0, _rows);

	/// <summary>
	/// The fault of the cell at (<paramref name="column"/>, <paramref name="row"/>), with what failed
	/// on its <see cref="HyperCast.Fault.Detail"/>; <see langword="null"/> for a cell that cast,
	/// whether or not it broke a constraint. The one way to a text column's:
	/// <see cref="CellVerdict.ToFault"/> has no room for the detail.
	/// </summary>
	/// <exception cref="ArgumentOutOfRangeException">The row is not in the batch.</exception>
	public Fault? Fault(int column, int row)
	{
		CheckRow(row);
		var verdict = _columns.Verdicts[column][row];
		return verdict.IsOk || verdict.IsViolation ? null : Detailed(verdict, column, row);
	}

	/// <summary>A verdict that is not ok as HyperCast's fault, with the note's detail on it.</summary>
	Fault Detailed(CellVerdict verdict, int column, int row) =>
		verdict.ToFault() with { DetailCode = Note(column, row).Detail };

	/// <summary>The cell's note: all zeros with notes off.</summary>
	internal Native.RawCellNote Note(int column, int row) =>
		_columns.Notes is { } notes ? notes[column][row] : default;

	/// <summary>
	/// Where the verdict of the cell at (<paramref name="column"/>, <paramref name="row"/>) came from.
	/// With notes off (<see cref="Dialect.Notes"/>, <see cref="SheetOptions.Notes"/>) every cell says
	/// <see cref="Origin.Cell"/>: a token's or a padded cell's verdict is still empty, and nothing was
	/// kept of why.
	/// </summary>
	/// <exception cref="ArgumentOutOfRangeException">The row is not in the batch.</exception>
	public Origin Origin(int column, int row)
	{
		CheckRow(row);
		return (Origin)Note(column, row).Origin;
	}

	/// <summary>
	/// What went wrong in this batch, in order by row and then by column, a row's own entry first:
	/// cells at <see cref="CastFailure.Malformed"/> or <see cref="CastFailure.OutOfRange"/>, formulas
	/// with no cached value (<see cref="Origin.Uncached"/>), cells that broke a constraint their
	/// column declares (<see cref="Problem.Keyword"/>), and rows longer than the width under
	/// <see cref="LongRows.Report"/>. Blanks, tokens, padding and <see cref="LongRows.Truncate"/> are
	/// not listed. A row the empty-row filter dropped cannot be named. With notes off a refused cell has
	/// no detail, and an uncached one is not listed.
	/// </summary>
	/// <returns>
	/// The list, which belongs to this batch: going on with it after the next read throws
	/// <see cref="InvalidOperationException"/>, and never answers for the next batch.
	/// </returns>
	public IEnumerable<Problem> Faults() => ListedFor(_generation);

	IEnumerable<Problem> ListedFor(ulong generation)
	{
		const int Held = 64;
		var held = new Native.RawProblem[Held];
		ulong cursor = 0;
		while (true)
		{
			StillCurrent(generation);
			var written = NextFaults(ref cursor, held);
			for (var index = 0; index < written; index++)
			{
				yield return ProblemOf(held[index]);
				StillCurrent(generation);
			}
			if (written < Held)
				yield break;
		}
	}

	/// <summary>Moves whenever the batch is over: what a loop over a reader's rows holds to see that nothing read on under it.</summary>
	internal ulong Generation => _generation;

	internal void StillCurrent(ulong generation)
	{
		if (generation != _generation)
			throw new InvalidOperationException("The batch this walks is over: the reader has read on.");
	}

	int NextFaults(ref ulong cursor, Native.RawProblem[] held)
	{
		var report = _columns.RowWidths is not null && !_workbook;
		ulong at = cursor;
		nuint written;
		fixed (Native.RawProblem* into = held)
		fixed (uint* widths = report ? _columns.RowWidths : null)
		{
			var code = Native.hypertabular_faults(
				HyperTabular.Columns.Address(_columns.Buffers), (nuint)_columns.Plan.Length, (nuint)_rows, widths, _expected,
				report ? (uint)LongRows.Report : (uint)LongRows.Reject, &at, into, (nuint)held.Length, &written);
			if (code != Native.Ok)
				throw new InvalidOperationException(Workbook.ContractViolation);
		}
		cursor = at;
		return (int)written;
	}

	Problem ProblemOf(in Native.RawProblem raw)
	{
		var row = (int)raw.Row;
		var line = Line(row);
		if (raw.PlanColumn == Native.RawProblem.RowLevel)
			return new(row, line, null, null, null, 0, HyperTabular.Origin.Cell, 0, 0, (int)raw.Expected, (int)raw.Found);
		var column = (int)raw.PlanColumn;
		return new(row, line, column, _columns.Plan[column].Ordinal + _firstColumn, Abi.ReasonFrom(raw.Reason), raw.Detail,
			(Origin)raw.Origin, (int)raw.Offset, (int)raw.Len, (int)raw.Expected, (int)raw.Found)
		{
			Keyword = HyperCast.Keyword.FromCode(raw.Keyword),
		};
	}

	/// <summary>
	/// A whole column as the core wrote it, one value per row, nothing copied:
	/// <list type="table">
	/// <item><term><see cref="bool"/></term><description><see cref="Door.Boolean"/></description></item>
	/// <item><term><see cref="sbyte"/> … <see cref="ulong"/>, <see cref="float"/>, <see cref="double"/></term><description>the door of the same name</description></item>
	/// <item><term><see cref="ulong"/></term><description><see cref="Door.Time"/>, nanoseconds since midnight</description></item>
	/// <item><term><see cref="RawDecimal"/></term><description><see cref="Door.Decimal"/></description></item>
	/// <item><term><see cref="RawTimestamp"/></term><description><see cref="Door.Timestamp"/>, <see cref="Door.Unix"/>, <see cref="Door.ExcelSerial"/></description></item>
	/// <item><term><see cref="RawDate"/></term><description><see cref="Door.Date"/>, <see cref="Door.DateOrdered"/></description></item>
	/// <item><term><see cref="RawCivil"/></term><description><see cref="Door.DateTime"/></description></item>
	/// <item><term><see cref="RawDuration"/></term><description><see cref="Door.Duration"/></description></item>
	/// </list>
	/// The <c>Raw</c> types are HyperCast's own (<see cref="HyperCast.Interop"/>), laid out as
	/// the core writes them, and each converts itself to the type <see cref="Get{T}"/> makes. The
	/// value of a row whose verdict is a fault is zero; a violation's is the value as cast. A <see cref="Door.Uuid"/> column has no
	/// span — its 16 bytes are in RFC 9562 order, which is not <see cref="Guid"/>'s layout — and
	/// is read a cell at a time with <see cref="Get{T}"/>, as is a <see cref="Door.Text"/> column
	/// with <see cref="TryGetText"/>.
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
			: typeof(T) == typeof(ulong) ? door is Door.UInt64 or Door.Time
			: typeof(T) == typeof(float) ? door == Door.Single
			: typeof(T) == typeof(double) ? door == Door.Double
			: typeof(T) == typeof(RawDecimal) ? door == Door.Decimal
			: typeof(T) == typeof(RawTimestamp) ? door is Door.Timestamp or Door.Unix or Door.ExcelSerial
			: typeof(T) == typeof(RawDate) ? door is Door.Date or Door.DateOrdered
			: typeof(T) == typeof(RawCivil) ? door == Door.DateTime
			: typeof(T) == typeof(RawDuration) && door == Door.Duration;
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
	/// <exception cref="InvalidOperationException">
	/// The column's door does not make a <typeparamref name="T"/>, or the column declares constraints,
	/// whose cells <see cref="GetChecked{T}"/> reads.
	/// </exception>
	/// <exception cref="ArgumentOutOfRangeException">The row is not in the batch.</exception>
	public Verdict<T> Get<T>(int column, int row) where T : struct
	{
		CheckRow(row);
		Unchecked(column);
		var door = _columns.Plan[column].Door;
		if (!Reads<T>(door))
			throw new InvalidOperationException($"Column {column} is cast through {door}, which does not make a {typeof(T).Name}.");
		var verdict = _columns.Verdicts[column][row];
		if (!verdict.IsOk)
			return new Verdict<T>(Detailed(verdict, column, row));
		var values = _columns.Values[column];
		if (typeof(T) == typeof(bool))
			return As<bool, T>(values[row] != 0);
		if (typeof(T) == typeof(decimal))
			return As<decimal, T>(Read<RawDecimal>(values, row).ToDecimal());
		if (typeof(T) == typeof(Guid))
			return As<Guid, T>(Abi.ToGuid(values.AsSpan(row * 16, 16)));
		if (typeof(T) == typeof(DateTimeOffset))
			return As<DateTimeOffset, T>(Read<RawTimestamp>(values, row).ToDateTimeOffset());
		if (typeof(T) == typeof(DateOnly))
			return As<DateOnly, T>(Read<RawDate>(values, row).ToDateOnly());
		if (typeof(T) == typeof(System.DateTime))
			return As<System.DateTime, T>(Read<RawCivil>(values, row).ToDateTime());
		if (typeof(T) == typeof(TimeOnly))
			return As<TimeOnly, T>(Abi.ToTimeOnly(Read<ulong>(values, row)));
		if (typeof(T) == typeof(TimeSpan))
			return As<TimeSpan, T>(Read<RawDuration>(values, row).ToTimeSpan());
		// The primitives: the value is the bytes the core wrote.
		return new Verdict<T>(new Success<T>(Unsafe.ReadUnaligned<T>(ref values[row * Unsafe.SizeOf<T>()])));
	}

	/// <summary>Refuses a column that declares constraints: a violation is no fault, so a plain getter would read it as passed.</summary>
	void Unchecked(int column)
	{
		if (_columns.Plan[column].IsChecked)
			throw new InvalidOperationException($"Column {column} declares constraints: read it with GetChecked.");
	}

	/// <summary>
	/// The cell at (<paramref name="column"/>, <paramref name="row"/>) as HyperCast cast and judged it:
	/// the value, flagged when <c>multipleOf</c>'s rounding adjusted it and naming the <c>enum</c>
	/// entry it matched, or the rejection, a <see cref="HyperCast.Fault"/> as <see cref="Get{T}"/>
	/// gives it or a <see cref="Violation{T}"/> naming the keyword broken and carrying the value as
	/// cast. <typeparamref name="T"/> is the type of the column's door: <see cref="sbyte"/> …
	/// <see cref="ulong"/>, <see cref="float"/>, <see cref="double"/> and <see cref="decimal"/> for the
	/// numeric doors, <see cref="string"/> for <see cref="Door.Text"/>, where an <c>enum</c> match
	/// reads as the entry as declared. A column that declares no constraints reads too: every cell
	/// that cast passes, unadjusted.
	/// </summary>
	/// <exception cref="InvalidOperationException">The column's door has no checked door that makes a <typeparamref name="T"/>.</exception>
	/// <exception cref="ArgumentOutOfRangeException">The row is not in the batch.</exception>
	public CheckedVerdict<T> GetChecked<T>(int column, int row) where T : notnull
	{
		CheckRow(row);
		var plan = _columns.Plan[column];
		var reads = typeof(T) == typeof(string) ? plan.Door == Door.Text : Column.IsNumeric(plan.Door) && Reads<T>(plan.Door);
		if (!reads)
			throw new InvalidOperationException($"Column {column} is cast through {plan.Door}, which has no checked door making a {typeof(T).Name}.");
		var verdict = _columns.Verdicts[column][row];
		if (!verdict.IsOk && !verdict.IsViolation)
			return new(new Rejection<T>(Detailed(verdict, column, row)));
		var value = CheckedValue<T>(column, row);
		if (_columns.Checks[column] is not { } checks)
			return new(new Checked<T>(value, false, null));
		var check = checks[row];
		if (verdict.IsViolation)
			return new(new Rejection<T>(new Violation<T>(
				HyperCast.Keyword.FromCode(check.Keyword) ?? throw new InvalidOperationException(Workbook.ContractViolation),
				value, verdict.Offset, verdict.Length)));
		if (check.EnumIndex == uint.MaxValue)
			return new(new Checked<T>(value, check.Adjusted != 0, null));
		// A text match is the entry as declared, the caller's own string.
		if (typeof(T) == typeof(string) && plan.Constraints is Constraints<string> { Enum: { } entries })
			value = (T)(object)entries[(int)check.EnumIndex].ToString();
		return new(new Checked<T>(value, check.Adjusted != 0, (int)check.EnumIndex));
	}

	/// <summary>The value the core wrote at <paramref name="row"/>, as the checked door's type.</summary>
	T CheckedValue<T>(int column, int row) where T : notnull
	{
		var values = _columns.Values[column];
		if (typeof(T) == typeof(string))
			return (T)(object)Encoding.UTF8.GetString(Bytes(Read<Native.RawSpan>(values, row)));
		if (typeof(T) == typeof(decimal))
		{
			var exact = Read<RawDecimal>(values, row).ToDecimal();
			return Unsafe.As<decimal, T>(ref exact);
		}
		return Unsafe.ReadUnaligned<T>(ref values[row * Unsafe.SizeOf<T>()]);
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
	/// <exception cref="InvalidOperationException">
	/// The column is not read through <see cref="Door.Text"/>, or declares constraints, whose cells
	/// <see cref="GetChecked{T}"/> reads.
	/// </exception>
	public bool TryGetText(int column, int row, out ReadOnlySpan<byte> utf8)
	{
		CheckRow(row);
		Unchecked(column);
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
	/// The cell of a <see cref="Door.Text"/> column as UTF-16, without making a string: its
	/// bytes decoded into an arena the batch keeps. Empty for an empty cell and for one whose bytes
	/// aren't UTF-8, which <see cref="Verdicts"/> tells apart and <see cref="Raw"/> still reads. Valid until the next read, as
	/// everything a batch gives out is: the arena is only ever appended to while the batch is
	/// current — a span handed out earlier is never written over — and starts over with the
	/// next batch, so after the first few batches it is large enough and nothing is allocated.
	/// </summary>
	/// <exception cref="InvalidOperationException">The column is not read through <see cref="Door.Text"/>.</exception>
	public ReadOnlySpan<char> GetChars(int column, int row)
	{
		if (!TryGetText(column, row, out var utf8) || utf8.IsEmpty)
			return default;
		// UTF-16 never takes more units than UTF-8 takes bytes, a replacement included.
		if (_chars.Length - _charsUsed < utf8.Length)
		{
			// A span handed out earlier keeps the old arena alive and unchanged; the new one
			// starts empty, with room for twice what this batch has decoded so far, so that
			// the next batch like it fits in one.
			_chars = new char[Stingy
				? utf8.Length
				: (int)Math.Min(Math.Max(((long)_charsDecoded + utf8.Length) * 2, 1024), Array.MaxLength)];
			_charsUsed = 0;
			CharsGrown++;
		}
		Utf8.ToUtf16(utf8, _chars.AsSpan(_charsUsed), out _, out var written, replaceInvalidSequences: true);
		var chars = _chars.AsSpan(_charsUsed, written);
		_charsUsed += written;
		_charsDecoded += written;
		return chars;
	}

	/// <summary>
	/// The cell of a <see cref="Door.Text"/> column decoded as UTF-16 into
	/// <paramref name="destination"/>. An empty cell, and one whose bytes aren't UTF-8, writes nothing.
	/// </summary>
	/// <returns>
	/// <see langword="false"/>, and <paramref name="charsWritten"/> zero, if
	/// <paramref name="destination"/> is too small for the cell — in which case what
	/// <paramref name="destination"/> holds is unspecified: the decode may have written part
	/// of the cell into it before running out of room.
	/// </returns>
	/// <exception cref="InvalidOperationException">The column is not read through <see cref="Door.Text"/>.</exception>
	public bool TryGetChars(int column, int row, Span<char> destination, out int charsWritten)
	{
		charsWritten = 0;
		if (!TryGetText(column, row, out var utf8))
			return true;
		if (Utf8.ToUtf16(utf8, destination, out _, out var written, replaceInvalidSequences: true) != System.Buffers.OperationStatus.Done)
			return false;
		charsWritten = written;
		return true;
	}

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

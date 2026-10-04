using System.Buffers;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Text;
using HyperCast;

namespace HyperTabular;

/// <summary>
/// Delimited text — CSV, TSV, any single-byte ASCII separator — read a batch at a time into
/// typed columns, every cell a HyperCast <see cref="Verdict{T}"/>.
/// </summary>
/// <remarks>
/// <para>
/// The native core (<c>libhypertabular</c>) owns no memory and reads no files. It is handed
/// a chunk of input and the buffers to fill, casts each plan column in one native loop, and
/// says how many rows it wrote and how many bytes it is finished with. Everything else is
/// here, which is the point of the design: this class allocates the input buffer, one value
/// array and one verdict array per column, the table that locates each cell, and the arena
/// for the rare escaped cell — once, pinned, and reused for every batch — and puts what the
/// core did not consume back in front of it. The native boundary is crossed once per
/// batch, not once per cell.
/// </para>
/// <para>
/// A value that does not cast is that cell's verdict, and the read goes on. Input that is
/// not rows of cells at all — a record of the wrong width, a quote never closed — is a
/// <see cref="TabularException"/>, raised after every intact row before it has been
/// delivered.
/// </para>
/// <para>
/// Spans handed out for a batch — <see cref="Values{T}"/>, <see cref="Verdicts"/>,
/// <see cref="TryGetText"/>, <see cref="Raw"/> — point into the reader's own buffers and
/// are valid until the next <see cref="Read"/>. Not thread-safe.
/// </para>
/// </remarks>
[SkipLocalsInit]
public sealed unsafe class DelimitedReader : IDisposable
{
	/// <summary>The row ceiling: a single record larger than this is a <see cref="TabularFailure.RowTooLong"/>.</summary>
	public const int MaxRowBytes = 1 << 30;

	/// <summary>The initial read buffer for a stream.</summary>
	public const int DefaultBufferBytes = 256 * 1024;

	/// <summary>Rows per batch unless told otherwise.</summary>
	public const int DefaultBatchRows = 4096;

	const string ContractViolation =
		"libhypertabular reported a contract violation — a binding bug, please report it.";

	/// <summary>
	/// The core's state block, field for field (<c>kernel::delimited::fill::State</c>): where
	/// the input stands. The core writes it; this class reads the position out of it.
	/// </summary>
	[StructLayout(LayoutKind.Sequential)]
	struct State
	{
		public byte Separator;
		public byte Quoting;
		public byte SkipBlankLines;
		public byte Engine;
		public uint Line;
		public ulong Records;
		public ulong Offset;
		public uint Expected;
		public byte Started;
		public byte Reserved0;
		public byte Reserved1;
		public byte Reserved2;
		public Native.RawFailure Failure;
	}

	readonly Column[] _plan;
	readonly int _batchRows;
	/// <summary>Cell-table entries one row takes: the widest ordinal the plan reads, plus two.</summary>
	readonly int _perRow;

	// Every array below is on the pinned object heap: the core is given their addresses, and
	// an address that could move is one that would have to be pinned again on every call.
	readonly State[] _state;
	readonly Native.RawColumnSpec[] _specs;
	readonly Native.RawColumnBuffer[] _buffers;
	readonly byte[][] _values;
	readonly CellVerdict[][] _verdicts;
	Native.RawSpan[] _cells;
	byte[] _arena;
	byte[] _scratch = [];

	// The source: a stream read into _buffer, or memory read in place.
	readonly Stream? _stream;
	readonly bool _leaveOpen;
	byte[] _buffer = [];
	MemoryHandle _pin;
	readonly byte* _memory;
	int _start;
	int _end;
	bool _eof;

	// The batch in hand, and the window of input its spans point into.
	byte* _window;
	int _rows;

	string[]? _header;
	TabularException? _failure;
	bool _disposed;

	/// <summary>Reads UTF-8 delimited text from a stream.</summary>
	/// <param name="utf8">The stream. Read forward only; never sought.</param>
	/// <param name="dialect">The declared dialect.</param>
	/// <param name="plan">The output columns, in output order.</param>
	/// <param name="batchRows">Rows per batch.</param>
	/// <param name="bufferBytes">The initial read buffer; it doubles when a record does not fit.</param>
	/// <param name="leaveOpen">Whether <see cref="Dispose"/> leaves the stream open.</param>
	/// <exception cref="ArgumentException">The dialect's separator or a column's numeric format cannot be honoured.</exception>
	/// <exception cref="TabularException">The header record is structurally broken.</exception>
	public DelimitedReader(
		Stream utf8,
		Dialect dialect,
		ReadOnlySpan<Column> plan,
		int batchRows = DefaultBatchRows,
		int bufferBytes = DefaultBufferBytes,
		bool leaveOpen = false)
		: this(dialect, plan, batchRows)
	{
		ArgumentNullException.ThrowIfNull(utf8);
		ArgumentOutOfRangeException.ThrowIfNegativeOrZero(bufferBytes);
		_stream = utf8;
		_leaveOpen = leaveOpen;
		_buffer = GC.AllocateUninitializedArray<byte>(Math.Min(bufferBytes, MaxRowBytes), pinned: true);
		if (dialect.HasHeader)
			ReadHeader();
	}

	/// <summary>Reads UTF-8 delimited text already in memory, in place — nothing is copied.</summary>
	/// <param name="utf8">The text. Pinned for the reader's lifetime.</param>
	/// <param name="dialect">The declared dialect.</param>
	/// <param name="plan">The output columns, in output order.</param>
	/// <param name="batchRows">Rows per batch.</param>
	/// <exception cref="ArgumentException">The dialect's separator or a column's numeric format cannot be honoured.</exception>
	/// <exception cref="TabularException">The header record is structurally broken.</exception>
	public DelimitedReader(
		ReadOnlyMemory<byte> utf8,
		Dialect dialect,
		ReadOnlySpan<Column> plan,
		int batchRows = DefaultBatchRows)
		: this(dialect, plan, batchRows)
	{
		_pin = utf8.Pin();
		_memory = (byte*)_pin.Pointer;
		_end = utf8.Length;
		_eof = true;
		if (dialect.HasHeader)
			ReadHeader();
	}

	/// <summary>Opens a file of UTF-8 delimited text.</summary>
	/// <param name="path">The file.</param>
	/// <param name="dialect">The declared dialect.</param>
	/// <param name="plan">The output columns, in output order.</param>
	/// <param name="batchRows">Rows per batch.</param>
	/// <param name="bufferBytes">The initial read buffer.</param>
	public static DelimitedReader Open(
		string path,
		Dialect dialect,
		ReadOnlySpan<Column> plan,
		int batchRows = DefaultBatchRows,
		int bufferBytes = DefaultBufferBytes)
	{
		// The reader has its own buffer; a second one inside the stream would only copy.
		var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read,
			bufferSize: 0, FileOptions.SequentialScan);
		try
		{
			return new DelimitedReader(stream, dialect, plan, batchRows, bufferBytes);
		}
		catch
		{
			stream.Dispose();
			throw;
		}
	}

	DelimitedReader(Dialect dialect, ReadOnlySpan<Column> plan, int batchRows)
	{
		ArgumentOutOfRangeException.ThrowIfNegativeOrZero(batchRows);
		_plan = plan.ToArray();
		_batchRows = batchRows;

		var widest = -1;
		_specs = GC.AllocateUninitializedArray<Native.RawColumnSpec>(_plan.Length, pinned: true);
		_buffers = GC.AllocateUninitializedArray<Native.RawColumnBuffer>(_plan.Length, pinned: true);
		_values = new byte[_plan.Length][];
		_verdicts = new CellVerdict[_plan.Length][];
		for (var index = 0; index < _plan.Length; index++)
		{
			var column = _plan[index];
			if (column.Door == Door.Unspecified)
				throw new ArgumentException($"Plan column {index} is default(Column); build one with Column's factories.", nameof(plan));
			widest = Math.Max(widest, column.Ordinal);
			_specs[index] = new Native.RawColumnSpec
			{
				Ordinal = (uint)column.Ordinal,
				Door = (uint)column.Door,
				Param = column.Declared,
				Format = ToRaw(column.Format, index),
			};
			_values[index] = GC.AllocateUninitializedArray<byte>(checked(batchRows * column.ValueSize), pinned: true);
			_verdicts[index] = GC.AllocateUninitializedArray<CellVerdict>(batchRows, pinned: true);
			_buffers[index] = new Native.RawColumnBuffer
			{
				Values = Address(_values[index]),
				Verdicts = Address(_verdicts[index]),
			};
		}
		_perRow = widest + 2;
		_cells = GC.AllocateUninitializedArray<Native.RawSpan>(checked(_perRow * batchRows), pinned: true);
		_arena = GC.AllocateUninitializedArray<byte>(4096, pinned: true);

		if (dialect.Separator > 0x7E)
			throw new ArgumentException($"Separator U+{(int)dialect.Separator:X4} is not a single ASCII byte.", nameof(dialect));
		_state = GC.AllocateUninitializedArray<State>(1, pinned: true);
		var raw = new Native.RawDialect
		{
			Separator = (byte)dialect.Separator,
			Quoting = dialect.Quoting ? (byte)1 : (byte)0,
			SkipBlankLines = dialect.SkipBlankLines ? (byte)1 : (byte)0,
		};
		if (Native.hypertabular_delimited_init((Native.State*)Address(_state), &raw) != Native.Ok)
			throw new ArgumentException(
				$"Separator '{dialect.Separator}' is not tab or printable ASCII other than '\"'.", nameof(dialect));
	}

	/// <summary>The address of a pinned array's first element — stable for the array's lifetime.</summary>
	static T* Address<T>(T[] pinned) where T : unmanaged =>
		(T*)Unsafe.AsPointer(ref MemoryMarshal.GetArrayDataReference(pinned));

	/// <summary>HyperCast's <see cref="NumFormat"/> in the core's 32-byte layout, validated as HyperCast validates it.</summary>
	static Native.RawNumFormat ToRaw(NumFormat format, int column)
	{
		if (format.DecimalSeparator == format.GroupSeparator)
			throw new ArgumentException(
				$"Plan column {column}: decimal and group separators must differ; both are '{format.DecimalSeparator}'.", "plan");
		if (char.IsSurrogate(format.DecimalSeparator) || char.IsSurrogate(format.GroupSeparator))
			throw new ArgumentException($"Plan column {column}: separators must be whole code points.", "plan");
		var raw = new Native.RawNumFormat
		{
			DecimalSep = format.DecimalSeparator,
			GroupSep = format.GroupSeparator,
			Flags = (uint)format.Styles,
		};
		var symbol = format.CurrencySymbol;
		if (symbol.Length == 0)
			return raw;
		foreach (var c in symbol)
			if (char.IsAsciiDigit(c) || char.IsWhiteSpace(c))
				throw new ArgumentException(
					$"Plan column {column}: currency symbol '{symbol}' must not contain a digit or whitespace.", "plan");
		if (!Encoding.UTF8.TryGetBytes(symbol, raw.Currency, out var written))
			throw new ArgumentException(
				$"Plan column {column}: currency symbol '{symbol}' exceeds {NumFormat.MaxCurrencyBytes} UTF-8 bytes.", "plan");
		raw.CurrencyLen = (uint)written;
		return raw;
	}

	/// <summary>The header's names, when the dialect declares one; empty for an input with no record.</summary>
	public IReadOnlyList<string>? Header => _header;

	/// <summary>The number of plan columns.</summary>
	public int ColumnCount => _plan.Length;

	/// <summary>The plan column at <paramref name="column"/>.</summary>
	public Column Column(int column) => _plan[column];

	/// <summary>Rows in the batch in hand; <c>0</c> before the first <see cref="Read"/> and after the last.</summary>
	public int Rows => _rows;

	/// <summary>Records finished so far — the header and skipped blank lines included.</summary>
	public long Records => (long)_state[0].Records;

	/// <summary>What the core is to read next, and whether nothing follows it.</summary>
	byte* Window(out int length, out bool last)
	{
		length = _end - _start;
		last = _eof;
		return (_stream is null ? _memory : Address(_buffer)) + _start;
	}

	/// <summary>Puts the unfinished record at the front of the buffer and reads more behind it.</summary>
	void Refill()
	{
		var stream = _stream!;
		var pending = _end - _start;
		if (_start > 0)
		{
			Buffer.BlockCopy(_buffer, _start, _buffer, 0, pending);
			_start = 0;
			_end = pending;
		}
		if (_end == _buffer.Length)
		{
			// One record fills the buffer: it needs a bigger one.
			if (_buffer.Length >= MaxRowBytes)
			{
				ref readonly var state = ref _state[0];
				throw _failure = new TabularException(TabularFailure.RowTooLong,
					(long)state.Records, (int)state.Line, (long)state.Offset, 0, 0);
			}
			var larger = GC.AllocateUninitializedArray<byte>(
				(int)Math.Min((long)_buffer.Length * 2, MaxRowBytes), pinned: true);
			Buffer.BlockCopy(_buffer, 0, larger, 0, _end);
			_buffer = larger;
		}
		var read = stream.Read(_buffer, _end, _buffer.Length - _end);
		if (read == 0)
			_eof = true;
		_end += read;
	}

	TabularException Structural(in Native.RawFailure failure) =>
		_failure = new TabularException(
			failure.Code == 2 ? TabularFailure.ColumnCount : TabularFailure.UnclosedQuote,
			(long)failure.Record, (int)failure.Line, (long)failure.Byte,
			(int)failure.Expected, (int)failure.Found);

	void ReadHeader()
	{
		var names = GC.AllocateUninitializedArray<Native.RawSpan>(64, pinned: true);
		Native.RawFilled filled;
		while (true)
		{
			var window = Window(out var length, out var last);
			var code = Native.hypertabular_delimited_header(
				(Native.State*)Address(_state), window, (nuint)length, last ? 1u : 0u,
				Address(names), (nuint)names.Length, Address(_arena), (nuint)_arena.Length, &filled);
			switch (code)
			{
				case Native.Ok when filled.Rows > 0:
					var header = new string[(int)filled.Rows];
					for (var index = 0; index < header.Length; index++)
					{
						var name = names[index];
						var from = name.Flagged ? Address(_arena) : window;
						header[index] = Encoding.UTF8.GetString(from + name.Offset, name.Length);
					}
					_header = header;
					_start += (int)filled.Consumed;
					return;
				case Native.Ok:
					var consumed = (int)filled.Consumed;
					_start += consumed;
					if (last && (consumed == length || consumed == 0))
					{
						// An empty input has no header and no rows; the width is unknown.
						_header = [];
						return;
					}
					if (consumed == 0)
						Refill();
					break;
				case Native.ErrCells:
					names = GC.AllocateUninitializedArray<Native.RawSpan>((int)filled.Needed, pinned: true);
					break;
				case Native.ErrArena:
					_arena = GC.AllocateUninitializedArray<byte>((int)filled.Needed, pinned: true);
					break;
				case Native.ErrStructure:
					throw Structural(filled.Failure);
				default:
					throw new InvalidOperationException(ContractViolation);
			}
		}
	}

	/// <summary>
	/// Reads the next batch. <see langword="true"/> with <see cref="Rows"/> rows in hand;
	/// <see langword="false"/> once the input is exhausted.
	/// </summary>
	/// <exception cref="TabularException">
	/// The input is structurally broken. Raised after every intact row before the break has
	/// been delivered, and again on every later call.
	/// </exception>
	public bool Read()
	{
		ObjectDisposedException.ThrowIf(_disposed, this);
		if (_failure is not null)
			throw _failure;
		_rows = 0;
		Native.RawFilled filled;
		while (true)
		{
			var window = Window(out var length, out var last);
			var code = Native.hypertabular_delimited_fill(
				(Native.State*)Address(_state), window, (nuint)length, last ? 1u : 0u,
				Address(_specs), Address(_buffers), (nuint)_plan.Length, (nuint)_batchRows,
				Address(_cells), (nuint)_cells.Length, Address(_arena), (nuint)_arena.Length, &filled);
			switch (code)
			{
				case Native.Ok:
					var consumed = (int)filled.Consumed;
					_start += consumed;
					if (filled.Rows > 0)
					{
						_rows = (int)filled.Rows;
						_window = window;
						return true;
					}
					if (last && (consumed == length || consumed == 0))
						return false;
					if (consumed == 0)
						Refill();
					break;
				case Native.ErrCells:
					_cells = GC.AllocateUninitializedArray<Native.RawSpan>(
						checked((int)filled.Needed * _batchRows), pinned: true);
					break;
				case Native.ErrArena:
					_arena = GC.AllocateUninitializedArray<byte>(
						(int)Math.Max((long)filled.Needed, (long)_arena.Length * 2), pinned: true);
					break;
				case Native.ErrStructure:
					throw Structural(filled.Failure);
				default:
					throw new InvalidOperationException(ContractViolation);
			}
		}
	}

	/// <summary>A column's verdicts for the batch in hand, one per row.</summary>
	public ReadOnlySpan<CellVerdict> Verdicts(int column) => _verdicts[column].AsSpan(0, _rows);

	/// <summary>
	/// A column's values for the batch in hand, one per row, exactly as the core wrote them
	/// — for the doors whose value is a primitive: <see cref="Door.Boolean"/> as
	/// <see cref="bool"/>, the integer and floating-point doors as their own type. The value
	/// of a row whose verdict is not ok is zero. Columns of other doors are read a cell at
	/// a time.
	/// </summary>
	/// <exception cref="InvalidOperationException"><typeparamref name="T"/> is not the type the column's door writes.</exception>
	public ReadOnlySpan<T> Values<T>(int column) where T : unmanaged
	{
		var door = _plan[column].Door;
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
		return MemoryMarshal.Cast<byte, T>(_values[column].AsSpan(0, _rows * sizeof(T)));
	}

	/// <summary>The verdict of the cell at (<paramref name="column"/>, <paramref name="row"/>), checked against the doors the caller's accessor reads.</summary>
	CellVerdict At(int column, int row, Door door, Door also = Door.Unspecified, Door orElse = Door.Unspecified)
	{
		ArgumentOutOfRangeException.ThrowIfGreaterThanOrEqual((uint)row, (uint)_rows, nameof(row));
		var actual = _plan[column].Door;
		if (actual != door && actual != also && actual != orElse)
			throw new InvalidOperationException($"Column {column} is cast through {actual}, not {door}.");
		return _verdicts[column][row];
	}

	T Value<T>(int column, int row) where T : unmanaged =>
		Unsafe.ReadUnaligned<T>(ref _values[column][row * sizeof(T)]);

	Verdict<T> Cell<T>(int column, int row, Door door) where T : unmanaged
	{
		var verdict = At(column, row, door);
		return verdict.IsOk ? Value<T>(column, row) : new Verdict<T>(verdict.ToFault());
	}

	/// <summary>The cell of a <see cref="Door.Boolean"/> column.</summary>
	public Verdict<bool> Boolean(int column, int row)
	{
		var verdict = At(column, row, Door.Boolean);
		return verdict.IsOk ? Value<byte>(column, row) != 0 : new Verdict<bool>(verdict.ToFault());
	}

	/// <summary>The cell of a <see cref="Door.SByte"/> column.</summary>
	public Verdict<sbyte> SByte(int column, int row) => Cell<sbyte>(column, row, Door.SByte);

	/// <summary>The cell of a <see cref="Door.Int16"/> column.</summary>
	public Verdict<short> Int16(int column, int row) => Cell<short>(column, row, Door.Int16);

	/// <summary>The cell of a <see cref="Door.Int32"/> column.</summary>
	public Verdict<int> Int32(int column, int row) => Cell<int>(column, row, Door.Int32);

	/// <summary>The cell of a <see cref="Door.Int64"/> column.</summary>
	public Verdict<long> Int64(int column, int row) => Cell<long>(column, row, Door.Int64);

	/// <summary>The cell of a <see cref="Door.Byte"/> column.</summary>
	public Verdict<byte> Byte(int column, int row) => Cell<byte>(column, row, Door.Byte);

	/// <summary>The cell of a <see cref="Door.UInt16"/> column.</summary>
	public Verdict<ushort> UInt16(int column, int row) => Cell<ushort>(column, row, Door.UInt16);

	/// <summary>The cell of a <see cref="Door.UInt32"/> column.</summary>
	public Verdict<uint> UInt32(int column, int row) => Cell<uint>(column, row, Door.UInt32);

	/// <summary>The cell of a <see cref="Door.UInt64"/> column.</summary>
	public Verdict<ulong> UInt64(int column, int row) => Cell<ulong>(column, row, Door.UInt64);

	/// <summary>The cell of a <see cref="Door.Single"/> column.</summary>
	public Verdict<float> Single(int column, int row) => Cell<float>(column, row, Door.Single);

	/// <summary>The cell of a <see cref="Door.Double"/> column.</summary>
	public Verdict<double> Double(int column, int row) => Cell<double>(column, row, Door.Double);

	/// <summary>The cell of a <see cref="Door.Decimal"/> column, exact.</summary>
	public Verdict<decimal> Decimal(int column, int row)
	{
		var verdict = At(column, row, Door.Decimal);
		if (!verdict.IsOk)
			return new Verdict<decimal>(verdict.ToFault());
		var value = Value<Native.RawDecimal>(column, row);
		return new decimal((int)value.Lo, (int)(value.Lo >> 32), (int)value.Hi, value.Negative != 0, value.Scale);
	}

	/// <summary>The cell of a <see cref="Door.Uuid"/> column.</summary>
	public Verdict<Guid> Uuid(int column, int row)
	{
		var verdict = At(column, row, Door.Uuid);
		return verdict.IsOk
			? new Guid(_values[column].AsSpan(row * 16, 16), bigEndian: true)
			: new Verdict<Guid>(verdict.ToFault());
	}

	/// <summary>
	/// The cell of a <see cref="Door.Timestamp"/>, <see cref="Door.Unix"/> or
	/// <see cref="Door.ExcelSerial"/> column: an instant, at .NET's 100 ns fidelity
	/// (sub-tick nanoseconds truncate).
	/// </summary>
	public Verdict<DateTimeOffset> Timestamp(int column, int row)
	{
		var verdict = At(column, row, Door.Timestamp, Door.Unix, Door.ExcelSerial);
		if (!verdict.IsOk)
			return new Verdict<DateTimeOffset>(verdict.ToFault());
		var value = Value<Native.RawTimestamp>(column, row);
		return new DateTimeOffset(
			System.DateTime.UnixEpoch.Ticks + value.Seconds * TimeSpan.TicksPerSecond + value.Nanos / 100,
			TimeSpan.Zero);
	}

	/// <summary>The cell of a <see cref="Door.Date"/> or <see cref="Door.DateOrdered"/> column.</summary>
	public Verdict<DateOnly> Date(int column, int row)
	{
		var verdict = At(column, row, Door.Date, Door.DateOrdered);
		if (!verdict.IsOk)
			return new Verdict<DateOnly>(verdict.ToFault());
		var value = Value<Native.RawDate>(column, row);
		return new DateOnly(value.Year, value.Month, value.Day);
	}

	/// <summary>
	/// The cell of a <see cref="Door.DateTime"/> column: a wall clock with
	/// <see cref="DateTimeKind.Unspecified"/> — the text named no zone and none is invented.
	/// </summary>
	public Verdict<DateTime> DateTime(int column, int row)
	{
		var verdict = At(column, row, Door.DateTime);
		if (!verdict.IsOk)
			return new Verdict<DateTime>(verdict.ToFault());
		var value = Value<Native.RawCivil>(column, row);
		return new DateTime(value.Year, value.Month, value.Day, 0, 0, 0, DateTimeKind.Unspecified)
			.AddTicks((long)(value.NanosOfDay / 100));
	}

	/// <summary>The cell of a <see cref="Door.Time"/> column.</summary>
	public Verdict<TimeOnly> Time(int column, int row)
	{
		var verdict = At(column, row, Door.Time);
		return verdict.IsOk
			? new TimeOnly((long)(Value<ulong>(column, row) / 100))
			: new Verdict<TimeOnly>(verdict.ToFault());
	}

	/// <summary>The cell of a <see cref="Door.Duration"/> column.</summary>
	public Verdict<TimeSpan> Duration(int column, int row)
	{
		var verdict = At(column, row, Door.Duration);
		if (!verdict.IsOk)
			return new Verdict<TimeSpan>(verdict.ToFault());
		var value = Value<Native.RawDuration>(column, row);
		return new TimeSpan(value.Seconds * TimeSpan.TicksPerSecond + value.Nanos / 100);
	}

	/// <summary>
	/// The cell of a <see cref="Door.Text"/> column: its bytes, untrimmed, quotes resolved.
	/// <see langword="false"/> for a cell with no bytes at all, which is the one way text
	/// fails. The span points into the reader's buffers — zero-copy for every cell that
	/// had no escaped quote in it — and is valid until the next <see cref="Read"/>.
	/// </summary>
	public bool TryGetText(int column, int row, out ReadOnlySpan<byte> utf8)
	{
		if (!At(column, row, Door.Text).IsOk)
		{
			utf8 = default;
			return false;
		}
		var span = Value<Native.RawSpan>(column, row);
		utf8 = new ReadOnlySpan<byte>((span.Flagged ? Address(_arena) : _window) + span.Offset, span.Length);
		return true;
	}

	/// <summary>The cell of a <see cref="Door.Text"/> column as a string, or <see langword="null"/> for an empty cell.</summary>
	public string? GetString(int column, int row) =>
		TryGetText(column, row, out var utf8) ? Encoding.UTF8.GetString(utf8) : null;

	/// <summary>
	/// The text the cell at (<paramref name="column"/>, <paramref name="row"/>) was cast
	/// from, whatever its door and whatever its verdict — what a fault's span indexes, and
	/// what to show for a value that did not cast. Valid until the next call to
	/// <see cref="Raw"/> or <see cref="Read"/>.
	/// </summary>
	public ReadOnlySpan<byte> Raw(int column, int row)
	{
		ArgumentOutOfRangeException.ThrowIfGreaterThanOrEqual((uint)row, (uint)_rows, nameof(row));
		var cell = _cells[row * _perRow + _plan[column].Ordinal];
		var raw = new ReadOnlySpan<byte>(_window + cell.Offset, cell.Length);
		if (!cell.Flagged)
			return raw;
		// A cell with an escaped quote in it: unescaped, as the core cast it.
		if (_scratch.Length < raw.Length)
			_scratch = new byte[Math.Max(raw.Length, 256)];
		fixed (byte* source = raw)
		fixed (byte* target = _scratch)
		{
			var written = Native.hypertabular_delimited_unescape(
				source, (nuint)raw.Length, target, (nuint)_scratch.Length);
			return _scratch.AsSpan(0, (int)written);
		}
	}

	/// <summary>Releases the pinned memory, and the stream unless it was to be left open.</summary>
	public void Dispose()
	{
		if (_disposed)
			return;
		_disposed = true;
		_rows = 0;
		_pin.Dispose();
		if (!_leaveOpen)
			_stream?.Dispose();
	}
}

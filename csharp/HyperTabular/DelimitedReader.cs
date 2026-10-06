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
/// <see cref="Read"/> hands out a <see cref="Batch"/> that is a view of those buffers, valid
/// until the next <see cref="Read"/>. A value that does not cast is that cell's verdict, and
/// the read goes on. Input that is not rows of cells at all — a record of the wrong width, a
/// quote never closed — is a <see cref="TabularException"/>, raised after every intact row
/// before it has been delivered. Not thread-safe.
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

	readonly Dialect _dialect;
	readonly Columns _columns;
	readonly Batch _batch;
	/// <summary>Cell-table entries one row takes: one per source column the plan reaches, and one more.</summary>
	int _perRow;

	// Pinned, as the column arrays are: the core is given their addresses.
	readonly State[] _state;
	Native.RawSpan[] _cells;
	byte[] _arena;
	/// <summary>The last batch came up short with the arena mostly used: it wants a larger one.</summary>
	bool _cramped;

	// The source: a stream read into _buffer, or memory read in place.
	readonly Stream? _stream;
	readonly bool _leaveOpen;
	byte[] _buffer = [];
	MemoryHandle _pin;
	readonly byte* _memory;
	int _start;
	int _end;
	bool _eof;

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
		_dialect = dialect;
		_columns = new Columns(plan, batchRows);
		_batch = new Batch(_columns, workbook: false);
		_perRow = _columns.Width + 1;
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
		if (Native.hypertabular_delimited_init((Native.State*)Columns.Address(_state), &raw) != Native.Ok)
			throw new ArgumentException(
				$"Separator '{dialect.Separator}' is not tab or printable ASCII other than '\"'.", nameof(dialect));
	}

	static T* Address<T>(T[] pinned) where T : unmanaged => Columns.Address(pinned);

	/// <summary>The dialect the reader was built with.</summary>
	public Dialect Dialect => _dialect;

	/// <summary>The plan the reader reads through: column <c>i</c> of every batch is <c>Plan[i]</c>.</summary>
	public IReadOnlyList<Column> Plan => _columns.Plan;

	/// <summary>The header's names, when the dialect declares one; empty for an input with no record.</summary>
	public IReadOnlyList<string>? Header => _header;

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

	TabularException Structural(in Native.RawFailure failure) => _failure = TabularException.From(failure);

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
	/// Reads the next batch: up to the reader's batch size of rows, or <see langword="null"/>
	/// once the input is exhausted. The batch is valid until the next call.
	/// </summary>
	/// <exception cref="TabularException">
	/// The input is structurally broken. Raised after every intact row before the break has
	/// been delivered, and again on every later call.
	/// </exception>
	public Batch? Read()
	{
		ObjectDisposedException.ThrowIf(_disposed, this);
		_batch.Clear();
		if (_failure is not null)
			throw _failure;
		// The core stops a batch at the row the arena has no room for, so an arena too small
		// for a batch's unescaped text makes for short batches, not for an error. The batch
		// that was short has been given back by now, and the arena can move.
		if (_cramped)
		{
			_cramped = false;
			_arena = GC.AllocateUninitializedArray<byte>(checked(_arena.Length * 2), pinned: true);
		}
		var batchRows = _columns.BatchRows;
		Native.RawFilled filled;
		while (true)
		{
			var window = Window(out var length, out var last);
			var code = Native.hypertabular_delimited_fill(
				(Native.State*)Address(_state), window, (nuint)length, last ? 1u : 0u,
				Address(_columns.Specs), Address(_columns.Buffers), (nuint)_columns.Plan.Length, (nuint)batchRows,
				Address(_cells), (nuint)_cells.Length, Address(_arena), (nuint)_arena.Length, &filled);
			switch (code)
			{
				case Native.Ok:
					var consumed = (int)filled.Consumed;
					_start += consumed;
					if (filled.Rows > 0)
					{
						var rows = (int)filled.Rows;
						_cramped = rows < batchRows && (long)filled.ArenaUsed * 2 >= _arena.Length;
						return _batch.Fill(rows, _cells, _perRow, window, _arena);
					}
					if (last && (consumed == length || consumed == 0))
						return null;
					if (consumed == 0)
						Refill();
					break;
				case Native.ErrCells:
					// The core says how many entries one row takes; the table holds a batch of them.
					_perRow = Math.Max(_perRow, (int)filled.Needed);
					_cells = GC.AllocateUninitializedArray<Native.RawSpan>(
						checked((int)filled.Needed * batchRows), pinned: true);
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

	/// <summary>Releases the pinned memory, and the stream unless it was to be left open.</summary>
	public void Dispose()
	{
		if (_disposed)
			return;
		_disposed = true;
		_batch.Clear();
		_pin.Dispose();
		if (!_leaveOpen)
			_stream?.Dispose();
	}
}

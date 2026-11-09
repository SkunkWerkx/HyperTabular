using System.Buffers;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Text;
using HyperCast;

namespace HyperTabular;

/// <summary>
/// Delimited text — CSV, TSV, any single-byte ASCII separator — read a batch at a time into
/// typed columns, every cell a HyperCast <see cref="Verdict{T}"/>. UTF-8 unless the dialect declares
/// another <see cref="TextEncoding"/>, which is decoded ahead of the read.
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
/// A reader is opened either with its plan or without one: opened without, it reads the
/// header first, and <see cref="Header"/> says where each named column is before
/// <see cref="Bind"/> declares the plan. A stream a browser serves is opened, and read, with
/// <see cref="OpenAsync(Stream, Dialect, int, bool, CancellationToken)"/> and
/// <see cref="ReadAsync"/>.
/// </para>
/// <para>
/// <see cref="Read"/> hands out a <see cref="Batch"/> that is a view of those buffers, valid
/// until the next <see cref="Read"/>. A value that does not cast is that cell's verdict, and
/// the read goes on. Input that is not rows of cells at all — a record of the wrong width, a
/// quote never closed — is a <see cref="TabularException"/>, raised after every intact row
/// before it has been delivered. Not thread-safe.
/// </para>
/// <para>
/// The caller's stream runs in the middle of a read. A call from it back into the reader — a
/// read, a bind, a loop over <see cref="Rows"/> — is refused with an
/// <see cref="InvalidOperationException"/>, as is a second read while a <see cref="ReadAsync"/>
/// is pending; a <see cref="Dispose"/> from it ends that read with no batch.
/// </para>
/// </remarks>
[SkipLocalsInit]
public sealed unsafe class DelimitedReader : IDisposable, IAsyncDisposable, IBatchSource
{
	/// <summary>The row ceiling: a single record larger than this is a <see cref="TabularFailure.RowTooLong"/>.</summary>
	public const int MaxRowBytes = 1 << 30;

	/// <summary>A test's ceiling in place of <see cref="MaxRowBytes"/>, on its thread, for the paths the suite proves many times over: each decoded encoding and the async read, where the real one would take a gigabyte a run.</summary>
	[ThreadStatic]
	internal static int RowCeiling;

	/// <summary>The initial read buffer for a stream.</summary>
	public const int DefaultBufferBytes = 256 * 1024;

	/// <summary>Rows per batch unless told otherwise.</summary>
	public const int DefaultBatchRows = 4096;

	/// <summary>
	/// The cell entries a reader's table is sized for at most: what the default 4,096-row batch
	/// of a plan reaching 256 source columns takes (257 entries a row), about 8 MiB. A plan
	/// reaching further gets as many rows as fit — at least one — and its batches are that
	/// short, so the table follows the source's width and not the ordinal times the batch.
	/// </summary>
	internal const int CellBudget = 4096 * 257;

	const string ContractViolation =
		"libhypertabular reported a contract violation — a binding bug, please report it.";

	const string Busy =
		"The reader is in the middle of a read; its stream can't call back into it, and a second read can't start until the first has completed.";

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
	/// <summary>The plan's columns and the batch they are read into: <see langword="null"/> until a plan is bound.</summary>
	Columns? _columns;
	Batch? _batch;
	/// <summary>Cell-table entries one row takes: one per source column the plan reaches, and one more.</summary>
	int _perRow;

	// Pinned, as the column arrays are: the core is given their addresses.
	readonly State[] _state;
	Native.RawSpan[] _cells = [];
	/// <summary>The cell table's entries, for the tests that hold it to <see cref="CellBudget"/>.</summary>
	internal int CellCapacity => _cells.Length;
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
	/// <summary>What stands between the source and the buffer when the dialect declares an encoding.</summary>
	readonly Decoder? _decoder;
	/// <summary>The dialect's tokens as the core reads them: <see langword="null"/> for none.</summary>
	readonly byte[]? _tokens;

	/// <summary>The table the header's names are located in, while the header is being read.</summary>
	Native.RawSpan[]? _names;
	Header? _header;
	TabularException? _failure;
	bool _disposed;
	/// <summary>A call is in progress: a read (until an asynchronous one completes), a bind or the header. The caller's stream runs in the middle of one.</summary>
	bool _busy;

	/// <summary>
	/// Reads delimited text from a stream, the header first: the reader is open, its
	/// <see cref="Header"/> read, and the plan left to <see cref="Bind"/> once the header has
	/// said where each column is.
	/// </summary>
	/// <param name="utf8">The stream. Read forward only; never sought.</param>
	/// <param name="dialect">The declared dialect.</param>
	/// <param name="bufferBytes">The initial read buffer; it doubles when a record does not fit.</param>
	/// <param name="leaveOpen">Whether <see cref="Dispose"/> leaves the stream open.</param>
	/// <exception cref="ArgumentException">The dialect's separator cannot be honoured.</exception>
	/// <exception cref="TabularException">The header record is structurally broken.</exception>
	/// <remarks>
	/// The header is read here, synchronously. A stream that can only be read asynchronously —
	/// a browser's — is opened with <see cref="OpenAsync(Stream, Dialect, int, bool, CancellationToken)"/>.
	/// </remarks>
	public DelimitedReader(Stream utf8, Dialect dialect, int bufferBytes = DefaultBufferBytes, bool leaveOpen = false)
		: this(dialect, null, utf8, bufferBytes, leaveOpen)
	{
		if (dialect.HasHeader)
			ReadHeader();
	}

	/// <summary>Reads delimited text from a stream through <paramref name="plan"/>.</summary>
	/// <param name="utf8">The stream. Read forward only; never sought.</param>
	/// <param name="dialect">The declared dialect.</param>
	/// <param name="plan">The output columns, in output order.</param>
	/// <param name="batchRows">Rows per batch.</param>
	/// <param name="bufferBytes">The initial read buffer; it doubles when a record does not fit.</param>
	/// <param name="leaveOpen">Whether <see cref="Dispose"/> leaves the stream open.</param>
	/// <exception cref="ArgumentException">The dialect's separator or a column's numeric format cannot be honoured, a column's ordinal is 2²⁰ or more, or a column reads past the columns the header gives every record.</exception>
	/// <exception cref="TabularException">The header record is structurally broken.</exception>
	public DelimitedReader(
		Stream utf8,
		Dialect dialect,
		ReadOnlySpan<Column> plan,
		int batchRows = DefaultBatchRows,
		int bufferBytes = DefaultBufferBytes,
		bool leaveOpen = false)
		: this(dialect, Bound(dialect, plan, batchRows, bufferBytes), utf8, bufferBytes, leaveOpen)
	{
		if (dialect.HasHeader)
			ReadHeader();
		WithinHeader();
	}

	/// <summary>
	/// Reads delimited text already in memory, in place — nothing is copied unless the dialect declares an encoding, which has to be decoded — the header
	/// first, leaving the plan to <see cref="Bind"/>.
	/// </summary>
	/// <param name="utf8">The text. Pinned for the reader's lifetime.</param>
	/// <param name="dialect">The declared dialect.</param>
	/// <exception cref="ArgumentException">The dialect's separator cannot be honoured.</exception>
	/// <exception cref="TabularException">The header record is structurally broken.</exception>
	public DelimitedReader(ReadOnlyMemory<byte> utf8, Dialect dialect)
		: this(dialect, null, utf8)
	{
		if (dialect.HasHeader)
			ReadHeader();
	}

	/// <summary>Reads delimited text already in memory, in place — nothing is copied unless the dialect declares an encoding, which has to be decoded — through <paramref name="plan"/>.</summary>
	/// <param name="utf8">The text. Pinned for the reader's lifetime.</param>
	/// <param name="dialect">The declared dialect.</param>
	/// <param name="plan">The output columns, in output order.</param>
	/// <param name="batchRows">Rows per batch.</param>
	/// <exception cref="ArgumentException">The dialect's separator or a column's numeric format cannot be honoured, a column's ordinal is 2²⁰ or more, or a column reads past the columns the header gives every record.</exception>
	/// <exception cref="TabularException">The header record is structurally broken.</exception>
	public DelimitedReader(
		ReadOnlyMemory<byte> utf8,
		Dialect dialect,
		ReadOnlySpan<Column> plan,
		int batchRows = DefaultBatchRows)
		: this(dialect, Bound(dialect, plan, batchRows), utf8)
	{
		if (dialect.HasHeader)
			ReadHeader();
		WithinHeader();
	}

	/// <summary>Opens a file of delimited text and reads its header, leaving the plan to <see cref="Bind"/>.</summary>
	/// <param name="path">The file.</param>
	/// <param name="dialect">The declared dialect.</param>
	/// <param name="bufferBytes">The initial read buffer.</param>
	public static DelimitedReader Open(string path, Dialect dialect, int bufferBytes = DefaultBufferBytes)
	{
		CheckSizes(dialect, [], DefaultBatchRows, bufferBytes);
		var stream = OpenFile(path);
		try
		{
			return new DelimitedReader(stream, dialect, bufferBytes);
		}
		catch
		{
			stream.Dispose();
			throw;
		}
	}

	/// <summary>Opens a file of delimited text, read through <paramref name="plan"/>.</summary>
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
		CheckSizes(dialect, plan, batchRows, bufferBytes);
		var stream = OpenFile(path);
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

	// The reader has its own buffer; a second one inside the stream would only copy.
	static FileStream OpenFile(string path) =>
		new(path, FileMode.Open, FileAccess.Read, FileShare.Read, bufferSize: 0, FileOptions.SequentialScan);

	/// <summary>
	/// Opens a stream of delimited text and awaits its header, leaving the plan to
	/// <see cref="Bind"/>: the constructor, for a stream that is read asynchronously — the only
	/// way a browser's (an <c>HttpClient</c> response, a picked file) can be read.
	/// </summary>
	/// <param name="utf8">The stream. Read forward only; never sought.</param>
	/// <param name="dialect">The declared dialect.</param>
	/// <param name="bufferBytes">The initial read buffer; it doubles when a record does not fit.</param>
	/// <param name="leaveOpen">Whether disposing the reader leaves the stream open.</param>
	/// <param name="cancellationToken">Observed before anything is done, and at every read of the stream.</param>
	/// <exception cref="ArgumentException">The dialect's separator cannot be honoured.</exception>
	/// <exception cref="TabularException">The header record is structurally broken.</exception>
	/// <exception cref="OperationCanceledException">The token was cancelled.</exception>
	public static ValueTask<DelimitedReader> OpenAsync(
		Stream utf8,
		Dialect dialect,
		int bufferBytes = DefaultBufferBytes,
		bool leaveOpen = false,
		CancellationToken cancellationToken = default)
	{
		if (cancellationToken.IsCancellationRequested)
			return ValueTask.FromCanceled<DelimitedReader>(cancellationToken);
		return HeaderAsync(new DelimitedReader(dialect, null, utf8, bufferBytes, leaveOpen), cancellationToken);
	}

	/// <summary>
	/// Opens a stream of delimited text through <paramref name="plan"/> and awaits its
	/// header: <see cref="OpenAsync(Stream, Dialect, int, bool, CancellationToken)"/> with the
	/// plan declared up front.
	/// </summary>
	/// <param name="utf8">The stream. Read forward only; never sought.</param>
	/// <param name="dialect">The declared dialect.</param>
	/// <param name="plan">The output columns, in output order.</param>
	/// <param name="batchRows">Rows per batch.</param>
	/// <param name="bufferBytes">The initial read buffer; it doubles when a record does not fit.</param>
	/// <param name="leaveOpen">Whether disposing the reader leaves the stream open.</param>
	/// <param name="cancellationToken">Observed before anything is done, and at every read of the stream.</param>
	/// <exception cref="ArgumentException">The dialect's separator or a column's numeric format cannot be honoured, a column's ordinal is 2²⁰ or more, or a column reads past the columns the header gives every record.</exception>
	/// <exception cref="TabularException">The header record is structurally broken.</exception>
	/// <exception cref="OperationCanceledException">The token was cancelled.</exception>
	public static ValueTask<DelimitedReader> OpenAsync(
		Stream utf8,
		Dialect dialect,
		ReadOnlySpan<Column> plan,
		int batchRows = DefaultBatchRows,
		int bufferBytes = DefaultBufferBytes,
		bool leaveOpen = false,
		CancellationToken cancellationToken = default)
	{
		if (cancellationToken.IsCancellationRequested)
			return ValueTask.FromCanceled<DelimitedReader>(cancellationToken);
		return HeaderAsync(
			new DelimitedReader(dialect, Bound(dialect, plan, batchRows, bufferBytes), utf8, bufferBytes, leaveOpen), cancellationToken);
	}

	static async ValueTask<DelimitedReader> HeaderAsync(DelimitedReader reader, CancellationToken cancellationToken)
	{
		if (reader._dialect.HasHeader)
		{
			reader.Enter();
			try
			{
				while (!reader.HeaderStep())
					await reader.RefillAsync(cancellationToken).ConfigureAwait(false);
			}
			finally
			{
				reader._busy = false;
			}
		}
		reader.WithinHeader();
		return reader;
	}

	/// <summary>A reader of a stream, its header not yet read.</summary>
	DelimitedReader(Dialect dialect, Columns? columns, Stream utf8, int bufferBytes, bool leaveOpen)
		: this(dialect, columns)
	{
		ArgumentNullException.ThrowIfNull(utf8);
		// The batch's size is the columns' to answer for: this asks about the buffer alone.
		var start = (int)Columns.CheckSizes(default, 1, bufferBytes, 0).BufferBytes;
		_stream = utf8;
		_leaveOpen = leaveOpen;
		_buffer = GC.AllocateUninitializedArray<byte>(start, pinned: true);
		if (dialect.Encoding != TextEncoding.Utf8)
			_decoder = new Decoder((uint)dialect.Encoding, start);
	}

	/// <summary>A reader of memory, its header not yet read. Under a declared encoding the memory is the decoder's, and what it decodes to lands in a buffer.</summary>
	DelimitedReader(Dialect dialect, Columns? columns, ReadOnlyMemory<byte> text)
		: this(dialect, columns)
	{
		_pin = text.Pin();
		_memory = (byte*)_pin.Pointer;
		if (dialect.Encoding == TextEncoding.Utf8)
		{
			_end = text.Length;
			_eof = true;
			return;
		}
		_buffer = GC.AllocateUninitializedArray<byte>(DefaultBufferBytes, pinned: true);
		_decoder = new Decoder((uint)dialect.Encoding, _memory, text.Length);
	}

	/// <summary>A plan's columns, as a dialect has them read.</summary>
	/// <summary>The core's answer on these sizes, ahead of touching a file.</summary>
	static void CheckSizes(Dialect dialect, ReadOnlySpan<Column> plan, int batchRows, int bufferBytes) =>
		Columns.CheckSizes(Columns.SizeSpecs(plan), batchRows, bufferBytes, SizeFlags(dialect));

	static uint SizeFlags(Dialect dialect) =>
		(dialect.Notes ? Columns.NotesFlag : 0) | (dialect.LongRows == LongRows.Report ? Columns.WidthsFlag : 0);

	static Columns Bound(Dialect dialect, ReadOnlySpan<Column> plan, int batchRows, int bufferBytes = 1) =>
		new(plan, batchRows, dialect.Notes, rowWidths: dialect.LongRows == LongRows.Report, bufferBytes: bufferBytes);

	/// <summary>
	/// The dialect made the core's, and the plan's columns when the plan came with the reader:
	/// built first, so that a plan that cannot be honoured is refused before any input is read.
	/// </summary>
	DelimitedReader(Dialect dialect, Columns? columns)
	{
		_dialect = dialect;
		_arena = GC.AllocateUninitializedArray<byte>(4096, pinned: true);
		if (columns is not null)
			Attach(columns);

		if (dialect.Separator > 0x7E)
			throw new ArgumentException($"Separator U+{(int)dialect.Separator:X4} is not a single ASCII byte.", nameof(dialect));
		// As many 64-byte elements as the library's block takes; only the first's fields are read.
		dialect.CheckDeclared();
		_state = GC.AllocateUninitializedArray<State>(
			(int)((Native.hypertabular_delimited_state_size() + 63) / 64), pinned: true);
		var raw = new Native.RawDelimitedOptions
		{
			Size = (uint)sizeof(Native.RawDelimitedOptions),
			Separator = (byte)dialect.Separator,
			Quoting = dialect.Quoting ? (byte)1 : (byte)0,
			SkipBlankLines = dialect.SkipBlankLines ? (byte)1 : (byte)0,
			Skip = dialect.Skip,
			HeaderRow = dialect.HeaderRow,
			ShortRows = (uint)dialect.ShortRows,
			LongRows = (uint)dialect.LongRows,
		};
		if (Native.hypertabular_delimited_init_with((Native.State*)Columns.Address(_state), &raw) != Native.Ok)
			throw new ArgumentException(
				$"Separator '{dialect.Separator}' is not tab or printable ASCII other than '\"'.", nameof(dialect));
		if (dialect.Tokens.Packed.Length > 0)
			_tokens = Columns.Pinned(dialect.Tokens.Packed);
	}

	/// <summary>The rows a cell table of <paramref name="perRow"/> entries a row is sized for: as many as <see cref="CellBudget"/> holds, from one to a batch.</summary>
	static long CellRows(long perRow, int batchRows) => Math.Clamp(CellBudget / perRow, 1, batchRows);

	/// <summary>Sizes everything a batch is read into by the plan's columns.</summary>
	void Attach(Columns columns)
	{
		// Every ordinal is below 2²⁰, so this is at most CellBudget or one row of 2²⁰ + 1.
		long perRow = columns.Width + 1;
		var cells = perRow * CellRows(perRow, columns.BatchRows);
		_perRow = (int)perRow;
		_cells = GC.AllocateUninitializedArray<Native.RawSpan>((int)cells, pinned: true);
		_batch = new Batch(this, columns, workbook: false);
		_columns = columns;
	}

	/// <summary>
	/// Declares the plan a reader opened without one reads through — once, before the first
	/// read, and usually after <see cref="Header"/> has said where each column is
	/// (<c>Column.Int32(reader.Header!.Ordinal("id"))</c>). Every column array, the cell table
	/// and the batch are sized here, by the plan and <paramref name="batchRows"/>.
	/// </summary>
	/// <param name="plan">The output columns, in output order.</param>
	/// <param name="batchRows">Rows per batch.</param>
	/// <exception cref="InvalidOperationException">The reader already has a plan, or is in the middle of a read (from its stream).</exception>
	/// <exception cref="ArgumentException">A column's numeric format cannot be honoured, a column's ordinal is 2²⁰ or more, or a column reads past the columns the header gives every record; the reader is left without a plan.</exception>
	public void Bind(ReadOnlySpan<Column> plan, int batchRows = DefaultBatchRows)
	{
		ObjectDisposedException.ThrowIf(_disposed, this);
		Enter();
		try
		{
			if (_columns is not null)
				throw new InvalidOperationException("The reader already has a plan; a plan is bound once.");
			var columns = Bound(_dialect, plan, batchRows);
			Within(columns.Plan);
			Attach(columns);
		}
		finally
		{
			_busy = false;
		}
	}

	/// <summary>
	/// Refuses the first plan column past the width a header has fixed: every record has the
	/// header's count of cells, so a column past it could only ever be empty.
	/// </summary>
	void Within(Column[] plan)
	{
		if (ColumnCount is not int width)
			return;
		for (var index = 0; index < plan.Length; index++)
		{
			if (plan[index].Ordinal >= width)
				throw new ArgumentException(
					$"Plan column {index} reads source column {plan[index].Ordinal}, past the {width} columns the header gives every record.", "plan");
		}
	}

	/// <summary>
	/// A plan that came with the reader, held to the header once it has been read. A reader
	/// refused unpins its memory; its stream is left as a header that failed leaves it.
	/// </summary>
	void WithinHeader()
	{
		if (_columns is null)
			return;
		try
		{
			Within(_columns.Plan);
		}
		catch
		{
			Release();
			throw;
		}
	}

	/// <summary>Whether a plan has been bound: always, for a reader opened with one.</summary>
	public bool IsBound => _columns is not null;

	static T* Address<T>(T[] pinned) where T : unmanaged => Columns.Address(pinned);

	/// <summary>The dialect the reader was built with.</summary>
	public Dialect Dialect => _dialect;

	/// <summary>The plan the reader reads through: column <c>i</c> of every batch is <c>Plan[i]</c>. Empty until one is bound.</summary>
	public IReadOnlyList<Column> Plan => _columns?.Plan ?? [];

	/// <summary>
	/// The header's names, when the dialect declares one — read when the reader is opened, so
	/// they are to hand before the plan is bound; empty for an input with no record.
	/// <see langword="null"/> when the dialect declares no header.
	/// </summary>
	public Header? Header => _header;

	/// <summary>
	/// How many cells a record has: the header's count once it has been read, otherwise the
	/// first record's once it has been; <see langword="null"/> until then.
	/// </summary>
	public int? ColumnCount => _state[0].Expected is var expected and not 0 ? (int)expected : null;

	/// <summary>Records finished so far — the header and skipped blank lines included.</summary>
	public long Records => (long)_state[0].Records;

	/// <summary>What the core is to read next, and whether nothing follows it.</summary>
	byte* Window(out int length, out bool last)
	{
		length = _end - _start;
		last = _eof;
		return (_stream is null && _decoder is null ? _memory : Address(_buffer)) + _start;
	}

	/// <summary>Puts the unfinished record at the front of the buffer.</summary>
	void Compact()
	{
		var pending = _end - _start;
		if (_start > 0)
		{
			Buffer.BlockCopy(_buffer, _start, _buffer, 0, pending);
			_start = 0;
			_end = pending;
		}
	}

	/// <summary>Doubles the buffer, up to the row ceiling.</summary>
	void Grow()
	{
		if (_buffer.Length >= (RowCeiling > 0 ? RowCeiling : MaxRowBytes))
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

	/// <summary>
	/// What every refill does ahead of the source: the unfinished record to the front, room behind it,
	/// and a failure the decoder is holding, now that the fill has had all the text ahead of it.
	/// </summary>
	/// <returns><see langword="false"/> if nothing is to be read: the fill has one more look.</returns>
	bool MakeRoom()
	{
		Compact();
		if (_decoder is { Failed: true })
		{
			// A \r the fill is holding ends a row whatever comes next, and only waits to see if a \n
			// is part of it; nothing is coming, and this says so. In a quote it ends nothing, and the
			// record is as cut as it was.
			if (_end == 0 || _buffer[_end - 1] != (byte)'\r')
			{
				ref readonly var state = ref _state[0];
				throw _failure = new TabularException(TabularFailure.Encoding,
					(long)state.Records, (int)state.Line, (long)_decoder.FailedAt, 0, 0);
			}
			if (_end == _buffer.Length)
				Grow();
			_buffer[_end++] = (byte)'\n';
			return false;
		}
		// One record fills the buffer: it needs a bigger one.
		if (_end == _buffer.Length)
			Grow();
		return true;
	}

	/// <summary>Counts what a read of the stream brought in.</summary>
	void Landed(int read)
	{
		if (read == 0)
			_eof = true;
		_end += read;
	}

	/// <summary>Puts the unfinished record at the front of the buffer and reads more behind it.</summary>
	void Refill()
	{
		if (!MakeRoom())
			return;
		if (_decoder is null)
		{
			Landed(Returned(_stream!.Read(_buffer, _end, _buffer.Length - _end)));
			return;
		}
		while (!Decode())
			_decoder.Landed(Returned(_stream!.Read(_decoder.Raw!, _decoder.End, _decoder.Raw!.Length - _decoder.End)));
	}

	/// <summary>
	/// What a read of the stream brought in, once the stream has given control back: a reader the
	/// stream (or anything it ran) disposed ends the read here, with no batch, rather than going on.
	/// </summary>
	int Returned(int read)
	{
		if (_disposed)
			throw new ObjectDisposedException(GetType().FullName, "The reader was disposed in the middle of a read.");
		return read;
	}

	/// <summary>
	/// <see cref="Refill"/>, awaiting the stream. A read that is cancelled throws before it
	/// counts anything, so the reader is as it was: what was buffered stays buffered.
	/// </summary>
	async ValueTask RefillAsync(CancellationToken cancellationToken)
	{
		cancellationToken.ThrowIfCancellationRequested();
		if (!MakeRoom())
			return;
		if (_decoder is null)
		{
			Landed(Returned(await _stream!.ReadAsync(_buffer.AsMemory(_end), cancellationToken).ConfigureAwait(false)));
			return;
		}
		while (!Decode())
			_decoder.Landed(Returned(await _stream!.ReadAsync(_decoder.Raw.AsMemory(_decoder.End), cancellationToken).ConfigureAwait(false)));
	}

	/// <summary>
	/// Decodes what the decoder holds behind what the buffer holds, with whatever that wants first.
	/// </summary>
	/// <returns><see langword="true"/> once the fill has something new to look at: text, the end, or a failure to raise when it next asks. <see langword="false"/> if the stream has to bring more first.</returns>
	bool Decode()
	{
		var decoder = _decoder!;
		while (true)
		{
			var room = _buffer.Length - _end;
			Native.RawTranscoded done;
			if (Native.hypertabular_transcode(decoder.Encoding, decoder.Input + decoder.At, (nuint)(decoder.End - decoder.At),
				decoder.Eof ? 1u : 0u, Address(_buffer) + _end, (nuint)room, &done) != Native.Ok)
				throw new InvalidOperationException(ContractViolation);
			if (done.Failed != 0)
			{
				decoder.Failed = true;
				decoder.FailedAt = decoder.Offset + done.FailureAt;
			}
			decoder.At += (int)done.Consumed;
			decoder.Offset += done.Consumed;
			if (done.Written > 0)
			{
				_end += (int)done.Written;
				return true;
			}
			// The next refill raises it, once the fill has had one more look.
			if (decoder.Failed)
				return true;
			if (decoder.At < decoder.End && room < Decoder.MaxCharacter)
			{
				Grow();
				continue;
			}
			if (decoder.Eof)
			{
				_eof = true;
				return true;
			}
			decoder.Compact();
			return false;
		}
	}

	/// <summary>
	/// The step ahead of the fill, for a dialect that declares an encoding: the original bytes, which
	/// go through the core's transcoder into the reader's buffer, where the fill reads them as it would
	/// have read UTF-8.
	/// </summary>
	sealed class Decoder
	{
		/// <summary>The most bytes of UTF-8 one character takes: with this much room, a transcode that writes nothing and does not fail wants more input.</summary>
		internal const int MaxCharacter = 4;

		internal readonly uint Encoding;
		/// <summary>What a stream's reads land in: fixed in size, since what is left when more is wanted is less than a character. <see langword="null"/> over memory.</summary>
		internal readonly byte[]? Raw;
		/// <summary>The original bytes, <see cref="At"/> to <see cref="End"/> of them not yet decoded.</summary>
		internal readonly byte* Input;
		internal int At;
		internal int End;
		/// <summary>Whether the source has ended.</summary>
		internal bool Eof;
		/// <summary>The original bytes decoded so far: added to a call's <c>FailureAt</c>, this is the input's offset.</summary>
		internal ulong Offset;
		/// <summary>A unit has no character, at <see cref="FailedAt"/> in the original input: raised only when the fill has had all that was decoded ahead of it, and wants more.</summary>
		internal bool Failed;
		internal ulong FailedAt;

		/// <summary>A decoder of a stream.</summary>
		internal Decoder(uint encoding, int size)
		{
			Encoding = encoding;
			Raw = GC.AllocateUninitializedArray<byte>(Math.Max(size, MaxCharacter), pinned: true);
			Input = Address(Raw);
		}

		/// <summary>A decoder of pinned memory, all there already.</summary>
		internal Decoder(uint encoding, byte* input, int length)
		{
			Encoding = encoding;
			Input = input;
			End = length;
			Eof = true;
		}

		/// <summary>Puts what is left undecoded at the front.</summary>
		internal void Compact()
		{
			new ReadOnlySpan<byte>(Input + At, End - At).CopyTo(new Span<byte>(Input, End));
			End -= At;
			At = 0;
		}

		/// <summary>Counts what a read of the stream brought in.</summary>
		internal void Landed(int read)
		{
			if (read == 0)
				Eof = true;
			End += read;
		}
	}

	TabularException Structural(in Native.RawFailure failure) => _failure = TabularException.From(failure);

	/// <summary>
	/// Starts a call that reads or changes the reader, refusing one made from inside another: the
	/// caller's stream runs in the middle of a read, and the read has its buffers' offsets in hand.
	/// </summary>
	void Enter()
	{
		if (_busy)
			throw new InvalidOperationException(Busy);
		_busy = true;
	}

	void ReadHeader()
	{
		Enter();
		try
		{
			while (!HeaderStep())
				Refill();
		}
		finally
		{
			_busy = false;
		}
	}

	/// <summary>Reads the header from what is buffered: <see langword="true"/> once it has been read, <see langword="false"/> if the core needs more input first.</summary>
	bool HeaderStep()
	{
		var names = _names ??= GC.AllocateUninitializedArray<Native.RawSpan>(64, pinned: true);
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
					var header = new Header.Builder((int)filled.Rows);
					for (var index = 0; index < (int)filled.Rows; index++)
					{
						var name = names[index];
						var from = name.Flagged ? Address(_arena) : window;
						header.Add(new ReadOnlySpan<byte>(from + name.Offset, name.Length));
					}
					_header = header.Build();
					_names = null;
					_start += (int)filled.Consumed;
					return true;
				case Native.Ok:
					var consumed = (int)filled.Consumed;
					_start += consumed;
					if (last && (consumed == length || consumed == 0))
					{
						// An empty input has no header and no rows; the width is unknown.
						_header = HyperTabular.Header.Empty;
						_names = null;
						return true;
					}
					if (consumed == 0)
						return false;
					break;
				case Native.ErrCells:
					names = _names = GC.AllocateUninitializedArray<Native.RawSpan>((int)filled.Needed, pinned: true);
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

	/// <summary>What every read does first: ends the batch lent last, and says whether there is anything to read through.</summary>
	void Begin()
	{
		ObjectDisposedException.ThrowIf(_disposed, this);
		_batch?.Clear();
		if (_failure is not null)
			throw _failure;
		if (_batch is null)
			throw new InvalidOperationException("The reader has no plan yet: Bind one before reading.");
		// The core stops a batch at the row the arena has no room for, so an arena too small
		// for a batch's unescaped text makes for short batches, not for an error. The batch
		// that was short has been given back by now, and the arena can move.
		if (_cramped)
		{
			_cramped = false;
			_arena = GC.AllocateUninitializedArray<byte>(checked(_arena.Length * 2), pinned: true);
		}
	}

	/// <summary>Where a fill from what is buffered left the read.</summary>
	enum Step
	{
		/// <summary>A batch of rows is ready.</summary>
		Rows,
		/// <summary>The input has no more rows.</summary>
		End,
		/// <summary>The core needs more of the stream first.</summary>
		Input,
	}

	/// <summary>Fills a batch from what is buffered.</summary>
	Step FillStep()
	{
		var columns = _columns!;
		var batchRows = columns.BatchRows;
		Native.RawFilled filled;
		while (true)
		{
			var window = Window(out var length, out var last);
			// Only the cells, the arena, the tokens and the row widths are the fill's; the rest stays empty.
			var buffers = new Native.RawBuffersWith
			{
				Size = (uint)sizeof(Native.RawBuffersWith),
				Arena = Address(_arena),
				ArenaCap = (nuint)_arena.Length,
				Cells = Address(_cells),
				CellsCap = (nuint)_cells.Length,
				Tokens = _tokens is null ? null : Address(_tokens),
				TokensLen = _tokens is null ? 0 : (nuint)_tokens.Length,
				RowWidths = columns.RowWidths is null ? null : Address(columns.RowWidths),
			};
			var code = Native.hypertabular_delimited_fill_with(
				(Native.State*)Address(_state), window, (nuint)length, last ? 1u : 0u,
				Address(columns.Specs), Address(columns.Buffers), (nuint)columns.Plan.Length, (nuint)batchRows,
				&buffers, &filled);
			switch (code)
			{
				case Native.Ok:
					var consumed = (int)filled.Consumed;
					_start += consumed;
					if (filled.Rows > 0)
					{
						var rows = (int)filled.Rows;
						// Short of what the cell table holds, not of the batch: a plan reaching past
						// the budget fills fewer rows than a batch every time, and that is no
						// reason to grow the arena.
						_cramped = rows < Math.Min(batchRows, _cells.Length / _perRow) && (long)filled.ArenaUsed * 2 >= _arena.Length;
						_batch!.Fill(rows, _cells, _perRow, window, _arena, _state[0].Expected);
						return Step.Rows;
					}
					if (last && (consumed == length || consumed == 0))
						return Step.End;
					if (consumed == 0)
						return Step.Input;
					break;
				case Native.ErrCells:
					// The core says how many entries one row takes; the table holds as many rows of
					// them as the budget does.
					_perRow = Math.Max(_perRow, (int)filled.Needed);
					_cells = GC.AllocateUninitializedArray<Native.RawSpan>(
						checked((int)((long)filled.Needed * CellRows((long)filled.Needed, batchRows))), pinned: true);
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

	/// <summary>
	/// Reads the next batch: up to the reader's batch size of rows, or <see langword="null"/>
	/// once the input is exhausted. The batch is valid until the next call.
	/// </summary>
	/// <exception cref="TabularException">
	/// The input is structurally broken. Raised after every intact row before the break has
	/// been delivered, and again on every later call.
	/// </exception>
	/// <exception cref="InvalidOperationException">No plan has been bound (not final: bind one and read), or the reader is in the middle of a read: the call came from its stream, or while a <see cref="ReadAsync"/> is pending.</exception>
	/// <exception cref="ObjectDisposedException">The reader is disposed, or was disposed while this read waited on the stream.</exception>
	public Batch? Read()
	{
		ObjectDisposedException.ThrowIf(_disposed, this);
		Enter();
		try
		{
			Begin();
			while (true)
			{
				switch (FillStep())
				{
					case Step.Rows:
						return _batch;
					case Step.End:
						return null;
					default:
						Refill();
						break;
				}
			}
		}
		finally
		{
			_busy = false;
		}
	}

	/// <summary>
	/// <see cref="Read"/>, awaiting the stream whenever the core needs more of it. A reader of
	/// memory, or one whose next batch is already buffered, completes synchronously; the
	/// native fill itself is synchronous, and runs between the stream's reads.
	/// </summary>
	/// <param name="cancellationToken">
	/// Observed before anything is done and at every read of the stream. A cancelled read
	/// loses nothing: what had been read stays buffered, and the next read — synchronous or
	/// not — goes on from there.
	/// </param>
	/// <exception cref="TabularException">
	/// The input is structurally broken. Raised after every intact row before the break has
	/// been delivered, and again on every later call.
	/// </exception>
	/// <exception cref="InvalidOperationException">No plan has been bound (not final: bind one and read), or the reader is in the middle of a read: the call came from its stream, or while another <see cref="ReadAsync"/> is pending.</exception>
	/// <exception cref="ObjectDisposedException">The reader is disposed, or was disposed while this read waited on the stream.</exception>
	/// <exception cref="OperationCanceledException">The token was cancelled.</exception>
	/// <remarks>One read at a time: until this one has completed, another read or a bind is refused.</remarks>
	public ValueTask<Batch?> ReadAsync(CancellationToken cancellationToken = default)
	{
		if (cancellationToken.IsCancellationRequested)
			return ValueTask.FromCanceled<Batch?>(cancellationToken);
		try
		{
			ObjectDisposedException.ThrowIf(_disposed, this);
			Enter();
		}
		catch (Exception e)
		{
			return ValueTask.FromException<Batch?>(e);
		}
		try
		{
			Begin();
			switch (FillStep())
			{
				case Step.Rows:
					_busy = false;
					return new(_batch);
				case Step.End:
					_busy = false;
					return new((Batch?)null);
			}
		}
		catch (Exception e)
		{
			_busy = false;
			return ValueTask.FromException<Batch?>(e);
		}
		return ReadRestAsync(cancellationToken);
	}

	/// <summary>The rest of a <see cref="ReadAsync"/> that has to wait on the stream: the reader stays busy until it completes.</summary>
	async ValueTask<Batch?> ReadRestAsync(CancellationToken cancellationToken)
	{
		try
		{
			while (true)
			{
				await RefillAsync(cancellationToken).ConfigureAwait(false);
				switch (FillStep())
				{
					case Step.Rows:
						return _batch;
					case Step.End:
						return null;
				}
			}
		}
		finally
		{
			_busy = false;
		}
	}

	/// <summary>
	/// Every row left, batch by batch: <c>foreach (var row in reader.Rows())</c> calls
	/// <see cref="Read"/> whenever a batch runs out. Each <see cref="Row"/> is a view of the
	/// current batch and is over when the loop moves past it; do not keep one.
	/// </summary>
	public RowSequence Rows() => new(this);

	/// <summary>Releases the pinned memory, and the stream unless it was to be left open.</summary>
	/// <remarks>
	/// Never refused: called from the reader's stream in the middle of a read, or while a
	/// <see cref="ReadAsync"/> is still waiting on the stream, it disposes the reader there and then,
	/// and that read ends with an <see cref="ObjectDisposedException"/> once the stream gives control
	/// back, handing out no batch. A <c>Dispose</c> from another thread that lands after the stream
	/// has given control back is concurrent use, which the reader doesn't support: that read may
	/// still hand out its batch, whole and correct, and the next read is refused.
	/// </remarks>
	public void Dispose()
	{
		if (!Release())
			return;
		if (!_leaveOpen)
			_stream?.Dispose();
	}

	/// <summary>Releases the pinned memory, and disposes the stream asynchronously unless it was to be left open.</summary>
	public ValueTask DisposeAsync()
	{
		if (!Release() || _leaveOpen || _stream is null)
			return default;
		return _stream.DisposeAsync();
	}

	bool Release()
	{
		if (_disposed)
			return false;
		_disposed = true;
		_batch?.Clear();
		_pin.Dispose();
		return true;
	}

	/// <summary>
	/// Reads a sample of the file at <paramref name="path"/> through the text door and says what its
	/// columns look like: a plan to look at, change and hand to a read, and a report. Nothing is sniffed
	/// in a read, and nothing here reaches one: this is its own read of the file, opened and closed here.
	/// There is no form of this for a stream, which would lose the rows it sampled.
	/// </summary>
	/// <remarks>
	/// <para>
	/// Declaring the locale is the recommended route: a number format and a date order are the plan's to
	/// say, and with them nothing needs inferring. A suggestion is a starting point for a plan to inspect.
	/// </para>
	/// <para>
	/// The width is the header's, or the first record's, and a plan has to name the columns before that
	/// record can be read. So with no header the file is opened twice, once to read the first record: a
	/// path that can only be read once is not for this. The dialect is what the sample is read with, its
	/// tokens included, and a cell it says is empty is blank to the suggestion.
	/// </para>
	/// <para>What it says can surprise, so read the report before the plan:</para>
	/// <list type="bullet">
	/// <item><description>A column of <c>0</c> and <c>1</c> is <see cref="Door.Boolean"/>, the first door asked, and <c>F</c> or <c>No</c> read false.</description></item>
	/// <item><description><c>1.234</c> alone is ambiguous, 1.234 or 1234, and reads as text until a format is declared or a value settles it, which takes in a column of prices at three decimals. Beside <c>2.5</c> it is a decimal, and beside <c>2,5</c> it is 1234.</description></item>
	/// <item><description><c>2,5</c>, <c>0,125</c>, <c>1.234,5</c> and <c>1.234.567</c> read only with <c>,</c> as the decimal, so they are resolved in that notation, and <see cref="SuggestedColumn.Format"/> names it. <c>2,5</c> beside <c>2.5</c> is unresolved.</description></item>
	/// <item><description><c>1,23,456</c>, Indian grouping, reads under neither notation with none declared, because nothing says its culture: alone it is text, resolved, and beside <c>5</c> unresolved. Declaring <c>NumFormat.Invariant with { Grouping = NumGrouping.Indian }</c> for the column makes it a number.</description></item>
	/// <item><description><c>007</c> and an integer past <see cref="long"/> are text, with no note saying why, so as not to lose the zeros or the digits.</description></item>
	/// <item><description>A sheet's date column is text: <see cref="Sheet.Suggest"/> says why.</description></item>
	/// </list>
	/// </remarks>
	/// <param name="path">The file.</param>
	/// <param name="dialect">The declared dialect.</param>
	/// <param name="options">How the sample is taken and read; <see langword="null"/> takes 1,000 rows with nothing declared.</param>
	/// <exception cref="ArgumentException">A format the options declare is one HyperCast refuses: first, before the file is opened, though another replaces it or the sample never reaches its ordinal.</exception>
	/// <exception cref="TabularException">A failure among the sampled rows: there is no knowing what is past it, and a read would stop there too. A failure past them is not met, or only says the source was truncated.</exception>
	public static Suggestion Suggest(string path, Dialect dialect, SuggestOptions? options = null)
	{
		ArgumentNullException.ThrowIfNull(path);
		return Suggest(dialect, options, unnoted => Open(path, unnoted));
	}

	/// <summary><see cref="Suggest(string, Dialect, SuggestOptions?)"/> over text in memory.</summary>
	/// <param name="text">The text.</param>
	/// <param name="dialect">The declared dialect.</param>
	/// <param name="options">How the sample is taken and read; <see langword="null"/> takes 1,000 rows with nothing declared.</param>
	/// <exception cref="ArgumentException">A format the options declare is one HyperCast refuses: first, before anything is read.</exception>
	/// <exception cref="TabularException">A failure among the sampled rows.</exception>
	public static Suggestion Suggest(ReadOnlyMemory<byte> text, Dialect dialect, SuggestOptions? options = null) =>
		Suggest(dialect, options, unnoted => new DelimitedReader(text, unnoted));

	static Suggestion Suggest(Dialect dialect, SuggestOptions? options, Func<Dialect, DelimitedReader> open)
	{
		options ??= SuggestOptions.Default;
		options.CheckFormats();
		var limit = options.RowsWithin(DefaultBatchRows);
		var unnoted = dialect with { Notes = false };
		var width = -1;
		if (!dialect.HasHeader)
		{
			// A headerless source's width is its first record's, which a read says through a plan of
			// one column.
			using var first = open(unnoted);
			first.Bind([Column.Text(0)], 1);
			first.Read();
			width = first.ColumnCount ?? 0;
		}
		using var reader = open(unnoted);
		if (width < 0)
			width = reader.Header!.Count;
		// One row more than the sample, so that a row past it costs no second batch to find, in as many
		// batches as the core says that takes.
		var sample = Sample.Of(reader, plan => reader.Bind(plan, Columns.SampleRows(plan, limit + 1, SizeFlags(unnoted))), width, limit, sheet: false);
		return sample.Judge(reader.Header, options);
	}
}

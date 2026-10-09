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
/// </remarks>
[SkipLocalsInit]
public sealed unsafe class DelimitedReader : IDisposable, IAsyncDisposable, IBatchSource
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
	/// <summary>The plan's columns and the batch they are read into: <see langword="null"/> until a plan is bound.</summary>
	Columns? _columns;
	Batch? _batch;
	/// <summary>Cell-table entries one row takes: one per source column the plan reaches, and one more.</summary>
	int _perRow;

	// Pinned, as the column arrays are: the core is given their addresses.
	readonly State[] _state;
	Native.RawSpan[] _cells = [];
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

	/// <summary>The table the header's names are located in, while the header is being read.</summary>
	Native.RawSpan[]? _names;
	Header? _header;
	TabularException? _failure;
	bool _disposed;

	/// <summary>
	/// Reads UTF-8 delimited text from a stream, the header first: the reader is open, its
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

	/// <summary>Reads UTF-8 delimited text from a stream through <paramref name="plan"/>.</summary>
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
		: this(dialect, new Columns(plan, batchRows), utf8, bufferBytes, leaveOpen)
	{
		if (dialect.HasHeader)
			ReadHeader();
	}

	/// <summary>
	/// Reads UTF-8 delimited text already in memory, in place — nothing is copied — the header
	/// first, leaving the plan to <see cref="Bind"/>.
	/// </summary>
	/// <param name="utf8">The text. Pinned for the reader's lifetime.</param>
	/// <param name="dialect">The declared dialect.</param>
	/// <exception cref="ArgumentException">The dialect's separator cannot be honoured.</exception>
	/// <exception cref="TabularException">The header record is structurally broken.</exception>
	public DelimitedReader(ReadOnlyMemory<byte> utf8, Dialect dialect)
		: this(dialect, null)
	{
		_pin = utf8.Pin();
		_memory = (byte*)_pin.Pointer;
		_end = utf8.Length;
		_eof = true;
		if (dialect.HasHeader)
			ReadHeader();
	}

	/// <summary>Reads UTF-8 delimited text already in memory, in place — nothing is copied — through <paramref name="plan"/>.</summary>
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
		: this(dialect, new Columns(plan, batchRows))
	{
		_pin = utf8.Pin();
		_memory = (byte*)_pin.Pointer;
		_end = utf8.Length;
		_eof = true;
		if (dialect.HasHeader)
			ReadHeader();
	}

	/// <summary>Opens a file of UTF-8 delimited text and reads its header, leaving the plan to <see cref="Bind"/>.</summary>
	/// <param name="path">The file.</param>
	/// <param name="dialect">The declared dialect.</param>
	/// <param name="bufferBytes">The initial read buffer.</param>
	public static DelimitedReader Open(string path, Dialect dialect, int bufferBytes = DefaultBufferBytes)
	{
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

	/// <summary>Opens a file of UTF-8 delimited text, read through <paramref name="plan"/>.</summary>
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
	/// Opens a stream of UTF-8 delimited text and awaits its header, leaving the plan to
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
	/// Opens a stream of UTF-8 delimited text through <paramref name="plan"/> and awaits its
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
	/// <exception cref="ArgumentException">The dialect's separator or a column's numeric format cannot be honoured.</exception>
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
			new DelimitedReader(dialect, new Columns(plan, batchRows), utf8, bufferBytes, leaveOpen), cancellationToken);
	}

	static async ValueTask<DelimitedReader> HeaderAsync(DelimitedReader reader, CancellationToken cancellationToken)
	{
		if (reader._dialect.HasHeader)
		{
			while (!reader.HeaderStep())
				await reader.RefillAsync(cancellationToken).ConfigureAwait(false);
		}
		return reader;
	}

	/// <summary>A reader of a stream, its header not yet read.</summary>
	DelimitedReader(Dialect dialect, Columns? columns, Stream utf8, int bufferBytes, bool leaveOpen)
		: this(dialect, columns)
	{
		ArgumentNullException.ThrowIfNull(utf8);
		ArgumentOutOfRangeException.ThrowIfNegativeOrZero(bufferBytes);
		_stream = utf8;
		_leaveOpen = leaveOpen;
		_buffer = GC.AllocateUninitializedArray<byte>(Math.Min(bufferBytes, MaxRowBytes), pinned: true);
	}

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

	/// <summary>Sizes everything a batch is read into by the plan's columns.</summary>
	void Attach(Columns columns)
	{
		_perRow = columns.Width + 1;
		_cells = GC.AllocateUninitializedArray<Native.RawSpan>(checked(_perRow * columns.BatchRows), pinned: true);
		_batch = new Batch(columns, workbook: false);
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
	/// <exception cref="InvalidOperationException">The reader already has a plan.</exception>
	/// <exception cref="ArgumentException">A column's numeric format cannot be honoured; the reader is left without a plan.</exception>
	public void Bind(ReadOnlySpan<Column> plan, int batchRows = DefaultBatchRows)
	{
		ObjectDisposedException.ThrowIf(_disposed, this);
		if (_columns is not null)
			throw new InvalidOperationException("The reader already has a plan; a plan is bound once.");
		Attach(new Columns(plan, batchRows));
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
		return (_stream is null ? _memory : Address(_buffer)) + _start;
	}

	/// <summary>Puts the unfinished record at the front of the buffer, and makes room behind it.</summary>
	void Compact()
	{
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
		Compact();
		Landed(_stream!.Read(_buffer, _end, _buffer.Length - _end));
	}

	/// <summary>
	/// <see cref="Refill"/>, awaiting the stream. A read that is cancelled throws before it
	/// counts anything, so the reader is as it was: what was buffered stays buffered.
	/// </summary>
	async ValueTask RefillAsync(CancellationToken cancellationToken)
	{
		cancellationToken.ThrowIfCancellationRequested();
		Compact();
		Landed(await _stream!.ReadAsync(_buffer.AsMemory(_end), cancellationToken).ConfigureAwait(false));
	}

	TabularException Structural(in Native.RawFailure failure) => _failure = TabularException.From(failure);

	void ReadHeader()
	{
		while (!HeaderStep())
			Refill();
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
			// Only the cells and the arena are the fill's; the rest stays empty.
			var buffers = new Native.RawBuffers
			{
				Arena = Address(_arena),
				ArenaCap = (nuint)_arena.Length,
				Cells = Address(_cells),
				CellsCap = (nuint)_cells.Length,
			};
			var code = Native.hypertabular_delimited_fill(
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
						_cramped = rows < batchRows && (long)filled.ArenaUsed * 2 >= _arena.Length;
						_batch!.Fill(rows, _cells, _perRow, window, _arena);
						return Step.Rows;
					}
					if (last && (consumed == length || consumed == 0))
						return Step.End;
					if (consumed == 0)
						return Step.Input;
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

	/// <summary>
	/// Reads the next batch: up to the reader's batch size of rows, or <see langword="null"/>
	/// once the input is exhausted. The batch is valid until the next call.
	/// </summary>
	/// <exception cref="TabularException">
	/// The input is structurally broken. Raised after every intact row before the break has
	/// been delivered, and again on every later call.
	/// </exception>
	/// <exception cref="InvalidOperationException">No plan has been bound. Not final: bind one and read.</exception>
	public Batch? Read()
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
	/// <exception cref="InvalidOperationException">No plan has been bound. Not final: bind one and read.</exception>
	/// <exception cref="OperationCanceledException">The token was cancelled.</exception>
	/// <remarks>One read at a time: the next is not started until this one has completed.</remarks>
	public ValueTask<Batch?> ReadAsync(CancellationToken cancellationToken = default)
	{
		if (cancellationToken.IsCancellationRequested)
			return ValueTask.FromCanceled<Batch?>(cancellationToken);
		try
		{
			Begin();
			switch (FillStep())
			{
				case Step.Rows:
					return new(_batch);
				case Step.End:
					return new((Batch?)null);
			}
		}
		catch (Exception e)
		{
			return ValueTask.FromException<Batch?>(e);
		}
		return ReadRestAsync(cancellationToken);
	}

	async ValueTask<Batch?> ReadRestAsync(CancellationToken cancellationToken)
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

	/// <summary>
	/// Every row left, batch by batch: <c>foreach (var row in reader.Rows())</c> calls
	/// <see cref="Read"/> whenever a batch runs out. Each <see cref="Row"/> is a view of the
	/// current batch and is over when the loop moves past it; do not keep one.
	/// </summary>
	public RowSequence Rows() => new(this);

	/// <summary>Releases the pinned memory, and the stream unless it was to be left open.</summary>
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
}

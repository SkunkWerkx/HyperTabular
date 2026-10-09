namespace HyperTabular;

/// <summary>
/// A forward-only read of one sheet of a <see cref="Workbook"/>, a batch at a time, through
/// a plan — the same <see cref="Batch"/> delimited text is read into.
/// </summary>
/// <remarks>
/// A sheet has its own buffers and its own copy of the workbook's read state, so several can
/// be read at once; each reads from the workbook, which has to outlive it. A typed workbook
/// cell is converted directly by its door — a stored <c>42.0</c> never passes through text to
/// become an <see cref="int"/> — and a text cell goes through the door as delimited text
/// would. A sheet started without a plan has read its header, and is given its plan by
/// <see cref="Bind"/>. Not thread-safe.
/// </remarks>
public sealed unsafe class Sheet : IBatchSource
{
	readonly Workbook _book;
	readonly SheetOptions _options;
	/// <summary>The plan's columns and the batch they are read into: <see langword="null"/> until a plan is bound.</summary>
	Columns? _columns;
	Batch? _batch;
	readonly ulong[] _state;
	readonly Workbook.Scratch _scratch;
	/// <summary>Cell-table entries to a row: one per plan column, and one more.</summary>
	int _perRow;
	readonly Header? _header;
	/// <summary>A failure met with rows before it: those went out first, and this is next.</summary>
	TabularException? _pending;
	TabularException? _failure;

	/// <summary>Starts a read of a sheet, through <paramref name="plan"/> when <paramref name="bound"/>, or with the plan left to <see cref="Bind"/>.</summary>
	internal Sheet(Workbook book, SheetInfo info, SheetOptions options, ReadOnlySpan<Column> plan, bool bound)
	{
		ArgumentOutOfRangeException.ThrowIfNegativeOrZero(options.BatchRows, nameof(options));
		_book = book;
		_options = options;
		// A copy of the opened state, which the core allows, so that this sheet's read is its own.
		_state = GC.AllocateArray<ulong>(book.State.Length, pinned: true);
		book.State.AsSpan().CopyTo(_state);
		if (bound)
		{
			var columns = new Columns(plan, options.BatchRows);
			_perRow = columns.Plan.Length + 1;
			_scratch = Workbook.Stingy
				? new Workbook.Scratch(1, 1, 1, columns.Width)
				: new Workbook.Scratch(0, 4096, checked(options.BatchRows * _perRow), columns.Width);
			_columns = columns;
			_batch = new Batch(columns, workbook: true);
		}
		else
		{
			// The header row's cells are kept in the row's slots as well as named in the cell
			// table — the slots are what a row the sheet repeats (an ODS
			// number-rows-repeated) is delivered again from. A plan says how many slots it
			// reads; without one there is a slot for every entry the cell table has room for,
			// and the slots grow with it (Scratch.Drive) while the header is read.
			_scratch = Workbook.Stingy
				? new Workbook.Scratch(1, 1, 1, 1)
				: new Workbook.Scratch(0, 4096, 64, 64);
		}

		Native.RawFilled filled;
		int code;
		fixed (byte* part = info.Part)
			code = Native.hypertabular_workbook_sheet(
				Columns.Address(_state), book.Container, book.Length, part, (nuint)info.Part.Length, info.Index,
				options.HasHeader ? 1u : 0u, options.SkipEmptyRows ? 1u : 0u, &filled);
		Workbook.Settle((code, filled));
		if (options.HasHeader)
			_header = ReadHeader();
	}

	Header ReadHeader()
	{
		var filled = Workbook.Settle(_scratch.Drive(_book, _state, null, Workbook.Call.Header, rowFollowsCells: _columns is null));
		var header = new Header.Builder((int)filled.Rows);
		for (var index = 0; index < (int)filled.Rows; index++)
		{
			var name = _scratch.Cells[index];
			header.Add(name.Flagged
				? _scratch.Arena.AsSpan((int)name.Offset, name.Length)
				: _book.Strings.AsSpan((int)name.Offset, name.Length));
		}
		return header.Build();
	}

	/// <summary>
	/// Declares the plan a sheet started without one reads through — once, before the first
	/// read, and usually after <see cref="Header"/> has said where each column is. The column
	/// arrays and the batch are sized here, by the plan and the options' batch size.
	/// </summary>
	/// <param name="plan">The output columns, in output order.</param>
	/// <exception cref="InvalidOperationException">The sheet already has a plan.</exception>
	/// <exception cref="ArgumentException">A column's numeric format cannot be honoured; the sheet is left without a plan.</exception>
	/// <exception cref="ObjectDisposedException">The workbook has been disposed.</exception>
	public void Bind(ReadOnlySpan<Column> plan)
	{
		ObjectDisposedException.ThrowIf(_book.Disposed, _book);
		if (_columns is not null)
			throw new InvalidOperationException("The sheet already has a plan; a plan is bound once.");
		var columns = new Columns(plan, _options.BatchRows);
		_perRow = columns.Plan.Length + 1;
		// Grown, never replaced: the cell table only to save the first fill a call, and the
		// row's slots because what they hold is a header row the sheet may yet repeat.
		var cells = checked(_options.BatchRows * _perRow);
		if (!Workbook.Stingy && _scratch.Cells.Length < cells)
			_scratch.Cells = Columns.Grow(_scratch.Cells, cells);
		if (_scratch.Row.Length < columns.Width)
			_scratch.Row = Columns.Grow(_scratch.Row, columns.Width);
		_batch = new Batch(columns, workbook: true);
		_columns = columns;
	}

	/// <summary>Whether a plan has been bound: always, for a sheet started with one.</summary>
	public bool IsBound => _columns is not null;

	/// <summary>The options the sheet is read with.</summary>
	public SheetOptions Options => _options;

	/// <summary>The plan the sheet is read through: column <c>i</c> of every batch is <c>Plan[i]</c>. Empty until one is bound.</summary>
	public IReadOnlyList<Column> Plan => _columns?.Plan ?? [];

	/// <summary>
	/// The header row's names — a typed cell said the way the text door says it — or
	/// <see langword="null"/> if the sheet was opened without a header. A header with no names
	/// is a sheet with no rows.
	/// </summary>
	public Header? Header => _header;

	/// <summary>
	/// Reads the next batch: up to the sheet's batch size of rows, or <see langword="null"/>
	/// when the sheet has no more. The batch is valid until the next call.
	/// </summary>
	/// <exception cref="TabularException">
	/// The sheet is structurally broken. Raised after every intact row before the break has
	/// been delivered, and again on every later call.
	/// </exception>
	/// <exception cref="InvalidOperationException">No plan has been bound. Not final: bind one and read.</exception>
	/// <exception cref="ObjectDisposedException">The workbook has been disposed.</exception>
	public Batch? Read()
	{
		ObjectDisposedException.ThrowIf(_book.Disposed, _book);
		if (_batch is null)
			throw new InvalidOperationException("The sheet has no plan yet: Bind one before reading.");
		_batch.Clear();
		if (_pending is not null)
			(_failure, _pending) = (_pending, null);
		if (_failure is not null)
			throw _failure;
		var (code, filled) = _scratch.Drive(_book, _state, _columns, Workbook.Call.Fill);
		var rows = (int)filled.Rows;
		if (code != Native.Ok)
		{
			var failure = code == Native.ErrStructure
				? TabularException.From(filled.Failure)
				: throw new InvalidOperationException(Workbook.ContractViolation);
			if (rows == 0)
				throw _failure = failure;
			_pending = failure;
		}
		if (rows == 0)
			return null;
		return _batch.Fill(rows, _scratch.Cells, _perRow, Columns.Address(_book.Strings), _scratch.Arena);
	}

	/// <summary>
	/// Every row left, batch by batch: <c>foreach (var row in sheet.Rows())</c> calls
	/// <see cref="Read"/> whenever a batch runs out. Each <see cref="Row"/> is a view of the
	/// current batch and is over when the loop moves past it; do not keep one.
	/// </summary>
	public RowSequence Rows() => new(this);
}

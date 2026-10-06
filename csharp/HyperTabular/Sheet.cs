using System.Text;

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
/// would. Not thread-safe.
/// </remarks>
public sealed unsafe class Sheet
{
	readonly Workbook _book;
	readonly SheetOptions _options;
	readonly Columns _columns;
	readonly Batch _batch;
	readonly ulong[] _state;
	readonly Workbook.Scratch _scratch;
	/// <summary>Cell-table entries to a row: one per plan column, and one more.</summary>
	readonly int _perRow;
	readonly string[]? _header;
	/// <summary>A failure met with rows before it: those went out first, and this is next.</summary>
	TabularException? _pending;
	TabularException? _failure;

	internal Sheet(Workbook book, SheetInfo info, SheetOptions options, ReadOnlySpan<Column> plan)
	{
		ArgumentOutOfRangeException.ThrowIfNegativeOrZero(options.BatchRows, nameof(options));
		_book = book;
		_options = options;
		_columns = new Columns(plan, options.BatchRows);
		_batch = new Batch(_columns, workbook: true);
		_perRow = _columns.Plan.Length + 1;
		// A copy of the opened state, which the core allows, so that this sheet's read is its own.
		_state = GC.AllocateArray<ulong>(book.State.Length, pinned: true);
		book.State.AsSpan().CopyTo(_state);
		_scratch = Workbook.Stingy
			? new Workbook.Scratch(1, 1, 1, _columns.Width)
			: new Workbook.Scratch(0, 4096, checked(options.BatchRows * _perRow), _columns.Width);

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

	string[] ReadHeader()
	{
		var filled = Workbook.Settle(_scratch.Drive(_book, _state, null, Workbook.Call.Header));
		var header = new string[(int)filled.Rows];
		for (var index = 0; index < header.Length; index++)
		{
			var name = _scratch.Cells[index];
			var bytes = name.Flagged
				? _scratch.Arena.AsSpan((int)name.Offset, name.Length)
				: _book.Strings.AsSpan((int)name.Offset, name.Length);
			header[index] = Encoding.UTF8.GetString(bytes);
		}
		return header;
	}

	/// <summary>The options the sheet is read with.</summary>
	public SheetOptions Options => _options;

	/// <summary>The plan the sheet is read through: column <c>i</c> of every batch is <c>Plan[i]</c>.</summary>
	public IReadOnlyList<Column> Plan => _columns.Plan;

	/// <summary>
	/// The header row's names — a typed cell said the way the text door says it — or
	/// <see langword="null"/> if the sheet was opened without a header. A header with no names
	/// is a sheet with no rows.
	/// </summary>
	public IReadOnlyList<string>? Header => _header;

	/// <summary>
	/// Reads the next batch: up to the sheet's batch size of rows, or <see langword="null"/>
	/// when the sheet has no more. The batch is valid until the next call.
	/// </summary>
	/// <exception cref="TabularException">
	/// The sheet is structurally broken. Raised after every intact row before the break has
	/// been delivered, and again on every later call.
	/// </exception>
	/// <exception cref="ObjectDisposedException">The workbook has been disposed.</exception>
	public Batch? Read()
	{
		ObjectDisposedException.ThrowIf(_book.Disposed, _book);
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
}

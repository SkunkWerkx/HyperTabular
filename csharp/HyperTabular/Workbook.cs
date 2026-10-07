using System.Buffers;
using System.Text;
using HyperCast;
using HyperCast.Interop;

namespace HyperTabular;

/// <summary>Which kind of workbook a container holds — told by what is in it, never by its name.</summary>
public enum WorkbookFormat
{
	/// <summary>Sentinel CLR default — never produced.</summary>
	Unspecified = 0,
	/// <summary>Office Open XML (<c>.xlsx</c>).</summary>
	Xlsx = 1,
	/// <summary>OpenDocument (<c>.ods</c>).</summary>
	Ods = 2
}

/// <summary>One sheet of a workbook, as its listing names it.</summary>
public sealed class SheetInfo
{
	internal SheetInfo(string name, bool hidden, byte[] part, uint index)
	{
		Name = name;
		Hidden = hidden;
		Part = part;
		Index = index;
	}

	/// <summary>The sheet's name. (Bytes that are not UTF-8 are replaced.)</summary>
	public string Name { get; }

	/// <summary>Whether the workbook hides the sheet. A hidden sheet reads like any other.</summary>
	public bool Hidden { get; }

	/// <summary>What the core is given to position on the sheet: the XLSX part that holds it.</summary>
	internal byte[] Part { get; }

	/// <summary>The sheet's place among an ODS document's tables.</summary>
	internal uint Index { get; }

	/// <inheritdoc/>
	public override string ToString() => Hidden ? $"{Name} (hidden)" : Name;
}

/// <summary>How a sheet is read.</summary>
/// <param name="HasHeader">Whether the first row delivered is the header rather than data.</param>
/// <param name="SkipEmptyRows">
/// Whether rows with no cell in them — and the rows a gap in the row numbers stands for — are
/// left out rather than delivered as rows of empty cells.
/// </param>
/// <param name="BatchRows">The most rows a batch holds.</param>
public readonly record struct SheetOptions(bool HasHeader = true, bool SkipEmptyRows = true, int BatchRows = 4096)
{
	/// <summary>A header, empty rows skipped, 4096 rows a batch. (<c>default(SheetOptions)</c> is not this: its batch size is zero.)</summary>
	public static readonly SheetOptions Default = new(true, true, 4096);
}

/// <summary>
/// A workbook — XLSX or ODS — opened for reading through a plan into the batches delimited
/// text is read into.
/// </summary>
/// <remarks>
/// <para>
/// A workbook holds its container's bytes, pinned, and what every sheet reads its cells
/// against: the shared strings and the number-format kind of each cell format, loaded once
/// when it is opened. <see cref="Sheet(int, SheetOptions, ReadOnlySpan{Column})"/> starts a
/// forward-only read of one sheet; several can be open at once, each with its own buffers,
/// all reading from the one workbook, which has to outlive them.
/// </para>
/// <para>
/// The core owns no memory, as for delimited text: every buffer it works in is this
/// binding's, and when a call says one is too small it is grown — keeping what it held,
/// since a workbook part is a compressed stream the read cannot back up through — and the
/// call made again. Not thread-safe.
/// </para>
/// </remarks>
public sealed unsafe class Workbook : IDisposable
{
	/// <summary>The window a part is inflated through starts at the size the core requires.</summary>
	const int WindowMin = 64 * 1024;

	/// <summary>
	/// For the tests: every buffer a workbook call works in starts with room for one element
	/// and no shared-strings bound is asked for, so every call that can stop and resume does —
	/// the grow-and-keep path exercised mid-part.
	/// </summary>
	[ThreadStatic]
	internal static bool Stingy;

	/// <summary>For the tests: how many times the window, the arena and the cell table were grown on this thread.</summary>
	[ThreadStatic]
	internal static (int Window, int Arena, int Cells) Grown;

	internal const string ContractViolation =
		"libhypertabular reported a contract violation — a binding bug, please report it.";

	MemoryHandle _pin;
	readonly byte* _container;
	readonly nuint _length;
	/// <summary>The state as opening left it: the template every sheet's own state is copied from.</summary>
	readonly ulong[] _state;
	readonly SheetInfo[] _sheets;
	readonly byte[] _strings;
	readonly Native.RawSpan[] _table;
	readonly byte[] _kinds;
	bool _disposed;

	/// <summary>Opens the workbook in memory the caller holds. Pinned for the workbook's lifetime; nothing is copied.</summary>
	/// <param name="container">The workbook's bytes.</param>
	/// <exception cref="TabularException">The bytes are not a workbook this reader can read.</exception>
	public Workbook(ReadOnlyMemory<byte> container)
	{
		_pin = container.Pin();
		_container = (byte*)_pin.Pointer;
		_length = (nuint)container.Length;
		try
		{
			var size = (int)Native.hypertabular_workbook_state_size();
			_state = GC.AllocateArray<ulong>((size + 7) / 8, pinned: true);
			var scratch = Stingy ? new Scratch(1, 1, 1) : new Scratch(WindowMin, 1024, 64);
			var opened = Open(scratch);
			Format = opened.Format == 1 ? WorkbookFormat.Xlsx : WorkbookFormat.Ods;
			DateSystem = Abi.ExcelEpochFrom(opened.Epoch) ?? throw new InvalidOperationException(ContractViolation);

			var filled = Settle(scratch.Drive(this, _state, null, Call.Sheets));
			_sheets = new SheetInfo[(int)filled.Rows];
			for (var index = 0; index < _sheets.Length; index++)
			{
				var name = scratch.Cells[index * 3];
				var part = scratch.Cells[index * 3 + 1];
				var last = scratch.Cells[index * 3 + 2];
				_sheets[index] = new SheetInfo(
					Encoding.UTF8.GetString(scratch.Arena.AsSpan((int)name.Offset, name.Length)),
					(last.Offset & 1) != 0,
					scratch.Arena.AsSpan((int)part.Offset, part.Length).ToArray(),
					last.Len);
			}

			// The shared strings take no more room than their part inflates to; asking for it
			// once saves growing into it.
			var bound = (long)Math.Min(opened.StringsBytes, 1UL << 28);
			if (!Stingy && scratch.Arena.Length < bound)
				scratch.Arena = Columns.Grow(scratch.Arena, bound);
			filled = Settle(scratch.Drive(this, _state, null, Call.Strings));
			_strings = Pinned(scratch.Arena.AsSpan(0, (int)filled.ArenaUsed));
			_table = Pinned(scratch.Cells.AsSpan(0, (int)filled.Rows));

			filled = Settle(scratch.Drive(this, _state, null, Call.Styles));
			_kinds = Pinned(scratch.Arena.AsSpan(0, (int)filled.Rows));
		}
		catch
		{
			_pin.Dispose();
			throw;
		}
	}

	/// <summary>Reads the file at <paramref name="path"/> into memory and opens it.</summary>
	/// <exception cref="TabularException">The file is not a workbook this reader can read.</exception>
	public static Workbook Open(string path) => new(File.ReadAllBytes(path));

	static T[] Pinned<T>(ReadOnlySpan<T> data) where T : unmanaged
	{
		var copy = GC.AllocateUninitializedArray<T>(data.Length, pinned: true);
		data.CopyTo(copy);
		return copy;
	}

	/// <summary>Opening starts over when it is refused, and reports through its own shape.</summary>
	Native.RawOpened Open(Scratch scratch)
	{
		while (true)
		{
			Native.RawOpened opened;
			var buffers = scratch.Buffers(null);
			var code = Native.hypertabular_workbook_open(Columns.Address(_state), _container, _length, &buffers, &opened);
			switch (code)
			{
				case Native.Ok:
					return opened;
				case Native.ErrWindow:
					scratch.Window = Columns.Grow(scratch.Window, (long)opened.Needed);
					break;
				case Native.ErrArena:
					scratch.Arena = Columns.Grow(scratch.Arena, (long)opened.Needed);
					break;
				case Native.ErrStructure:
					throw TabularException.From(opened.Failure);
				default:
					throw new InvalidOperationException(ContractViolation);
			}
		}
	}

	/// <summary>What a call's answer means: done, or a structural failure; anything else is this binding's bug.</summary>
	internal static Native.RawFilled Settle((int Code, Native.RawFilled Filled) answer) =>
		answer.Code switch
		{
			Native.Ok => answer.Filled,
			Native.ErrStructure => throw TabularException.From(answer.Filled.Failure),
			_ => throw new InvalidOperationException(ContractViolation),
		};

	/// <summary>Which kind of workbook this is.</summary>
	public WorkbookFormat Format { get; }

	/// <summary>The date system the workbook's serials count in: what a date-formatted number is read by.</summary>
	public ExcelEpoch DateSystem { get; }

	/// <summary>The workbook's sheets, in its own order. Sheets that hold no cells (chart sheets, macro sheets) are not among them.</summary>
	public IReadOnlyList<SheetInfo> Sheets => _sheets;

	/// <summary>Starts a read of the sheet at <paramref name="index"/> of <see cref="Sheets"/> through <paramref name="plan"/>.</summary>
	/// <exception cref="ArgumentOutOfRangeException">The workbook has no sheet at that index.</exception>
	/// <exception cref="ArgumentException">A column's numeric format cannot be honoured.</exception>
	/// <exception cref="TabularException">The sheet, or its header row, is structurally broken.</exception>
	public Sheet Sheet(int index, SheetOptions options, ReadOnlySpan<Column> plan)
	{
		ObjectDisposedException.ThrowIf(_disposed, this);
		ArgumentOutOfRangeException.ThrowIfGreaterThanOrEqual((uint)index, (uint)_sheets.Length, nameof(index));
		return new Sheet(this, _sheets[index], options, plan);
	}

	/// <summary>Starts a read of the first sheet named <paramref name="name"/> through <paramref name="plan"/>.</summary>
	/// <exception cref="KeyNotFoundException">The workbook has no sheet by that name.</exception>
	/// <exception cref="ArgumentException">A column's numeric format cannot be honoured.</exception>
	/// <exception cref="TabularException">The sheet, or its header row, is structurally broken.</exception>
	public Sheet Sheet(string name, SheetOptions options, ReadOnlySpan<Column> plan)
	{
		ObjectDisposedException.ThrowIf(_disposed, this);
		ArgumentNullException.ThrowIfNull(name);
		foreach (var sheet in _sheets)
			if (sheet.Name == name)
				return new Sheet(this, sheet, options, plan);
		throw new KeyNotFoundException($"The workbook has no sheet named \"{name}\".");
	}

	internal byte* Container => _container;
	internal nuint Length => _length;
	internal ulong[] State => _state;
	internal byte[] Strings => _strings;
	internal bool Disposed => _disposed;

	/// <summary>The workbook's tables, as a read of one of its sheets is handed them.</summary>
	internal Native.RawBuffers Tables(Native.RawBuffers buffers)
	{
		buffers.Strings = Columns.Address(_strings);
		buffers.StringsLen = (nuint)_strings.Length;
		buffers.Table = Columns.Address(_table);
		buffers.TableLen = (nuint)_table.Length;
		buffers.Kinds = Columns.Address(_kinds);
		buffers.KindsLen = (nuint)_kinds.Length;
		return buffers;
	}

	/// <summary>Releases the container's pin. Sheets read from the workbook are over with it.</summary>
	public void Dispose()
	{
		if (_disposed)
			return;
		_disposed = true;
		_pin.Dispose();
	}

	/// <summary>The workbook calls that take the shared buffers and may ask to have one grown.</summary>
	internal enum Call
	{
		Sheets,
		Strings,
		Styles,
		Header,
		Fill,
	}

	/// <summary>The buffers a call to the core may ask to have grown: one set for the workbook while it opens, one for each sheet.</summary>
	internal sealed class Scratch(int window, int arena, int cells, int row = 0)
	{
		public byte[] Window = GC.AllocateUninitializedArray<byte>(window, pinned: true);
		public byte[] Arena = GC.AllocateUninitializedArray<byte>(arena, pinned: true);
		public Native.RawSpan[] Cells = GC.AllocateUninitializedArray<Native.RawSpan>(cells, pinned: true);
		public readonly Native.RawSlot[] Row = GC.AllocateUninitializedArray<Native.RawSlot>(row, pinned: true);

		/// <summary>This scratch as the core takes it, with the workbook's tables when a sheet is being read.</summary>
		public Native.RawBuffers Buffers(Workbook? book)
		{
			var buffers = new Native.RawBuffers
			{
				Window = Columns.Address(Window),
				WindowCap = (nuint)Window.Length,
				Arena = Columns.Address(Arena),
				ArenaCap = (nuint)Arena.Length,
				Cells = Columns.Address(Cells),
				CellsCap = (nuint)Cells.Length,
				Row = Columns.Address(Row),
				RowCap = (nuint)Row.Length,
			};
			return book is null ? buffers : book.Tables(buffers);
		}

		/// <summary>
		/// Makes <paramref name="call"/> until it stops asking for room, growing the buffer it
		/// names each time — with what the buffer held kept, which is what lets the core go
		/// on from where it stopped. Returns the code it ended on and what it reported.
		/// </summary>
		public (int Code, Native.RawFilled Filled) Drive(Workbook book, ulong[] state, Columns? columns, Call call)
		{
			var tables = call is Call.Header or Call.Fill;
			while (true)
			{
				Native.RawFilled filled;
				var buffers = Buffers(tables ? book : null);
				var code = call switch
				{
					Call.Sheets => Native.hypertabular_workbook_sheets(Columns.Address(state), book.Container, book.Length, &buffers, &filled),
					Call.Strings => Native.hypertabular_workbook_strings(Columns.Address(state), book.Container, book.Length, &buffers, &filled),
					Call.Styles => Native.hypertabular_workbook_styles(Columns.Address(state), book.Container, book.Length, &buffers, &filled),
					Call.Header => Native.hypertabular_workbook_header(Columns.Address(state), book.Container, book.Length, &buffers, &filled),
					_ => Native.hypertabular_workbook_fill(Columns.Address(state), book.Container, book.Length,
						Columns.Address(columns!.Specs), Columns.Address(columns.Buffers), (nuint)columns.Plan.Length,
						(nuint)columns.BatchRows, &buffers, &filled),
				};
				switch (code)
				{
					case Native.ErrWindow:
						Window = Columns.Grow(Window, (long)filled.Needed);
						Grown.Window++;
						break;
					case Native.ErrArena:
						Arena = Columns.Grow(Arena, (long)filled.Needed);
						Grown.Arena++;
						break;
					case Native.ErrCells:
						Cells = Columns.Grow(Cells, (long)filled.Needed);
						Grown.Cells++;
						break;
					default:
						return (code, filled);
				}
			}
		}
	}
}

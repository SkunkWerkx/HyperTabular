using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using HyperCast.Interop;

namespace HyperTabular;

/// <summary>
/// The native core's C ABI: <c>libhypertabular</c>'s exports and the <c>#[repr(C)]</c>
/// shapes that cross them (<c>rust/src/kernel/abi.rs</c>). Every pointer is this assembly's
/// own memory for the length of the call; the core keeps nothing.
/// </summary>
/// <remarks>
/// Every export is declared three times, as HyperCast's <c>Cast</c> declares its own: against
/// <c>"hypertabular"</c> (the shared library, loaded on every desktop and server platform),
/// against <c>"*"</c> (the current module, the only thing that resolves a core statically
/// linked into a browser-wasm app), and against <c>"__Internal"</c> (the app's own executable,
/// which is where .NET for iOS and Mac Catalyst link a static library). The wrapper of each
/// picks one with <see cref="OperatingSystem.IsBrowser"/> and <see cref="OperatingSystem.IsIOS"/>
/// (true on Mac Catalyst as well), which the trimmer folds per publish target, so a trimmed
/// app keeps only the branch it can reach.
/// </remarks>
static unsafe partial class Native
{
	internal const int Ok = 0;
	internal const int ErrContract = -1;
	internal const int ErrStructure = -2;
	internal const int ErrArena = -3;
	internal const int ErrCells = -4;
	internal const int ErrWindow = -5;
	internal const int ErrBatch = -6;

	/// <summary>The flag in the top bit of a span's length; its meaning is the field's.</summary>
	internal const uint SpanFlag = 1u << 31;

	[StructLayout(LayoutKind.Sequential)]
	internal struct RawSpan
	{
		public uint Offset;
		public uint Len;

		public readonly int Length => (int)(Len & ~SpanFlag);
		public readonly bool Flagged => (Len & SpanFlag) != 0;
	}

	[StructLayout(LayoutKind.Sequential)]
	internal struct RawColumnSpec
	{
		public uint Ordinal;
		public uint Door;
		public uint Param;
		public RawNumFormat Format;
	}

	[StructLayout(LayoutKind.Sequential)]
	internal struct RawColumnBuffer
	{
		public void* Values;
		public CellVerdict* Verdicts;
	}

	[StructLayout(LayoutKind.Sequential)]
	internal struct RawFailure
	{
		public uint Code;
		public uint Line;
		public ulong Record;
		public ulong Byte;
		public uint Expected;
		public uint Found;
	}

	[StructLayout(LayoutKind.Sequential)]
	internal struct RawFilled
	{
		public ulong Rows;
		public ulong Consumed;
		public ulong ArenaUsed;
		public ulong Needed;
		public RawFailure Failure;
	}

	[StructLayout(LayoutKind.Sequential)]
	internal struct RawDialect
	{
		public byte Separator;
		public byte Quoting;
		public byte SkipBlankLines;
		public byte Engine;
	}

	/// <summary>
	/// The first 64 bytes of the core's state block, which it defines and this assembly only keeps.
	/// The block's own size is <see cref="hypertabular_delimited_state_size"/>'s to say.
	/// </summary>
	[StructLayout(LayoutKind.Sequential, Size = 64)]
	internal struct State
	{
		ulong _first;
	}

	/// <summary>One cell of the row a workbook read assembles: the core's scratch, never read here.</summary>
	[StructLayout(LayoutKind.Sequential)]
	internal struct RawSlot
	{
		public uint Tag;
		public uint Aux;
		public ulong Bits;
	}

	/// <summary>
	/// The memory a workbook call works in, handed over again on every call, and the delimited
	/// fill's cells and arena: Mono's interpreter, which runs .NET in the browser and in an iOS
	/// debug build, passes no more than twelve integer arguments to a native function, so the
	/// fill takes its two buffers here rather than as four arguments of their own.
	/// </summary>
	[StructLayout(LayoutKind.Sequential)]
	internal struct RawBuffers
	{
		public byte* Window;
		public nuint WindowCap;
		public byte* Arena;
		public nuint ArenaCap;
		public RawSpan* Cells;
		public nuint CellsCap;
		public RawSlot* Row;
		public nuint RowCap;
		public byte* Strings;
		public nuint StringsLen;
		public RawSpan* Table;
		public nuint TableLen;
		public byte* Kinds;
		public nuint KindsLen;
	}

	/// <summary>What opening a workbook found.</summary>
	[StructLayout(LayoutKind.Sequential)]
	internal struct RawOpened
	{
		public uint Format;
		public uint Epoch;
		public ulong StringsBytes;
		public ulong StringsCount;
		public ulong Needed;
		public RawFailure Failure;
	}

	/// <summary>The dialect as <c>hypertabular_delimited_init_with</c> takes it.</summary>
	[StructLayout(LayoutKind.Sequential)]
	internal struct RawDelimitedOptions
	{
		public uint Size;
		public byte Separator;
		public byte Quoting;
		public byte SkipBlankLines;
		public byte Engine;
		public uint Skip;
		public uint HeaderRow;
		public uint ShortRows;
		public uint LongRows;
	}

	/// <summary>A sheet's options as <c>hypertabular_workbook_sheet_with</c> takes them.</summary>
	[StructLayout(LayoutKind.Sequential)]
	internal struct RawSheetOptions
	{
		public uint Size;
		public uint HasHeader;
		public uint SkipEmptyRows;
		public uint Skip;
		public uint HeaderRow;
		public uint FirstRow;
		public uint FirstColumn;
		public uint LastRow;
		public uint LastColumn;
	}

	/// <summary>What <c>hypertabular_range_parse</c> makes of <c>A5:F200</c>.</summary>
	[StructLayout(LayoutKind.Sequential)]
	internal struct RawRange
	{
		public uint FirstRow;
		public uint FirstColumn;
		public uint LastRow;
		public uint LastColumn;
	}

	/// <summary>One plan column as the <c>_with</c> fills take it: <see cref="RawColumnSpec"/>, with date options, a month vocabulary and tokens.</summary>
	[StructLayout(LayoutKind.Sequential)]
	internal struct RawColumnSpecWith
	{
		public uint Size;
		public uint Ordinal;
		public uint Door;
		public uint Param;
		public RawNumFormat Format;
		public RawDateOptions Date;
		public byte* Months;
		public nuint MonthsLen;
		public byte* Tokens;
		public nuint TokensLen;
	}

	/// <summary>What a verdict does not say about a cell: HyperCast's detail code, and where the verdict came from.</summary>
	[StructLayout(LayoutKind.Sequential)]
	internal struct RawCellNote
	{
		public ushort Detail;
		public ushort Origin;
	}

	/// <summary>Where one column's output goes in the <c>_with</c> fills and what <c>hypertabular_faults</c> reads back.</summary>
	[StructLayout(LayoutKind.Sequential)]
	internal struct RawColumnBufferWith
	{
		public uint Size;
		public void* Values;
		public CellVerdict* Verdicts;
		public RawCellNote* Notes;
	}

	/// <summary>The memory the <c>_with</c> fills work in: <see cref="RawBuffers"/>'s fields, tokens and the row widths.</summary>
	[StructLayout(LayoutKind.Sequential)]
	internal struct RawBuffersWith
	{
		public uint Size;
		public byte* Window;
		public nuint WindowCap;
		public byte* Arena;
		public nuint ArenaCap;
		public RawSpan* Cells;
		public nuint CellsCap;
		public RawSlot* Row;
		public nuint RowCap;
		public byte* Strings;
		public nuint StringsLen;
		public RawSpan* Table;
		public nuint TableLen;
		public byte* Kinds;
		public nuint KindsLen;
		public byte* Tokens;
		public nuint TokensLen;
		public uint* RowWidths;
	}

	/// <summary>One thing <c>hypertabular_faults</c> lists.</summary>
	[StructLayout(LayoutKind.Sequential)]
	internal struct RawProblem
	{
		/// <summary><see cref="PlanColumn"/> of an entry about the row as a whole.</summary>
		public const uint RowLevel = uint.MaxValue;

		public uint Row;
		public uint PlanColumn;
		public uint Reason;
		public ushort Detail;
		public ushort Origin;
		public uint Offset;
		public uint Len;
		public uint Expected;
		public uint Found;
	}

	/// <summary>What <c>hypertabular_suggest</c> says of one column of a sample.</summary>
	[StructLayout(LayoutKind.Sequential)]
	internal struct RawSuggestion
	{
		public uint Door;
		public uint Param;
		public uint Status;
		public uint Matched;
		public uint Sampled;
	}

	/// <summary>How far <c>hypertabular_transcode</c> got: every field counts this call's input and output.</summary>
	[StructLayout(LayoutKind.Sequential)]
	internal struct RawTranscoded
	{
		public ulong Consumed;
		public ulong Written;
		public ulong FailureAt;
		public uint Failed;
		public uint Reserved;
	}

	/// <summary>What <c>hypertabular_batch_check</c> found of a read's sizes.</summary>
	[StructLayout(LayoutKind.Sequential)]
	internal struct RawBatchCheck
	{
		public ulong Needed;
		public ulong Limit;
		public ulong BufferBytes;
		public ulong SampleRows;
		public ulong Width;
		public uint Refused;
		public uint Reserved;
	}

	[LibraryImport("hypertabular", EntryPoint = "hypertabular_version")]
	private static partial uint hypertabular_version_native();
	[LibraryImport("*", EntryPoint = "hypertabular_version")]
	private static partial uint hypertabular_version_browser();
	[LibraryImport("__Internal", EntryPoint = "hypertabular_version")]
	private static partial uint hypertabular_version_internal();
	internal static uint hypertabular_version() =>
		OperatingSystem.IsBrowser() ? hypertabular_version_browser()
			: OperatingSystem.IsIOS() ? hypertabular_version_internal()
			: hypertabular_version_native();

	[LibraryImport("hypertabular", EntryPoint = "hypertabular_delimited_state_size")]
	private static partial nuint hypertabular_delimited_state_size_native();
	[LibraryImport("*", EntryPoint = "hypertabular_delimited_state_size")]
	private static partial nuint hypertabular_delimited_state_size_browser();
	[LibraryImport("__Internal", EntryPoint = "hypertabular_delimited_state_size")]
	private static partial nuint hypertabular_delimited_state_size_internal();
	internal static nuint hypertabular_delimited_state_size() =>
		OperatingSystem.IsBrowser() ? hypertabular_delimited_state_size_browser()
			: OperatingSystem.IsIOS() ? hypertabular_delimited_state_size_internal()
			: hypertabular_delimited_state_size_native();

	[LibraryImport("hypertabular", EntryPoint = "hypertabular_delimited_init")]
	private static partial int hypertabular_delimited_init_native(State* state, RawDialect* dialect);
	[LibraryImport("*", EntryPoint = "hypertabular_delimited_init")]
	private static partial int hypertabular_delimited_init_browser(State* state, RawDialect* dialect);
	[LibraryImport("__Internal", EntryPoint = "hypertabular_delimited_init")]
	private static partial int hypertabular_delimited_init_internal(State* state, RawDialect* dialect);
	internal static int hypertabular_delimited_init(State* state, RawDialect* dialect) =>
		OperatingSystem.IsBrowser() ? hypertabular_delimited_init_browser(state, dialect)
			: OperatingSystem.IsIOS() ? hypertabular_delimited_init_internal(state, dialect)
			: hypertabular_delimited_init_native(state, dialect);

	[LibraryImport("hypertabular", EntryPoint = "hypertabular_delimited_header")]
	private static partial int hypertabular_delimited_header_native(
		State* state, byte* input, nuint inputLen, uint last,
		RawSpan* names, nuint namesCap, byte* arena, nuint arenaCap, RawFilled* filled);
	[LibraryImport("*", EntryPoint = "hypertabular_delimited_header")]
	private static partial int hypertabular_delimited_header_browser(
		State* state, byte* input, nuint inputLen, uint last,
		RawSpan* names, nuint namesCap, byte* arena, nuint arenaCap, RawFilled* filled);
	[LibraryImport("__Internal", EntryPoint = "hypertabular_delimited_header")]
	private static partial int hypertabular_delimited_header_internal(
		State* state, byte* input, nuint inputLen, uint last,
		RawSpan* names, nuint namesCap, byte* arena, nuint arenaCap, RawFilled* filled);
	internal static int hypertabular_delimited_header(
		State* state, byte* input, nuint inputLen, uint last,
		RawSpan* names, nuint namesCap, byte* arena, nuint arenaCap, RawFilled* filled) =>
		OperatingSystem.IsBrowser() ? hypertabular_delimited_header_browser(state, input, inputLen, last, names, namesCap, arena, arenaCap, filled)
			: OperatingSystem.IsIOS() ? hypertabular_delimited_header_internal(state, input, inputLen, last, names, namesCap, arena, arenaCap, filled)
			: hypertabular_delimited_header_native(state, input, inputLen, last, names, namesCap, arena, arenaCap, filled);

	[LibraryImport("hypertabular", EntryPoint = "hypertabular_delimited_fill")]
	private static partial int hypertabular_delimited_fill_native(
		State* state, byte* input, nuint inputLen, uint last,
		RawColumnSpec* specs, RawColumnBuffer* columns, nuint columnCount, nuint maxRows,
		RawBuffers* buffers, RawFilled* filled);
	[LibraryImport("*", EntryPoint = "hypertabular_delimited_fill")]
	private static partial int hypertabular_delimited_fill_browser(
		State* state, byte* input, nuint inputLen, uint last,
		RawColumnSpec* specs, RawColumnBuffer* columns, nuint columnCount, nuint maxRows,
		RawBuffers* buffers, RawFilled* filled);
	[LibraryImport("__Internal", EntryPoint = "hypertabular_delimited_fill")]
	private static partial int hypertabular_delimited_fill_internal(
		State* state, byte* input, nuint inputLen, uint last,
		RawColumnSpec* specs, RawColumnBuffer* columns, nuint columnCount, nuint maxRows,
		RawBuffers* buffers, RawFilled* filled);
	internal static int hypertabular_delimited_fill(
		State* state, byte* input, nuint inputLen, uint last,
		RawColumnSpec* specs, RawColumnBuffer* columns, nuint columnCount, nuint maxRows,
		RawBuffers* buffers, RawFilled* filled) =>
		OperatingSystem.IsBrowser() ? hypertabular_delimited_fill_browser(state, input, inputLen, last, specs, columns, columnCount, maxRows, buffers, filled)
			: OperatingSystem.IsIOS() ? hypertabular_delimited_fill_internal(state, input, inputLen, last, specs, columns, columnCount, maxRows, buffers, filled)
			: hypertabular_delimited_fill_native(state, input, inputLen, last, specs, columns, columnCount, maxRows, buffers, filled);

	[LibraryImport("hypertabular", EntryPoint = "hypertabular_delimited_unescape")]
	private static partial nuint hypertabular_delimited_unescape_native(byte* cell, nuint len, byte* output, nuint cap);
	[LibraryImport("*", EntryPoint = "hypertabular_delimited_unescape")]
	private static partial nuint hypertabular_delimited_unescape_browser(byte* cell, nuint len, byte* output, nuint cap);
	[LibraryImport("__Internal", EntryPoint = "hypertabular_delimited_unescape")]
	private static partial nuint hypertabular_delimited_unescape_internal(byte* cell, nuint len, byte* output, nuint cap);
	internal static nuint hypertabular_delimited_unescape(byte* cell, nuint len, byte* output, nuint cap) =>
		OperatingSystem.IsBrowser() ? hypertabular_delimited_unescape_browser(cell, len, output, cap)
			: OperatingSystem.IsIOS() ? hypertabular_delimited_unescape_internal(cell, len, output, cap)
			: hypertabular_delimited_unescape_native(cell, len, output, cap);

	[LibraryImport("hypertabular", EntryPoint = "hypertabular_workbook_state_size")]
	private static partial nuint hypertabular_workbook_state_size_native();
	[LibraryImport("*", EntryPoint = "hypertabular_workbook_state_size")]
	private static partial nuint hypertabular_workbook_state_size_browser();
	[LibraryImport("__Internal", EntryPoint = "hypertabular_workbook_state_size")]
	private static partial nuint hypertabular_workbook_state_size_internal();
	internal static nuint hypertabular_workbook_state_size() =>
		OperatingSystem.IsBrowser() ? hypertabular_workbook_state_size_browser()
			: OperatingSystem.IsIOS() ? hypertabular_workbook_state_size_internal()
			: hypertabular_workbook_state_size_native();

	[LibraryImport("hypertabular", EntryPoint = "hypertabular_workbook_open")]
	private static partial int hypertabular_workbook_open_native(
		void* state, byte* container, nuint containerLen, RawBuffers* buffers, RawOpened* opened);
	[LibraryImport("*", EntryPoint = "hypertabular_workbook_open")]
	private static partial int hypertabular_workbook_open_browser(
		void* state, byte* container, nuint containerLen, RawBuffers* buffers, RawOpened* opened);
	[LibraryImport("__Internal", EntryPoint = "hypertabular_workbook_open")]
	private static partial int hypertabular_workbook_open_internal(
		void* state, byte* container, nuint containerLen, RawBuffers* buffers, RawOpened* opened);
	internal static int hypertabular_workbook_open(
		void* state, byte* container, nuint containerLen, RawBuffers* buffers, RawOpened* opened) =>
		OperatingSystem.IsBrowser() ? hypertabular_workbook_open_browser(state, container, containerLen, buffers, opened)
			: OperatingSystem.IsIOS() ? hypertabular_workbook_open_internal(state, container, containerLen, buffers, opened)
			: hypertabular_workbook_open_native(state, container, containerLen, buffers, opened);

	[LibraryImport("hypertabular", EntryPoint = "hypertabular_workbook_sheets")]
	private static partial int hypertabular_workbook_sheets_native(
		void* state, byte* container, nuint containerLen, RawBuffers* buffers, RawFilled* filled);
	[LibraryImport("*", EntryPoint = "hypertabular_workbook_sheets")]
	private static partial int hypertabular_workbook_sheets_browser(
		void* state, byte* container, nuint containerLen, RawBuffers* buffers, RawFilled* filled);
	[LibraryImport("__Internal", EntryPoint = "hypertabular_workbook_sheets")]
	private static partial int hypertabular_workbook_sheets_internal(
		void* state, byte* container, nuint containerLen, RawBuffers* buffers, RawFilled* filled);
	internal static int hypertabular_workbook_sheets(
		void* state, byte* container, nuint containerLen, RawBuffers* buffers, RawFilled* filled) =>
		OperatingSystem.IsBrowser() ? hypertabular_workbook_sheets_browser(state, container, containerLen, buffers, filled)
			: OperatingSystem.IsIOS() ? hypertabular_workbook_sheets_internal(state, container, containerLen, buffers, filled)
			: hypertabular_workbook_sheets_native(state, container, containerLen, buffers, filled);

	[LibraryImport("hypertabular", EntryPoint = "hypertabular_workbook_strings")]
	private static partial int hypertabular_workbook_strings_native(
		void* state, byte* container, nuint containerLen, RawBuffers* buffers, RawFilled* filled);
	[LibraryImport("*", EntryPoint = "hypertabular_workbook_strings")]
	private static partial int hypertabular_workbook_strings_browser(
		void* state, byte* container, nuint containerLen, RawBuffers* buffers, RawFilled* filled);
	[LibraryImport("__Internal", EntryPoint = "hypertabular_workbook_strings")]
	private static partial int hypertabular_workbook_strings_internal(
		void* state, byte* container, nuint containerLen, RawBuffers* buffers, RawFilled* filled);
	internal static int hypertabular_workbook_strings(
		void* state, byte* container, nuint containerLen, RawBuffers* buffers, RawFilled* filled) =>
		OperatingSystem.IsBrowser() ? hypertabular_workbook_strings_browser(state, container, containerLen, buffers, filled)
			: OperatingSystem.IsIOS() ? hypertabular_workbook_strings_internal(state, container, containerLen, buffers, filled)
			: hypertabular_workbook_strings_native(state, container, containerLen, buffers, filled);

	[LibraryImport("hypertabular", EntryPoint = "hypertabular_workbook_styles")]
	private static partial int hypertabular_workbook_styles_native(
		void* state, byte* container, nuint containerLen, RawBuffers* buffers, RawFilled* filled);
	[LibraryImport("*", EntryPoint = "hypertabular_workbook_styles")]
	private static partial int hypertabular_workbook_styles_browser(
		void* state, byte* container, nuint containerLen, RawBuffers* buffers, RawFilled* filled);
	[LibraryImport("__Internal", EntryPoint = "hypertabular_workbook_styles")]
	private static partial int hypertabular_workbook_styles_internal(
		void* state, byte* container, nuint containerLen, RawBuffers* buffers, RawFilled* filled);
	internal static int hypertabular_workbook_styles(
		void* state, byte* container, nuint containerLen, RawBuffers* buffers, RawFilled* filled) =>
		OperatingSystem.IsBrowser() ? hypertabular_workbook_styles_browser(state, container, containerLen, buffers, filled)
			: OperatingSystem.IsIOS() ? hypertabular_workbook_styles_internal(state, container, containerLen, buffers, filled)
			: hypertabular_workbook_styles_native(state, container, containerLen, buffers, filled);

	[LibraryImport("hypertabular", EntryPoint = "hypertabular_workbook_sheet")]
	private static partial int hypertabular_workbook_sheet_native(
		void* state, byte* container, nuint containerLen, byte* part, nuint partLen, uint index,
		uint hasHeader, uint skipEmptyRows, RawFilled* filled);
	[LibraryImport("*", EntryPoint = "hypertabular_workbook_sheet")]
	private static partial int hypertabular_workbook_sheet_browser(
		void* state, byte* container, nuint containerLen, byte* part, nuint partLen, uint index,
		uint hasHeader, uint skipEmptyRows, RawFilled* filled);
	[LibraryImport("__Internal", EntryPoint = "hypertabular_workbook_sheet")]
	private static partial int hypertabular_workbook_sheet_internal(
		void* state, byte* container, nuint containerLen, byte* part, nuint partLen, uint index,
		uint hasHeader, uint skipEmptyRows, RawFilled* filled);
	internal static int hypertabular_workbook_sheet(
		void* state, byte* container, nuint containerLen, byte* part, nuint partLen, uint index,
		uint hasHeader, uint skipEmptyRows, RawFilled* filled) =>
		OperatingSystem.IsBrowser() ? hypertabular_workbook_sheet_browser(state, container, containerLen, part, partLen, index, hasHeader, skipEmptyRows, filled)
			: OperatingSystem.IsIOS() ? hypertabular_workbook_sheet_internal(state, container, containerLen, part, partLen, index, hasHeader, skipEmptyRows, filled)
			: hypertabular_workbook_sheet_native(state, container, containerLen, part, partLen, index, hasHeader, skipEmptyRows, filled);

	[LibraryImport("hypertabular", EntryPoint = "hypertabular_workbook_header")]
	private static partial int hypertabular_workbook_header_native(
		void* state, byte* container, nuint containerLen, RawBuffers* buffers, RawFilled* filled);
	[LibraryImport("*", EntryPoint = "hypertabular_workbook_header")]
	private static partial int hypertabular_workbook_header_browser(
		void* state, byte* container, nuint containerLen, RawBuffers* buffers, RawFilled* filled);
	[LibraryImport("__Internal", EntryPoint = "hypertabular_workbook_header")]
	private static partial int hypertabular_workbook_header_internal(
		void* state, byte* container, nuint containerLen, RawBuffers* buffers, RawFilled* filled);
	internal static int hypertabular_workbook_header(
		void* state, byte* container, nuint containerLen, RawBuffers* buffers, RawFilled* filled) =>
		OperatingSystem.IsBrowser() ? hypertabular_workbook_header_browser(state, container, containerLen, buffers, filled)
			: OperatingSystem.IsIOS() ? hypertabular_workbook_header_internal(state, container, containerLen, buffers, filled)
			: hypertabular_workbook_header_native(state, container, containerLen, buffers, filled);

	[LibraryImport("hypertabular", EntryPoint = "hypertabular_workbook_fill")]
	private static partial int hypertabular_workbook_fill_native(
		void* state, byte* container, nuint containerLen, RawColumnSpec* specs, RawColumnBuffer* columns,
		nuint columnCount, nuint maxRows, RawBuffers* buffers, RawFilled* filled);
	[LibraryImport("*", EntryPoint = "hypertabular_workbook_fill")]
	private static partial int hypertabular_workbook_fill_browser(
		void* state, byte* container, nuint containerLen, RawColumnSpec* specs, RawColumnBuffer* columns,
		nuint columnCount, nuint maxRows, RawBuffers* buffers, RawFilled* filled);
	[LibraryImport("__Internal", EntryPoint = "hypertabular_workbook_fill")]
	private static partial int hypertabular_workbook_fill_internal(
		void* state, byte* container, nuint containerLen, RawColumnSpec* specs, RawColumnBuffer* columns,
		nuint columnCount, nuint maxRows, RawBuffers* buffers, RawFilled* filled);
	internal static int hypertabular_workbook_fill(
		void* state, byte* container, nuint containerLen, RawColumnSpec* specs, RawColumnBuffer* columns,
		nuint columnCount, nuint maxRows, RawBuffers* buffers, RawFilled* filled) =>
		OperatingSystem.IsBrowser() ? hypertabular_workbook_fill_browser(state, container, containerLen, specs, columns, columnCount, maxRows, buffers, filled)
			: OperatingSystem.IsIOS() ? hypertabular_workbook_fill_internal(state, container, containerLen, specs, columns, columnCount, maxRows, buffers, filled)
			: hypertabular_workbook_fill_native(state, container, containerLen, specs, columns, columnCount, maxRows, buffers, filled);

	[LibraryImport("hypertabular", EntryPoint = "hypertabular_delimited_init_with")]
	private static partial int hypertabular_delimited_init_with_native(
		State* state, RawDelimitedOptions* options);
	[LibraryImport("*", EntryPoint = "hypertabular_delimited_init_with")]
	private static partial int hypertabular_delimited_init_with_browser(
		State* state, RawDelimitedOptions* options);
	[LibraryImport("__Internal", EntryPoint = "hypertabular_delimited_init_with")]
	private static partial int hypertabular_delimited_init_with_internal(
		State* state, RawDelimitedOptions* options);
	internal static int hypertabular_delimited_init_with(
		State* state, RawDelimitedOptions* options) =>
		OperatingSystem.IsBrowser() ? hypertabular_delimited_init_with_browser(state, options)
			: OperatingSystem.IsIOS() ? hypertabular_delimited_init_with_internal(state, options)
			: hypertabular_delimited_init_with_native(state, options);

	[LibraryImport("hypertabular", EntryPoint = "hypertabular_delimited_fill_with")]
	private static partial int hypertabular_delimited_fill_with_native(
		State* state, byte* input, nuint inputLen, uint last, RawColumnSpecWith* specs, RawColumnBufferWith* columns, nuint columnCount, nuint maxRows, RawBuffersWith* buffers, RawFilled* filled);
	[LibraryImport("*", EntryPoint = "hypertabular_delimited_fill_with")]
	private static partial int hypertabular_delimited_fill_with_browser(
		State* state, byte* input, nuint inputLen, uint last, RawColumnSpecWith* specs, RawColumnBufferWith* columns, nuint columnCount, nuint maxRows, RawBuffersWith* buffers, RawFilled* filled);
	[LibraryImport("__Internal", EntryPoint = "hypertabular_delimited_fill_with")]
	private static partial int hypertabular_delimited_fill_with_internal(
		State* state, byte* input, nuint inputLen, uint last, RawColumnSpecWith* specs, RawColumnBufferWith* columns, nuint columnCount, nuint maxRows, RawBuffersWith* buffers, RawFilled* filled);
	internal static int hypertabular_delimited_fill_with(
		State* state, byte* input, nuint inputLen, uint last, RawColumnSpecWith* specs, RawColumnBufferWith* columns, nuint columnCount, nuint maxRows, RawBuffersWith* buffers, RawFilled* filled) =>
		OperatingSystem.IsBrowser() ? hypertabular_delimited_fill_with_browser(state, input, inputLen, last, specs, columns, columnCount, maxRows, buffers, filled)
			: OperatingSystem.IsIOS() ? hypertabular_delimited_fill_with_internal(state, input, inputLen, last, specs, columns, columnCount, maxRows, buffers, filled)
			: hypertabular_delimited_fill_with_native(state, input, inputLen, last, specs, columns, columnCount, maxRows, buffers, filled);

	[LibraryImport("hypertabular", EntryPoint = "hypertabular_transcode")]
	private static partial int hypertabular_transcode_native(
		uint encoding, byte* input, nuint inputLen, uint last, byte* output, nuint cap, RawTranscoded* progress);
	[LibraryImport("*", EntryPoint = "hypertabular_transcode")]
	private static partial int hypertabular_transcode_browser(
		uint encoding, byte* input, nuint inputLen, uint last, byte* output, nuint cap, RawTranscoded* progress);
	[LibraryImport("__Internal", EntryPoint = "hypertabular_transcode")]
	private static partial int hypertabular_transcode_internal(
		uint encoding, byte* input, nuint inputLen, uint last, byte* output, nuint cap, RawTranscoded* progress);
	internal static int hypertabular_transcode(
		uint encoding, byte* input, nuint inputLen, uint last, byte* output, nuint cap, RawTranscoded* progress) =>
		OperatingSystem.IsBrowser() ? hypertabular_transcode_browser(encoding, input, inputLen, last, output, cap, progress)
			: OperatingSystem.IsIOS() ? hypertabular_transcode_internal(encoding, input, inputLen, last, output, cap, progress)
			: hypertabular_transcode_native(encoding, input, inputLen, last, output, cap, progress);

	[LibraryImport("hypertabular", EntryPoint = "hypertabular_faults")]
	private static partial int hypertabular_faults_native(
		RawColumnBufferWith* columns, nuint columnCount, nuint rows, uint* rowWidths, uint expectedWidth, uint longRows, ulong* cursor, RawProblem* output, nuint cap, nuint* written);
	[LibraryImport("*", EntryPoint = "hypertabular_faults")]
	private static partial int hypertabular_faults_browser(
		RawColumnBufferWith* columns, nuint columnCount, nuint rows, uint* rowWidths, uint expectedWidth, uint longRows, ulong* cursor, RawProblem* output, nuint cap, nuint* written);
	[LibraryImport("__Internal", EntryPoint = "hypertabular_faults")]
	private static partial int hypertabular_faults_internal(
		RawColumnBufferWith* columns, nuint columnCount, nuint rows, uint* rowWidths, uint expectedWidth, uint longRows, ulong* cursor, RawProblem* output, nuint cap, nuint* written);
	internal static int hypertabular_faults(
		RawColumnBufferWith* columns, nuint columnCount, nuint rows, uint* rowWidths, uint expectedWidth, uint longRows, ulong* cursor, RawProblem* output, nuint cap, nuint* written) =>
		OperatingSystem.IsBrowser() ? hypertabular_faults_browser(columns, columnCount, rows, rowWidths, expectedWidth, longRows, cursor, output, cap, written)
			: OperatingSystem.IsIOS() ? hypertabular_faults_internal(columns, columnCount, rows, rowWidths, expectedWidth, longRows, cursor, output, cap, written)
			: hypertabular_faults_native(columns, columnCount, rows, rowWidths, expectedWidth, longRows, cursor, output, cap, written);

	[LibraryImport("hypertabular", EntryPoint = "hypertabular_suggest")]
	private static partial int hypertabular_suggest_native(
		byte* input, nuint inputLen, byte* arena, nuint arenaLen, RawSpan* cells, byte* typed, nuint rows, nuint columns, RawNumFormat* formats, RawSuggestion* output);
	[LibraryImport("*", EntryPoint = "hypertabular_suggest")]
	private static partial int hypertabular_suggest_browser(
		byte* input, nuint inputLen, byte* arena, nuint arenaLen, RawSpan* cells, byte* typed, nuint rows, nuint columns, RawNumFormat* formats, RawSuggestion* output);
	[LibraryImport("__Internal", EntryPoint = "hypertabular_suggest")]
	private static partial int hypertabular_suggest_internal(
		byte* input, nuint inputLen, byte* arena, nuint arenaLen, RawSpan* cells, byte* typed, nuint rows, nuint columns, RawNumFormat* formats, RawSuggestion* output);
	internal static int hypertabular_suggest(
		byte* input, nuint inputLen, byte* arena, nuint arenaLen, RawSpan* cells, byte* typed, nuint rows, nuint columns, RawNumFormat* formats, RawSuggestion* output) =>
		OperatingSystem.IsBrowser() ? hypertabular_suggest_browser(input, inputLen, arena, arenaLen, cells, typed, rows, columns, formats, output)
			: OperatingSystem.IsIOS() ? hypertabular_suggest_internal(input, inputLen, arena, arenaLen, cells, typed, rows, columns, formats, output)
			: hypertabular_suggest_native(input, inputLen, arena, arenaLen, cells, typed, rows, columns, formats, output);

	[LibraryImport("hypertabular", EntryPoint = "hypertabular_workbook_sheet_with")]
	private static partial int hypertabular_workbook_sheet_with_native(
		void* state, byte* container, nuint containerLen, byte* part, nuint partLen, uint index, RawSheetOptions* options, RawFilled* filled);
	[LibraryImport("*", EntryPoint = "hypertabular_workbook_sheet_with")]
	private static partial int hypertabular_workbook_sheet_with_browser(
		void* state, byte* container, nuint containerLen, byte* part, nuint partLen, uint index, RawSheetOptions* options, RawFilled* filled);
	[LibraryImport("__Internal", EntryPoint = "hypertabular_workbook_sheet_with")]
	private static partial int hypertabular_workbook_sheet_with_internal(
		void* state, byte* container, nuint containerLen, byte* part, nuint partLen, uint index, RawSheetOptions* options, RawFilled* filled);
	internal static int hypertabular_workbook_sheet_with(
		void* state, byte* container, nuint containerLen, byte* part, nuint partLen, uint index, RawSheetOptions* options, RawFilled* filled) =>
		OperatingSystem.IsBrowser() ? hypertabular_workbook_sheet_with_browser(state, container, containerLen, part, partLen, index, options, filled)
			: OperatingSystem.IsIOS() ? hypertabular_workbook_sheet_with_internal(state, container, containerLen, part, partLen, index, options, filled)
			: hypertabular_workbook_sheet_with_native(state, container, containerLen, part, partLen, index, options, filled);

	[LibraryImport("hypertabular", EntryPoint = "hypertabular_workbook_fill_with")]
	private static partial int hypertabular_workbook_fill_with_native(
		void* state, byte* container, nuint containerLen, RawColumnSpecWith* specs, RawColumnBufferWith* columns, nuint columnCount, nuint maxRows, RawBuffersWith* buffers, RawFilled* filled);
	[LibraryImport("*", EntryPoint = "hypertabular_workbook_fill_with")]
	private static partial int hypertabular_workbook_fill_with_browser(
		void* state, byte* container, nuint containerLen, RawColumnSpecWith* specs, RawColumnBufferWith* columns, nuint columnCount, nuint maxRows, RawBuffersWith* buffers, RawFilled* filled);
	[LibraryImport("__Internal", EntryPoint = "hypertabular_workbook_fill_with")]
	private static partial int hypertabular_workbook_fill_with_internal(
		void* state, byte* container, nuint containerLen, RawColumnSpecWith* specs, RawColumnBufferWith* columns, nuint columnCount, nuint maxRows, RawBuffersWith* buffers, RawFilled* filled);
	internal static int hypertabular_workbook_fill_with(
		void* state, byte* container, nuint containerLen, RawColumnSpecWith* specs, RawColumnBufferWith* columns, nuint columnCount, nuint maxRows, RawBuffersWith* buffers, RawFilled* filled) =>
		OperatingSystem.IsBrowser() ? hypertabular_workbook_fill_with_browser(state, container, containerLen, specs, columns, columnCount, maxRows, buffers, filled)
			: OperatingSystem.IsIOS() ? hypertabular_workbook_fill_with_internal(state, container, containerLen, specs, columns, columnCount, maxRows, buffers, filled)
			: hypertabular_workbook_fill_with_native(state, container, containerLen, specs, columns, columnCount, maxRows, buffers, filled);

	[LibraryImport("hypertabular", EntryPoint = "hypertabular_range_parse")]
	private static partial int hypertabular_range_parse_native(
		byte* text, nuint length, RawRange* output);
	[LibraryImport("*", EntryPoint = "hypertabular_range_parse")]
	private static partial int hypertabular_range_parse_browser(
		byte* text, nuint length, RawRange* output);
	[LibraryImport("__Internal", EntryPoint = "hypertabular_range_parse")]
	private static partial int hypertabular_range_parse_internal(
		byte* text, nuint length, RawRange* output);
	internal static int hypertabular_range_parse(
		byte* text, nuint length, RawRange* output) =>
		OperatingSystem.IsBrowser() ? hypertabular_range_parse_browser(text, length, output)
			: OperatingSystem.IsIOS() ? hypertabular_range_parse_internal(text, length, output)
			: hypertabular_range_parse_native(text, length, output);

	[LibraryImport("hypertabular", EntryPoint = "hypertabular_batch_check")]
	private static partial int hypertabular_batch_check_native(
		RawColumnSpecWith* specs, nuint columnCount, ulong batchRows, ulong bufferBytes, uint flags, RawBatchCheck* output);
	[LibraryImport("*", EntryPoint = "hypertabular_batch_check")]
	private static partial int hypertabular_batch_check_browser(
		RawColumnSpecWith* specs, nuint columnCount, ulong batchRows, ulong bufferBytes, uint flags, RawBatchCheck* output);
	[LibraryImport("__Internal", EntryPoint = "hypertabular_batch_check")]
	private static partial int hypertabular_batch_check_internal(
		RawColumnSpecWith* specs, nuint columnCount, ulong batchRows, ulong bufferBytes, uint flags, RawBatchCheck* output);
	internal static int hypertabular_batch_check(
		RawColumnSpecWith* specs, nuint columnCount, ulong batchRows, ulong bufferBytes, uint flags, RawBatchCheck* output) =>
		OperatingSystem.IsBrowser() ? hypertabular_batch_check_browser(specs, columnCount, batchRows, bufferBytes, flags, output)
			: OperatingSystem.IsIOS() ? hypertabular_batch_check_internal(specs, columnCount, batchRows, bufferBytes, flags, output)
			: hypertabular_batch_check_native(specs, columnCount, batchRows, bufferBytes, flags, output);
}

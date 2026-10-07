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

	/// <summary>The core's state block: 64 bytes it defines and this assembly only keeps.</summary>
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
}

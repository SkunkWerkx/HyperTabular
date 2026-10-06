using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;

namespace HyperTabular;

/// <summary>
/// The native core's C ABI: <c>libhypertabular</c>'s exports and the <c>#[repr(C)]</c>
/// shapes that cross them (<c>rust/src/kernel/abi.rs</c>). Every pointer is this assembly's
/// own memory for the length of the call; the core keeps nothing.
/// </summary>
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
	internal struct RawNumFormat
	{
		public uint DecimalSep;
		public uint GroupSep;
		public uint Flags;
		public uint CurrencyLen;
		public CurrencyBytes Currency;
	}

	[InlineArray(16)]
	internal struct CurrencyBytes
	{
		byte _element0;
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

	[StructLayout(LayoutKind.Sequential)]
	internal struct RawTimestamp
	{
		public long Seconds;
		public int Nanos;
	}

	[StructLayout(LayoutKind.Sequential)]
	internal struct RawDate
	{
		public ushort Year;
		public byte Month;
		public byte Day;
	}

	[StructLayout(LayoutKind.Sequential)]
	internal struct RawCivil
	{
		public ushort Year;
		public byte Month;
		public byte Day;
		public ulong NanosOfDay;
	}

	[StructLayout(LayoutKind.Sequential)]
	internal struct RawDuration
	{
		public long Seconds;
		public int Nanos;
	}

	[StructLayout(LayoutKind.Sequential)]
	internal struct RawDecimal
	{
		public ulong Lo;
		public uint Hi;
		public byte Scale;
		public byte Negative;
	}

	/// <summary>One cell of the row a workbook read assembles: the core's scratch, never read here.</summary>
	[StructLayout(LayoutKind.Sequential)]
	internal struct RawSlot
	{
		public uint Tag;
		public uint Aux;
		public ulong Bits;
	}

	/// <summary>The memory a workbook call works in, handed over again on every call.</summary>
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

	[LibraryImport("hypertabular")]
	internal static partial uint hypertabular_version();

	[LibraryImport("hypertabular")]
	internal static partial nuint hypertabular_delimited_state_size();

	[LibraryImport("hypertabular")]
	internal static partial int hypertabular_delimited_init(State* state, RawDialect* dialect);

	[LibraryImport("hypertabular")]
	internal static partial int hypertabular_delimited_header(
		State* state, byte* input, nuint inputLen, uint last,
		RawSpan* names, nuint namesCap, byte* arena, nuint arenaCap, RawFilled* filled);

	[LibraryImport("hypertabular")]
	internal static partial int hypertabular_delimited_fill(
		State* state, byte* input, nuint inputLen, uint last,
		RawColumnSpec* specs, RawColumnBuffer* columns, nuint columnCount, nuint maxRows,
		RawSpan* cells, nuint cellsCap, byte* arena, nuint arenaCap, RawFilled* filled);

	[LibraryImport("hypertabular")]
	internal static partial nuint hypertabular_delimited_unescape(byte* cell, nuint len, byte* output, nuint cap);

	[LibraryImport("hypertabular")]
	internal static partial nuint hypertabular_workbook_state_size();

	[LibraryImport("hypertabular")]
	internal static partial int hypertabular_workbook_open(
		void* state, byte* container, nuint containerLen, RawBuffers* buffers, RawOpened* opened);

	[LibraryImport("hypertabular")]
	internal static partial int hypertabular_workbook_sheets(
		void* state, byte* container, nuint containerLen, RawBuffers* buffers, RawFilled* filled);

	[LibraryImport("hypertabular")]
	internal static partial int hypertabular_workbook_strings(
		void* state, byte* container, nuint containerLen, RawBuffers* buffers, RawFilled* filled);

	[LibraryImport("hypertabular")]
	internal static partial int hypertabular_workbook_styles(
		void* state, byte* container, nuint containerLen, RawBuffers* buffers, RawFilled* filled);

	[LibraryImport("hypertabular")]
	internal static partial int hypertabular_workbook_sheet(
		void* state, byte* container, nuint containerLen, byte* part, nuint partLen, uint index,
		uint hasHeader, uint skipEmptyRows, RawFilled* filled);

	[LibraryImport("hypertabular")]
	internal static partial int hypertabular_workbook_header(
		void* state, byte* container, nuint containerLen, RawBuffers* buffers, RawFilled* filled);

	[LibraryImport("hypertabular")]
	internal static partial int hypertabular_workbook_fill(
		void* state, byte* container, nuint containerLen, RawColumnSpec* specs, RawColumnBuffer* columns,
		nuint columnCount, nuint maxRows, RawBuffers* buffers, RawFilled* filled);
}

using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using HyperCast;
using HyperCast.Interop;

namespace HyperTabular;

/// <summary>
/// The columns a reader owns: the plan, the plan as the core takes it, and for each column
/// a value array and a verdict array sized for one batch. Every array is on the pinned
/// object heap — the core is handed their addresses, and an address that could move is one
/// that would have to be pinned again on every call.
/// </summary>
sealed unsafe class Columns
{
	internal readonly Column[] Plan;
	internal readonly int BatchRows;
	/// <summary>The widest ordinal the plan reads, plus one: the row slots a sheet needs.</summary>
	internal readonly int Width;
	internal readonly Native.RawColumnSpec[] Specs;
	internal readonly Native.RawColumnBuffer[] Buffers;
	internal readonly byte[][] Values;
	internal readonly CellVerdict[][] Verdicts;

	/// <summary>
	/// The widest ordinal a plan may name: 2²⁰ − 1. No workbook format has that many columns
	/// (XLSX and ODS stop at 16,384, Numbers at 1,000), and at the cap one row of cell table
	/// is at most 8 MiB, so a headerless plan cannot ask for ~16 GiB.
	/// </summary>
	internal const int MaxOrdinal = (1 << 20) - 1;

	internal Columns(ReadOnlySpan<Column> plan, int batchRows)
	{
		ArgumentOutOfRangeException.ThrowIfNegativeOrZero(batchRows);
		Plan = plan.ToArray();
		BatchRows = batchRows;
		Specs = GC.AllocateUninitializedArray<Native.RawColumnSpec>(Plan.Length, pinned: true);
		Buffers = GC.AllocateUninitializedArray<Native.RawColumnBuffer>(Plan.Length, pinned: true);
		Values = new byte[Plan.Length][];
		Verdicts = new CellVerdict[Plan.Length][];
		for (var index = 0; index < Plan.Length; index++)
		{
			var column = Plan[index];
			if (column.Door == Door.Unspecified)
				throw new ArgumentException($"Plan column {index} is default(Column); build one with Column's factories.", "plan");
			// Refused here as the plan's, for a reader and a sheet alike.
			if (column.Ordinal > MaxOrdinal)
				throw new ArgumentException(
					$"Plan column {index} reads source column {column.Ordinal}; an ordinal must be below 2^20 (1,048,576).", "plan");
			Width = Math.Max(Width, column.Ordinal + 1);
			Specs[index] = new Native.RawColumnSpec
			{
				Ordinal = (uint)column.Ordinal,
				Door = (uint)column.Door,
				Param = column.Declared,
				Format = ToRaw(column.Format, index),
			};
			Values[index] = GC.AllocateUninitializedArray<byte>(checked(batchRows * column.ValueSize), pinned: true);
			Verdicts[index] = GC.AllocateUninitializedArray<CellVerdict>(batchRows, pinned: true);
			Buffers[index] = new Native.RawColumnBuffer
			{
				Values = Address(Values[index]),
				Verdicts = Address(Verdicts[index]),
			};
		}
	}

	/// <summary>The address of a pinned array's first element — stable for the array's lifetime.</summary>
	internal static T* Address<T>(T[] pinned) where T : unmanaged =>
		(T*)Unsafe.AsPointer(ref MemoryMarshal.GetArrayDataReference(pinned));

	/// <summary>A pinned array at least <paramref name="needed"/> long holding what <paramref name="old"/> held.</summary>
	internal static T[] Grow<T>(T[] old, long needed) where T : unmanaged
	{
		var length = (int)Math.Min(Math.Max(needed, (long)old.Length + 1), Array.MaxLength);
		var larger = GC.AllocateUninitializedArray<T>(length, pinned: true);
		old.AsSpan().CopyTo(larger);
		return larger;
	}

	/// <summary>HyperCast's <see cref="NumFormat"/> in the core's 92-byte size-tagged layout, validated by HyperCast as its own doors validate it.</summary>
	static RawNumFormat ToRaw(NumFormat format, int column)
	{
		try
		{
			return format.ToRaw();
		}
		catch (ArgumentException invalid)
		{
			throw new ArgumentException($"Plan column {column} declares a NumFormat no door accepts; the inner exception says why.", "plan", invalid);
		}
	}
}

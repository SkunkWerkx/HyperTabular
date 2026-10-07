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

	/// <summary>HyperCast's <see cref="NumFormat"/> in the core's 32-byte layout, validated by HyperCast as its own doors validate it.</summary>
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

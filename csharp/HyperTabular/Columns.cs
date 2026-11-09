using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using HyperCast;
using HyperCast.Interop;

namespace HyperTabular;

/// <summary>
/// The columns a reader owns: the plan, the plan as the core takes it, and for each column
/// a value array, a verdict array and, unless they are turned off, a note array sized for one
/// batch. Every array is on the pinned object heap — the core is handed their addresses, and an address that could move is one
/// that would have to be pinned again on every call.
/// </summary>
sealed unsafe class Columns
{
	internal readonly Column[] Plan;
	internal readonly int BatchRows;
	/// <summary>The widest ordinal the plan reads, plus one: the row slots a sheet needs.</summary>
	internal readonly int Width;
	internal readonly Native.RawColumnSpecWith[] Specs;
	internal readonly Native.RawColumnBufferWith[] Buffers;
	internal readonly byte[][] Values;
	internal readonly CellVerdict[][] Verdicts;
	/// <summary>Each column's notes: <see langword="null"/> with notes off.</summary>
	internal readonly Native.RawCellNote[][]? Notes;
	/// <summary>How many cells each row had, for a read that asks: <see langword="null"/> when none does.</summary>
	internal readonly uint[]? RowWidths;
	/// <summary>The tokens and month vocabularies the specs point at, held for as long as they do.</summary>
	readonly List<byte[]> _lists = [];

	/// <summary>
	/// The widest ordinal a plan may name: 2²⁰ − 1. No workbook format has that many columns
	/// (XLSX and ODS stop at 16,384, Numbers at 1,000), and at the cap one row of cell table
	/// is at most 8 MiB, so a headerless plan cannot ask for ~16 GiB.
	/// </summary>
	internal const int MaxOrdinal = (1 << 20) - 1;

	/// <summary>The flags of <c>hypertabular_batch_check</c>.</summary>
	internal const uint NotesFlag = 1, WidthsFlag = 2, SheetFlag = 4;

	/// <summary>
	/// The core's answer on a read's sizes, ahead of everything the read allocates: the core
	/// decides how much a batch may ask for, and says what size to start an input buffer at. A
	/// refusal is an <see cref="ArgumentOutOfRangeException"/> naming the bytes and the limit the core gave.
	/// </summary>
	internal static Native.RawBatchCheck CheckSizes(ReadOnlySpan<Native.RawColumnSpecWith> specs, int batchRows, int bufferBytes, uint flags)
	{
		// The core takes an unsigned count, which a negative one isn't.
		ArgumentOutOfRangeException.ThrowIfNegative(batchRows);
		ArgumentOutOfRangeException.ThrowIfNegative(bufferBytes);
		Native.RawBatchCheck found;
		int code;
		fixed (Native.RawColumnSpecWith* first = specs)
			code = Native.hypertabular_batch_check(first, (nuint)specs.Length, (ulong)batchRows, (ulong)bufferBytes, flags, &found);
		if (code == Native.Ok)
			return found;
		if (code != Native.ErrBatch)
			throw new ArgumentException("The plan cannot be honoured.", "plan");
		throw found.Refused switch
		{
			1 => new ArgumentOutOfRangeException(nameof(batchRows), batchRows, "batchRows must be at least 1."),
			2 => new ArgumentOutOfRangeException(nameof(bufferBytes), bufferBytes, "bufferBytes must be at least 1."),
			4 => new ArgumentException($"A row of {found.Width} columns asks for {found.Needed} bytes, past the limit of {found.Limit}.", "plan"),
			_ => new ArgumentOutOfRangeException(nameof(batchRows), batchRows, $"batchRows asks for {found.Needed} bytes, past the limit of {found.Limit}."),
		};
	}

	/// <summary>
	/// What the core is told of a plan for its sizes alone. A plan that can't be honoured says so
	/// where it always has, so its size is judged without it.
	/// </summary>
	internal static Native.RawColumnSpecWith[] SizeSpecs(ReadOnlySpan<Column> plan)
	{
		var specs = new Native.RawColumnSpecWith[plan.Length];
		for (var index = 0; index < plan.Length; index++)
		{
			var column = plan[index];
			if (column.Door == Door.Unspecified || column.Ordinal > MaxOrdinal)
				return [];
			specs[index] = new Native.RawColumnSpecWith
			{
				Size = (uint)sizeof(Native.RawColumnSpecWith),
				Ordinal = (uint)column.Ordinal,
				Door = (uint)column.Door,
				Param = column.Declared,
			};
		}
		return specs;
	}

	/// <summary>
	/// The rows of a sampler's batch for a sampler wanting <paramref name="wanted"/> rows: the core's
	/// answer, however many batches that takes. Only a row past the limit is refused, in its words.
	/// </summary>
	internal static int SampleRows(ReadOnlySpan<Column> plan, int wanted, uint flags)
	{
		var specs = SizeSpecs(plan);
		Native.RawBatchCheck found = default;
		fixed (Native.RawColumnSpecWith* first = specs)
			Native.hypertabular_batch_check(first, (nuint)specs.Length, (ulong)wanted, 1, flags, &found);
		if (found.SampleRows == 0)
			CheckSizes(specs, wanted, 1, flags);
		return (int)found.SampleRows;
	}

	internal Columns(ReadOnlySpan<Column> plan, int batchRows, bool notes = true, bool rowWidths = false, bool sheet = false, int bufferBytes = 1)
	{
		Plan = plan.ToArray();
		BatchRows = batchRows;
		Specs = GC.AllocateUninitializedArray<Native.RawColumnSpecWith>(Plan.Length, pinned: true);
		Buffers = GC.AllocateUninitializedArray<Native.RawColumnBufferWith>(Plan.Length, pinned: true);
		Values = new byte[Plan.Length][];
		Verdicts = new CellVerdict[Plan.Length][];
		Notes = notes ? new Native.RawCellNote[Plan.Length][] : null;
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
			var spec = new Native.RawColumnSpecWith
			{
				Size = (uint)sizeof(Native.RawColumnSpecWith),
				Ordinal = (uint)column.Ordinal,
				Door = (uint)column.Door,
				Param = column.Declared,
				Format = ToRaw(column.Format, index),
			};
			if (column.DateOptions != default)
			{
				spec.Date = ToRaw(column.DateOptions, index);
				if (column.DateOptions.Months is { } months)
				{
					spec.Months = Held(months.Packed);
					spec.MonthsLen = (nuint)months.Packed.Length;
				}
			}
			// An empty list has an address: null would inherit the dialect's or the sheet's.
			if (column.Tokens is { } tokens)
			{
				spec.Tokens = Held(tokens.Packed);
				spec.TokensLen = (nuint)tokens.Packed.Length;
			}
			Specs[index] = spec;
		}
		CheckSizes(Specs, batchRows, bufferBytes, (notes ? NotesFlag : 0) | (rowWidths ? WidthsFlag : 0) | (sheet ? SheetFlag : 0));
		// The core allowed this; one array of a column's values is a .NET limit it doesn't know of.
		foreach (var column in Plan)
		{
			var bytes = (long)batchRows * column.ValueSize;
			if (bytes > Array.MaxLength)
				throw new ArgumentOutOfRangeException(nameof(batchRows), batchRows, $"batchRows asks for {bytes} bytes in one array, past the {Array.MaxLength} .NET holds in one.");
		}
		RowWidths = rowWidths ? GC.AllocateUninitializedArray<uint>(batchRows, pinned: true) : null;
		for (var index = 0; index < Plan.Length; index++)
		{
			var column = Plan[index];
			Values[index] = GC.AllocateUninitializedArray<byte>(batchRows * column.ValueSize, pinned: true);
			Verdicts[index] = GC.AllocateUninitializedArray<CellVerdict>(batchRows, pinned: true);
			if (Notes is not null)
				Notes[index] = GC.AllocateUninitializedArray<Native.RawCellNote>(batchRows, pinned: true);
			Buffers[index] = new Native.RawColumnBufferWith
			{
				Size = (uint)sizeof(Native.RawColumnBufferWith),
				Values = Address(Values[index]),
				Verdicts = Address(Verdicts[index]),
				Notes = Notes is null ? null : Address(Notes[index]),
			};
		}
	}

	/// <summary>A pinned copy of <paramref name="list"/>, held as long as the columns are, at an address that is never null.</summary>
	byte* Held(ReadOnlySpan<byte> list)
	{
		var pinned = Pinned(list);
		_lists.Add(pinned);
		return Address(pinned);
	}

	/// <summary>A copy of <paramref name="list"/> on the pinned object heap.</summary>
	internal static byte[] Pinned(ReadOnlySpan<byte> list)
	{
		var pinned = GC.AllocateUninitializedArray<byte>(list.Length, pinned: true);
		list.CopyTo(pinned);
		return pinned;
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

	/// <summary>HyperCast's <see cref="DateOptions"/> in the core's layout, validated by HyperCast.</summary>
	static RawDateOptions ToRaw(DateOptions options, int column)
	{
		try
		{
			options.TryToRaw(out var raw);
			return raw;
		}
		catch (ArgumentException invalid)
		{
			throw new ArgumentException($"Plan column {column} declares DateOptions no door accepts; the inner exception says why.", "plan", invalid);
		}
	}
}

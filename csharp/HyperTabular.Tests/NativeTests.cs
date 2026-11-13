using System.Globalization;
using System.Reflection;
using System.Runtime.InteropServices;
using System.Text.RegularExpressions;
using HyperCast;
using HyperCast.Interop;

namespace HyperTabular.Tests;

/// <summary>
/// What no read reaches: the 1.0 exports the ABI promises and no read path takes, the shapes the core
/// reads, and Mono's limit of twelve integer arguments.
/// </summary>
public sealed unsafe class NativeTests
{
	const int _capacity = 64;
	static readonly NumFormat _indian = NumFormat.Invariant with { Grouping = NumGrouping.Indian };

	/// <summary>Native memory a 1.0 fill writes: three columns' values, their verdicts and a shared arena.</summary>
	sealed class Columns10
	{
		public readonly long[][] Values;
		public readonly CellVerdict[][] Verdicts;
		public readonly Native.RawColumnBuffer[] Buffers;
		public readonly Native.RawColumnSpec[] Specs;

		public Columns10(params (uint Ordinal, Door Door, RawNumFormat Format)[] plan)
		{
			Values = new long[plan.Length][];
			Verdicts = new CellVerdict[plan.Length][];
			Buffers = new Native.RawColumnBuffer[plan.Length];
			Specs = new Native.RawColumnSpec[plan.Length];
			for (var index = 0; index < plan.Length; index++)
			{
				Values[index] = GC.AllocateArray<long>(_capacity, pinned: true);
				Verdicts[index] = GC.AllocateArray<CellVerdict>(_capacity, pinned: true);
				Buffers[index] = new Native.RawColumnBuffer
				{
					Values = Unsafe(Values[index]),
					Verdicts = (CellVerdict*)Unsafe(Verdicts[index]),
				};
				Specs[index] = new Native.RawColumnSpec { Ordinal = plan[index].Ordinal, Door = (uint)plan[index].Door, Format = plan[index].Format };
			}
		}
	}

	static void* Unsafe<T>(T[] pinned) where T : unmanaged => System.Runtime.CompilerServices.Unsafe.AsPointer(ref MemoryMarshal.GetArrayDataReference(pinned));

	static Native.State[] NewState() =>
		GC.AllocateArray<Native.State>((int)((Native.hypertabular_delimited_state_size() + 63) / 64), pinned: true);

	/// <summary>A whole input read through the 1.0 calls.</summary>
	static (Native.RawFilled Filled, byte[] Arena, Columns10 Columns) Fill10(byte[] input, char separator, Columns10 columns)
	{
		var state = NewState();
		var dialect = new Native.RawDialect { Separator = (byte)separator, Quoting = 1, SkipBlankLines = 1 };
		fixed (Native.State* at = state)
			Native.hypertabular_delimited_init(at, &dialect).ShouldBe(Native.Ok);
		var arena = GC.AllocateArray<byte>(4096, pinned: true);
		var cells = GC.AllocateArray<Native.RawSpan>(_capacity, pinned: true);
		var buffers = new Native.RawBuffers
		{
			Arena = (byte*)Unsafe(arena),
			ArenaCap = (nuint)arena.Length,
			Cells = (Native.RawSpan*)Unsafe(cells),
			CellsCap = (nuint)cells.Length,
		};
		Native.RawFilled filled;
		fixed (Native.State* at = state)
		fixed (byte* text = input)
		fixed (Native.RawColumnSpec* specs = columns.Specs)
		fixed (Native.RawColumnBuffer* outputs = columns.Buffers)
			Native.hypertabular_delimited_fill(at, text, (nuint)input.Length, 1, specs, outputs, (nuint)columns.Specs.Length, _capacity, &buffers, &filled)
				.ShouldBe(Native.Ok);
		return (filled, arena, columns);
	}

	/// <summary>A policy is the one thing 1.0 cannot be told: its fill reads the low 8 flag bits, so every format is Western there, and a read of a Western format says the same of every cell.</summary>
	[Fact]
	void The_delimited_1_0_exports_read_what_a_read_does()
	{
		var input = "1;a b;2.5\n12x;c;x\n1,23,456;d;4\n"u8.ToArray();
		var dialect = new Dialect(';', HasHeader: false) { Notes = false };
		var invariant = NumFormat.Invariant.ToRaw();
		var (filled, _, columns) = Fill10(input, ';', new Columns10((0, Door.Int32, invariant), (1, Door.Text, default), (2, Door.Double, invariant)));
		filled.Rows.ShouldBe(3UL);

		using var reader = new DelimitedReader(input.AsMemory(), dialect, [Column.Int32(0), Column.Text(1), Column.Double(2)]);
		var batch = reader.Read().ShouldNotBeNull();
		batch.Rows.ShouldBe(3);
		MemoryMarshal.Cast<long, int>(columns.Values[0])[..3].ToArray().ShouldBe([1, 0, 0]);
		batch.Verdicts(0)[0].IsOk.ShouldBeTrue();
		batch.Verdicts(0)[2].Reason.ShouldBe(CastFailure.Malformed);
		for (var column = 0; column < 3; column++)
			columns.Verdicts[column].AsSpan(0, 3).ToArray().ShouldBe(batch.Verdicts(column).ToArray());
		MemoryMarshal.Cast<long, double>(columns.Values[2])[..3].ToArray().ShouldBe(batch.Values<double>(2).ToArray());
		var spans = MemoryMarshal.Cast<long, Native.RawSpan>(columns.Values[1]);
		for (var row = 0; row < 3; row++)
		{
			batch.TryGetText(1, row, out var text).ShouldBeTrue();
			input.AsSpan((int)spans[row].Offset, spans[row].Length).SequenceEqual(text).ShouldBeTrue();
		}
	}

	/// <summary>Every word, the retired one and Indian's included, reads Western through 1.0: 1,234,567 taken and 1,23,456 refused.</summary>
	[Theory]
	[InlineData(351u)]
	[InlineData(607u)]
	[InlineData(863u)]
	void A_delimited_1_0_fill_reads_every_word_Western(uint flags)
	{
		var format = NumFormat.Invariant.ToRaw();
		format.Flags = flags;
		var (filled, _, columns) = Fill10("\"1,234,567\"\n\"1,23,456\"\n"u8.ToArray(), ',', new Columns10((0, Door.Int32, format)));
		filled.Rows.ShouldBe(2UL);
		columns.Verdicts[0][0].IsOk.ShouldBeTrue();
		MemoryMarshal.Cast<long, int>(columns.Values[0])[0].ShouldBe(1234567);
		columns.Verdicts[0][1].Reason.ShouldBe(CastFailure.Malformed);
	}

	/// <summary>A size-tagged fill told the retired word, past the binding's refusal, is a contract error before any cell; told Western or Indian, it reads.</summary>
	[Theory]
	[InlineData(351u, Native.ErrContract, 0UL)]
	[InlineData(863u, Native.ErrContract, 0UL)]
	[InlineData(95u, Native.Ok, 1UL)]
	[InlineData(607u, Native.Ok, 1UL)]
	void A_stale_word_past_the_binding_is_the_core_s_contract_error(uint flags, int code, ulong rows)
	{
		var state = NewState();
		var options = new Native.RawDelimitedOptions { Size = (uint)sizeof(Native.RawDelimitedOptions), Separator = (byte)',', Quoting = 1, SkipBlankLines = 1 };
		Native.RawFilled filled;
		fixed (Native.State* at = state)
			Native.hypertabular_delimited_init_with(at, &options).ShouldBe(Native.Ok);
		var format = NumFormat.Invariant.ToRaw();
		format.Flags = flags;
		var spec = new Native.RawColumnSpecWith { Size = (uint)sizeof(Native.RawColumnSpecWith), Door = (uint)Door.Int32, Format = format };
		var values = GC.AllocateArray<int>(_capacity, pinned: true);
		var verdicts = GC.AllocateArray<CellVerdict>(_capacity, pinned: true);
		var column = new Native.RawColumnBufferWith
		{
			Size = (uint)sizeof(Native.RawColumnBufferWith),
			Values = Unsafe(values),
			Verdicts = (CellVerdict*)Unsafe(verdicts),
		};
		var arena = GC.AllocateArray<byte>(4096, pinned: true);
		var cells = GC.AllocateArray<Native.RawSpan>(_capacity, pinned: true);
		var buffers = new Native.RawBuffersWith
		{
			Size = (uint)sizeof(Native.RawBuffersWith),
			Arena = (byte*)Unsafe(arena),
			ArenaCap = (nuint)arena.Length,
			Cells = (Native.RawSpan*)Unsafe(cells),
			CellsCap = (nuint)cells.Length,
		};
		var input = "\"1,234\"\n"u8.ToArray();
		fixed (Native.State* at = state)
		fixed (byte* text = input)
			Native.hypertabular_delimited_fill_with(at, text, (nuint)input.Length, 1, &spec, &column, 1, _capacity, &buffers, &filled).ShouldBe(code);
		if (code == Native.Ok)
		{
			filled.Rows.ShouldBe(rows);
			values[0].ShouldBe(1234);
		}
	}

	[Fact]
	void The_workbook_1_0_exports_read_what_a_read_does()
	{
		using var book = Workbook.Open(Path.Combine(CorpusTests._corpusDirectory, "workbook", "basic.xlsx"));
		var info = book.Sheets[0];
		var state = GC.AllocateArray<ulong>(book.State.Length, pinned: true);
		book.State.AsSpan().CopyTo(state);
		Native.RawFilled filled;
		fixed (byte* part = info.Part)
			Native.hypertabular_workbook_sheet(Unsafe(state), book.Container, book.Length, part, (nuint)info.Part.Length, info.Index, 1, 1, &filled).ShouldBe(Native.Ok);

		var scratch = new Workbook.Scratch(1 << 20, 1 << 20, 4096, 64);
		var buffers = scratch.Buffers(book);
		Native.hypertabular_workbook_header(Unsafe(state), book.Container, book.Length, &buffers, &filled).ShouldBe(Native.Ok);

		var columns = new Columns10((0, Door.Text, default), (1, Door.Text, default));
		fixed (Native.RawColumnSpec* specs = columns.Specs)
		fixed (Native.RawColumnBuffer* outputs = columns.Buffers)
			Native.hypertabular_workbook_fill(Unsafe(state), book.Container, book.Length, specs, outputs, 2, 16, &buffers, &filled).ShouldBe(Native.Ok);

		var sheet = book.Sheet(0, new SheetOptions(BatchRows: 16) { Notes = false }, [Column.Text(0), Column.Text(1)]);
		var batch = sheet.Read().ShouldNotBeNull();
		((int)filled.Rows).ShouldBeGreaterThan(0);
		((int)filled.Rows).ShouldBe(batch.Rows);
		for (var column = 0; column < 2; column++)
		{
			var spans = MemoryMarshal.Cast<long, Native.RawSpan>(columns.Values[column]);
			columns.Verdicts[column].AsSpan(0, batch.Rows).ToArray().ShouldBe(batch.Verdicts(column).ToArray());
			for (var row = 0; row < batch.Rows; row++)
			{
				if (!batch.TryGetText(column, row, out var text))
					continue;
				var said = spans[row].Flagged ? scratch.Arena.AsSpan((int)spans[row].Offset, spans[row].Length) : book.Strings.AsSpan((int)spans[row].Offset, spans[row].Length);
				said.SequenceEqual(text).ShouldBeTrue($"column {column}, row {row}");
			}
		}
	}

	/// <summary>What the header says of each shape's size, held against this assembly's own struct, by the header's name for it.</summary>
	[Fact]
	void Every_shape_is_the_size_the_header_says()
	{
		var path = Path.Combine(CorpusTests._corpusDirectory, "..", "include", "hypertabular.h");
		if (!File.Exists(path))
			Assert.Skip("include/ isn't in this tree: the Alpine run copies csharp/ and corpus/ only, and every other leg holds the sizes to the header.");
		var header = File.ReadAllText(path);
		int Said(string name)
		{
			var match = Regex.Match(header, $@"#define HyperTabular{name}_SIZE (\d+)");
			match.Success.ShouldBeTrue(name);
			return int.Parse(match.Groups[1].Value, CultureInfo.InvariantCulture);
		}
		Said("RawDelimitedOptions").ShouldBe(sizeof(Native.RawDelimitedOptions));
		Said("RawDialect").ShouldBe(sizeof(Native.RawDialect));
		Said("RawTranscoded").ShouldBe(sizeof(Native.RawTranscoded));
		Said("Span").ShouldBe(sizeof(Native.RawSpan));
		Said("Filled").ShouldBe(sizeof(Native.RawFilled));
		Said("Failure").ShouldBe(sizeof(Native.RawFailure));
		Said("ColumnSpec").ShouldBe(sizeof(Native.RawColumnSpec));
		Said("CellNote").ShouldBe(sizeof(Native.RawCellNote));
		Said("RawProblem").ShouldBe(sizeof(Native.RawProblem));
		Said("Slot").ShouldBe(sizeof(Native.RawSlot));
		Said("Opened").ShouldBe(sizeof(Native.RawOpened));
		Said("RawNumFormat").ShouldBe(sizeof(RawNumFormat));
		Said("RawDateOptions").ShouldBe(sizeof(RawDateOptions));
		Said("RawSheetOptions").ShouldBe(sizeof(Native.RawSheetOptions));
		Said("RawRange").ShouldBe(sizeof(Native.RawRange));
		Said("RawSuggestion").ShouldBe(sizeof(Native.RawSuggestion));
		Said("CellVerdict").ShouldBe(sizeof(CellVerdict));
		Said("RawConstraints").ShouldBe(sizeof(RawConstraints));
		Said("RawCheck").ShouldBe(sizeof(RawCheck));
		// A sheet's entry is read as three spans: name, part, and flags and index in the third's two words.
		Said("SheetInfo").ShouldBe(3 * sizeof(Native.RawSpan));

		// Those that hold pointers are a sizeof in the header: the core's asserts say what they come to.
		var word = IntPtr.Size;
		sizeof(Native.RawColumnSpecWith).ShouldBe(120 + 7 * word);
		sizeof(Native.RawColumnBufferWith).ShouldBe(5 * word);
		sizeof(Native.RawBuffersWith).ShouldBe(18 * word);
		Marshal.OffsetOf<Native.RawColumnSpecWith>(nameof(Native.RawColumnSpecWith.Date)).ToInt32().ShouldBe(108);
		Marshal.OffsetOf<Native.RawColumnSpecWith>(nameof(Native.RawColumnSpecWith.Months)).ToInt32().ShouldBe(120);
		Marshal.OffsetOf<Native.RawColumnSpecWith>(nameof(Native.RawColumnSpecWith.Tokens)).ToInt32().ShouldBe(120 + 2 * word);
		Marshal.OffsetOf<Native.RawColumnSpecWith>(nameof(Native.RawColumnSpecWith.Constraints)).ToInt32().ShouldBe(120 + 4 * word);
		Marshal.OffsetOf<Native.RawColumnSpecWith>(nameof(Native.RawColumnSpecWith.EnumLen)).ToInt32().ShouldBe(120 + 6 * word);
		Marshal.OffsetOf<Native.RawColumnBufferWith>(nameof(Native.RawColumnBufferWith.Notes)).ToInt32().ShouldBe(3 * word);
		Marshal.OffsetOf<Native.RawColumnBufferWith>(nameof(Native.RawColumnBufferWith.Checks)).ToInt32().ShouldBe(4 * word);
		Marshal.OffsetOf<Native.RawBuffersWith>(nameof(Native.RawBuffersWith.Tokens)).ToInt32().ShouldBe(15 * word);
		Marshal.OffsetOf<Native.RawBuffersWith>(nameof(Native.RawBuffersWith.RowWidths)).ToInt32().ShouldBe(17 * word);
		Marshal.OffsetOf<Native.RawProblem>(nameof(Native.RawProblem.Row)).ToInt32().ShouldBe(0);
		Marshal.OffsetOf<Native.RawProblem>(nameof(Native.RawProblem.Keyword)).ToInt32().ShouldBe(32);
		Marshal.OffsetOf<Native.RawSheetOptions>(nameof(Native.RawSheetOptions.LastColumn)).ToInt32().ShouldBe(32);
	}

	/// <summary>
	/// Mono's interpreter, which runs .NET in the browser and in an iOS debug build, passes no more than
	/// twelve integer arguments to a native function. Every declaration of every export is counted, for
	/// each of its three library names.
	/// </summary>
	[Fact]
	void No_export_takes_more_than_twelve_arguments()
	{
		var imports = typeof(Native).GetMethods(BindingFlags.Static | BindingFlags.NonPublic)
			.Select(method => (method, attribute: method.GetCustomAttribute<LibraryImportAttribute>()))
			.Where(entry => entry.attribute is not null)
			.ToList();
		var byExport = imports.GroupBy(entry => entry.attribute!.EntryPoint!).ToList();
		byExport.Count.ShouldBe(23);
		foreach (var export in byExport)
		{
			string.Join(' ', export.Select(entry => entry.attribute!.LibraryName).Order(StringComparer.Ordinal))
				.ShouldBe("* __Internal hypertabular", export.Key);
			foreach (var (method, _) in export)
				method.GetParameters().Length.ShouldBeLessThanOrEqualTo(MostIntegerArguments, method.Name);
		}
		// The biggest there is, so that the count above can't pass on nothing.
		byExport.Max(export => export.First().method.GetParameters().Length).ShouldBe(10);
	}

	const int MostIntegerArguments = 12;
}

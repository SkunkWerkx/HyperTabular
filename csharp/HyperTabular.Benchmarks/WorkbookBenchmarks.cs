using BenchmarkDotNet.Attributes;
using HyperCast;

namespace HyperTabular.Benchmarks;

/// <summary>
/// The workbook reader over the two real-application files of <c>corpus/README.md</c> —
/// Excel's <c>excel-win-300k.xlsx</c> and LibreOffice's <c>libreoffice-300k.ods</c>, 300 000
/// rows × 8 columns each — read whole through the plan <c>rust/benches/workbook_benchmarks.rs</c>
/// reads them through, so that this binding's number sits beside the core's: <c>Open</c> is
/// the package opened and nothing read; <c>Read</c> is opened, then the first sheet read in
/// batches of 4096, every column's verdicts looked at and every text cell's bytes — the
/// checksum every binding's benchmark arrives at, which says they all did the same work.
/// <c>ReadStrings</c> is <c>Read</c> with the text column made into strings, the price of a
/// <see cref="string"/> a cell.
/// </summary>
/// <remarks>
/// The files stay out of the tree (<c>corpus/generate/out/</c>, sha256 in the README's
/// table); <c>HYPERTABULAR_BENCH_DIR</c> names another directory holding them.
/// </remarks>
[MemoryDiagnoser]
public class WorkbookBenchmarks
{
	static readonly Column[] Plan =
	[
		Column.Int64(0),
		Column.Double(1),
		Column.Text(2),
		Column.Date(3),
		Column.Time(4),
		Column.Boolean(5),
		Column.Duration(6),
		Column.Int64(7),
	];

	byte[] _container = [];

	[Params("excel-win-300k.xlsx", "libreoffice-300k.ods")]
	public string File { get; set; } = "";

	[GlobalSetup]
	public void Setup()
	{
		var directory = Environment.GetEnvironmentVariable("HYPERTABULAR_BENCH_DIR")
			?? Path.Combine(AppContext.BaseDirectory, "../../../../../corpus/generate/out");
		_container = System.IO.File.ReadAllBytes(Path.Combine(directory, File));
		Console.WriteLine($"// {File}: checksum {Read()}");
	}

	[Benchmark]
	public int Open()
	{
		using var book = new Workbook(_container);
		return book.Sheets.Count;
	}

	[Benchmark]
	public long Read()
	{
		using var book = new Workbook(_container);
		var sheet = book.Sheet(0, SheetOptions.Default, Plan);
		long checksum = 0;
		while (sheet.Read() is { } batch)
		{
			for (var column = 0; column < Plan.Length; column++)
				foreach (var verdict in batch.Verdicts(column))
					if (verdict.IsOk)
						checksum++;
			for (var row = 0; row < batch.Rows; row++)
				if (batch.TryGetText(2, row, out var utf8))
					checksum += utf8.Length;
		}
		return checksum;
	}

	[Benchmark]
	public long ReadStrings()
	{
		using var book = new Workbook(_container);
		var sheet = book.Sheet(0, SheetOptions.Default, Plan);
		long checksum = 0;
		while (sheet.Read() is { } batch)
		{
			for (var column = 0; column < Plan.Length; column++)
				foreach (var verdict in batch.Verdicts(column))
					if (verdict.IsOk)
						checksum++;
			for (var row = 0; row < batch.Rows; row++)
				checksum += batch.GetString(2, row)?.Length ?? 0;
		}
		return checksum;
	}
}

using System.Text.Json;
using HyperCast;

namespace HyperTabular.Tests;

/// <summary>
/// Replays <c>corpus/workbook.json</c> — the contract every binding replays, and the one the
/// Rust binding replays — through this binding: each package opened from memory and from its
/// path, each sheet read by index and (where the name finds it) by name, in batches of one
/// row, of two, and of more than any sheet has.
/// </summary>
public sealed class WorkbookCorpusTests
{
	static string FailureName(TabularFailure failure) =>
		failure switch
		{
			TabularFailure.UnclosedQuote => "unclosed_quote",
			TabularFailure.ColumnCount => "column_count",
			TabularFailure.RowTooLong => "row_too_long",
			TabularFailure.NotAZip => "not_a_zip",
			TabularFailure.Container => "container",
			TabularFailure.Encrypted => "encrypted",
			TabularFailure.Method => "method",
			TabularFailure.MissingPart => "missing_part",
			TabularFailure.Xml => "xml",
			TabularFailure.Deflate => "deflate",
			TabularFailure.NotAWorkbook => "not_a_workbook",
			TabularFailure.SharedString => "shared_string",
			TabularFailure.TooLarge => "too_large",
			_ => throw new InvalidOperationException($"no corpus name for {failure}"),
		};

	static void AssertFailure(string label, TabularException? actual, JsonElement @case)
	{
		if (!@case.TryGetProperty("failure", out var expected) || expected.ValueKind == JsonValueKind.Null)
		{
			actual.ShouldBeNull(label);
			return;
		}
		actual.ShouldNotBeNull(label);
		FailureName(actual.Failure).ShouldBe(expected.GetProperty("kind").GetString(), label);
		actual.Record.ShouldBe(expected.GetProperty("record").GetInt64(), label);
		actual.Line.ShouldBe(expected.GetProperty("line").GetInt32(), label);
		actual.Byte.ShouldBe(expected.GetProperty("byte").GetInt64(), label);
		if (expected.TryGetProperty("expected", out var count))
			(actual.Expected, actual.Found).ShouldBe((count.GetInt32(), expected.GetProperty("found").GetInt32()), label);
	}

	[Fact]
	void Workbook_corpus() => Replay();

	/// <summary>
	/// The corpus again with every buffer starting at one element and never asked to be large
	/// up front: each read stops wherever the core runs out of room and resumes in a grown
	/// buffer that has to have kept what the old one held. A grow that dropped it would read
	/// garbage, and the corpus would say so.
	/// </summary>
	[Fact]
	void Workbook_corpus_with_buffers_that_start_with_no_room()
	{
		Workbook.Stingy = true;
		Workbook.Grown = default;
		try
		{
			Replay();
		}
		finally
		{
			Workbook.Stingy = false;
		}
		var (window, arena, cells) = Workbook.Grown;
		// Not once per call: many times, mid-part, for each of the three.
		window.ShouldBeGreaterThan(1_000);
		arena.ShouldBeGreaterThan(1_000);
		cells.ShouldBeGreaterThan(1_000);
	}

	static void Replay()
	{
		var corpus = CorpusTests.Corpus("workbook.json");
		corpus.Length.ShouldBeGreaterThanOrEqualTo(80);
		var cells = 0;
		foreach (var @case in corpus)
		{
			var name = @case.GetProperty("name").GetString()!;
			var path = Path.Combine(CorpusTests._corpusDirectory, @case.GetProperty("file").GetString()!);
			var bytes = File.ReadAllBytes(path);

			// A package the core refuses: every way of opening it gives the one failure.
			if (!@case.TryGetProperty("sheet", out var sheetIndex))
			{
				AssertFailure($"{name} (memory)", Should.Throw<TabularException>(() => new Workbook(bytes)), @case);
				AssertFailure($"{name} (path)", Should.Throw<TabularException>(() => Workbook.Open(path)), @case);
				continue;
			}

			Column[] plan = [.. @case.GetProperty("plan").EnumerateArray().Select(CorpusTests.ColumnOf)];
			var index = sheetIndex.GetInt32();
			var sheetName = @case.GetProperty("sheets")[index].GetProperty("name").GetString()!;
			var settings = @case.GetProperty("options");
			var rows = @case.GetProperty("rows");
			var numbers = @case.GetProperty("numbers");
			cells += rows.GetArrayLength() * plan.Length;

			foreach (var (source, open) in new (string, Func<Workbook>)[]
			{
				("memory", () => new Workbook(bytes)),
				("path", () => Workbook.Open(path)),
			})
			{
				using var book = open();
				book.Format.ShouldBe(@case.GetProperty("format").GetString() == "xlsx" ? WorkbookFormat.Xlsx : WorkbookFormat.Ods, name);
				book.DateSystem.ShouldBe((ExcelEpoch)@case.GetProperty("epoch").GetInt32(), name);
				book.Sheets.Select(sheet => (sheet.Name, sheet.Hidden)).ShouldBe(
					@case.GetProperty("sheets").EnumerateArray()
						.Select(sheet => (sheet.GetProperty("name").GetString()!, sheet.GetProperty("hidden").GetBoolean())), name);
				var byName = book.Sheets.Select(sheet => sheet.Name).ToList().IndexOf(sheetName) == index;

				foreach (var batchRows in (int[])[1, 2, 1024])
				{
					var options = new SheetOptions(
						settings.GetProperty("has_header").GetBoolean(),
						settings.GetProperty("skip_empty_rows").GetBoolean(),
						batchRows);
					foreach (var named in (bool[])[false, true])
					{
						if (named && !byName)
							continue;
						var label = $"{name}: {source}, {batchRows} rows a batch{(named ? ", by name" : "")}";
						var sheet = named ? book.Sheet(sheetName, options, plan) : book.Sheet(index, options, plan);
						var header = @case.GetProperty("header");
						if (header.ValueKind == JsonValueKind.Null)
							sheet.Header.ShouldBeNull(label);
						else
							sheet.Header.ShouldBe(header.EnumerateArray().Select(entry => entry.GetString()!), label);

						var seen = 0;
						TabularException? failure = null;
						try
						{
							while (sheet.Read() is { } batch)
							{
								batch.Rows.ShouldBeInRange(1, batchRows, label);
								for (var row = 0; row < batch.Rows; row++, seen++)
								{
									seen.ShouldBeLessThan(rows.GetArrayLength(), label);
									batch.Line(row).ShouldBe(numbers[seen].GetInt32(), $"{label}, row {seen}");
									for (var column = 0; column < plan.Length; column++)
										CorpusTests.AssertCell($"{label}, row {seen}, column {column}", batch, column, row, rows[seen][column]);
								}
							}
						}
						catch (TabularException e)
						{
							failure = e;
							// A failed sheet stays failed: the same failure, again.
							Should.Throw<TabularException>(() => sheet.Read()).ShouldBeSameAs(e, label);
						}
						seen.ShouldBe(rows.GetArrayLength(), label);
						AssertFailure(label, failure, @case);
					}
				}
			}
		}
		cells.ShouldBeGreaterThanOrEqualTo(12_000);
	}

	[Fact]
	void What_is_not_there_is_an_exception_of_its_own()
	{
		using var book = Workbook.Open(Path.Combine(CorpusTests._corpusDirectory, "workbook", "basic.xlsx"));
		Column[] plan = [Column.Text(0)];
		Should.Throw<KeyNotFoundException>(() => book.Sheet("No such sheet", SheetOptions.Default, plan));
		Should.Throw<ArgumentOutOfRangeException>(() => book.Sheet(99, SheetOptions.Default, plan));
		Should.Throw<ArgumentOutOfRangeException>(() => book.Sheet(0, default, plan));
		Should.Throw<DirectoryNotFoundException>(() => Workbook.Open("/no/such/file.xlsx"));
		Should.Throw<FileNotFoundException>(() => Workbook.Open(Path.Combine(CorpusTests._corpusDirectory, "no-such-file.xlsx")));
		Should.Throw<TabularException>(() => new Workbook("not a zip at all"u8.ToArray()))
			.Failure.ShouldBe(TabularFailure.NotAZip);

		// Two sheets read at once, each with its own buffers, over one workbook.
		var first = book.Sheet(0, SheetOptions.Default, plan);
		var second = book.Sheet(0, SheetOptions.Default with { HasHeader = false }, plan);
		var a = first.Read()!.Rows;
		var b = second.Read()!.Rows;
		(a + 1).ShouldBe(b, "the header is one row more");

		book.Dispose();
		Should.Throw<ObjectDisposedException>(() => first.Read());
	}
}

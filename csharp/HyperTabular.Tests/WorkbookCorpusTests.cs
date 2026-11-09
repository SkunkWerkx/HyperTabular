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
		Batch.Stingy = true;
		Workbook.Grown = default;
		try
		{
			Replay();
		}
		finally
		{
			Workbook.Stingy = false;
			Batch.Stingy = false;
		}
		var (window, arena, cells) = Workbook.Grown;
		// Not once per call: many times, mid-part, for each of the three.
		window.ShouldBeGreaterThan(1_000);
		arena.ShouldBeGreaterThan(1_000);
		cells.ShouldBeGreaterThan(1_000);
	}

	/// <summary>
	/// Every way a workbook is opened: from memory, from its path, and from a stream — a
	/// <see cref="MemoryStream"/> the workbook disposes, a non-seekable one it leaves open, and
	/// one read asynchronously (completing synchronously, so that the thread-static switches
	/// the stingy pass sets hold throughout) — each checked, once it has been read, for whether the stream
	/// was disposed as asked.
	/// </summary>
	static (string Source, Func<ValueTask<Workbook>> Open, Action<string> Check)[] Sources(string path, byte[] bytes)
	{
		MemoryStream? owned = null;
		ShortReadStream? kept = null;
		ShortReadStream? awaited = null;
		return
		[
			("memory", () => new(new Workbook(bytes)), _ => { }),
			("path", () => new(Workbook.Open(path)), _ => { }),
			("stream", () => new(new Workbook(owned = new MemoryStream(bytes))),
				label => owned!.CanRead.ShouldBeFalse($"{label}: the stream is disposed once read")),
			("non-seekable stream, left open", () => new(Workbook.Open(kept = new ShortReadStream(bytes, 4096), leaveOpen: true)),
				label => kept!.Disposed.ShouldBeFalse($"{label}: the stream is left open")),
			("stream read asynchronously", () => Workbook.OpenAsync(awaited = new ShortReadStream(bytes, 4096) { Yields = false }),
				label => awaited!.DisposedAsynchronously.ShouldBeTrue($"{label}: the stream is disposed asynchronously once read")),
		];
	}

	/// <summary>A workbook every source opens synchronously, the asynchronous one included: the replay stays on the thread its switches are set on.</summary>
	static Workbook Now(ValueTask<Workbook> open) =>
		open.IsCompleted ? open.Result : throw new InvalidOperationException("a source did not complete synchronously");

	static void Replay()
	{
		var corpus = CorpusTests.Corpus("workbook.json");
		corpus.Length.ShouldBeGreaterThanOrEqualTo(398);
		CorpusKeys.UnknownWorkbook(corpus).ShouldBeEmpty();
		var cells = 0;
		foreach (var @case in corpus)
		{
			var name = @case.GetProperty("name").GetString()!;
			var path = Path.Combine(CorpusTests._corpusDirectory, @case.GetProperty("file").GetString()!);
			var bytes = File.ReadAllBytes(path);

			// A package the core refuses: every way of opening it gives the one failure.
			if (!@case.TryGetProperty("sheet", out var sheetIndex))
			{
				foreach (var (source, open, check) in Sources(path, bytes))
				{
					CorpusTests.AssertFailure($"{name} ({source})", Should.Throw<TabularException>(() => Now(open())), @case);
					check($"{name} ({source})");
				}
				continue;
			}

			Column[] plan = [.. @case.GetProperty("plan").EnumerateArray().Select(CorpusTests.ColumnOf)];
			var index = sheetIndex.GetInt32();
			var sheetName = @case.GetProperty("sheets")[index].GetProperty("name").GetString()!;
			var settings = @case.GetProperty("options");
			var rows = @case.GetProperty("rows");
			var numbers = @case.GetProperty("numbers");
			cells += rows.GetArrayLength() * plan.Length;

			foreach (var (source, open, check) in Sources(path, bytes))
			{
				using var book = Now(open());
				check($"{name} ({source})");
				book.Format.ShouldBe(@case.GetProperty("format").GetString() switch
				{
					"xlsx" => WorkbookFormat.Xlsx,
					"numbers" => WorkbookFormat.Numbers,
					_ => WorkbookFormat.Ods,
				}, name);
				book.DateSystem.ShouldBe((ExcelEpoch)@case.GetProperty("epoch").GetInt32(), name);
				book.Sheets.Select(sheet => (sheet.Name, sheet.Hidden)).ShouldBe(
					@case.GetProperty("sheets").EnumerateArray()
						.Select(sheet => (sheet.GetProperty("name").GetString()!, sheet.GetProperty("hidden").GetBoolean())), name);
				var byName = book.Sheets.Select(sheet => sheet.Name).ToList().IndexOf(sheetName) == index;

				foreach (var batchRows in (int[])[1, 2, 1024])
				{
					var options = OptionsOf(settings) with { BatchRows = batchRows };
					foreach (var (named, headerFirst) in ((bool, bool)[])[(false, false), (true, false), (false, true), (true, true)])
					{
						if (named && !byName)
							continue;
						var label = $"{name}: {source}, {batchRows} rows a batch{(named ? ", by name" : "")}{(headerFirst ? ", header first" : "")}";
						var sheet = headerFirst
							? named ? book.Sheet(sheetName, options) : book.Sheet(index, options)
							: named ? book.Sheet(sheetName, options, plan) : book.Sheet(index, options, plan);
						var header = @case.GetProperty("header");
						if (header.ValueKind == JsonValueKind.Null)
							sheet.Header.ShouldBeNull(label);
						else
							sheet.Header.ShouldBe(header.EnumerateArray().Select(entry => entry.GetString()!), label);
						if (headerFirst)
						{
							sheet.IsBound.ShouldBeFalse(label);
							sheet.Plan.ShouldBeEmpty(label);
							Should.Throw<InvalidOperationException>(() => sheet.Read(), label);
							sheet.Bind(plan);
							Should.Throw<InvalidOperationException>(() => sheet.Bind(plan), label);
						}
						sheet.IsBound.ShouldBeTrue(label);

						var seen = 0;
						var problems = new List<string>();
						TabularException? failure = null;
						try
						{
							while (sheet.Read() is { } batch)
							{
								batch.Rows.ShouldBeInRange(1, batchRows, label);
								CorpusTests.AssertRows(label, batch);
								CorpusTests.AppendProblems(label, problems, batch, seen);
								for (var row = 0; row < batch.Rows; row++, seen++)
								{
									seen.ShouldBeLessThan(rows.GetArrayLength(), label);
									batch.Line(row).ShouldBe(numbers[seen].GetInt32(), $"{label}, row {seen}");
									rows[seen].GetArrayLength().ShouldBe(plan.Length, label);
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
						CorpusTests.AssertFailure(label, failure, @case);
						CorpusTests.AssertProblems(label, problems, @case, notes: true);
					}
				}
			}
		}
		cells.ShouldBeGreaterThanOrEqualTo(12_000);
	}

	/// <summary>A case's options, as the corpus says them.</summary>
	static SheetOptions OptionsOf(JsonElement settings)
	{
		var options = new SheetOptions(
			settings.GetProperty("has_header").GetBoolean(),
			settings.GetProperty("skip_empty_rows").GetBoolean());
		if (settings.TryGetProperty("skip", out var skip))
			options = options with { Skip = skip.GetUInt32() };
		if (settings.TryGetProperty("header_row", out var headerRow))
			options = options with { HeaderRow = headerRow.GetUInt32() };
		if (settings.TryGetProperty("range", out var range))
			options = options with { Range = SheetRange.Parse(range.GetString()!) };
		if (settings.TryGetProperty("tokens", out var tokens))
			options = options with { Tokens = CorpusTests.TokensOf(tokens) };
		return options;
	}

	[Fact]
	void Every_workbook_corpus_key_is_read() =>
		CorpusKeys.AJunkKeyIsSeenEverywhere("workbook.json", CorpusKeys.UnknownWorkbook);

	/// <summary>
	/// Every case read with notes off, once, from memory: refused cells keep their verdicts and lose
	/// their details, and the faults list loses uncached formulas.
	/// </summary>
	[Fact]
	void Workbook_corpus_with_notes_off()
	{
		foreach (var @case in CorpusTests.Corpus("workbook.json"))
		{
			if (!@case.TryGetProperty("sheet", out var sheetIndex))
				continue;
			var name = @case.GetProperty("name").GetString()!;
			using var book = Workbook.Open(Path.Combine(CorpusTests._corpusDirectory, @case.GetProperty("file").GetString()!));
			Column[] plan = [.. @case.GetProperty("plan").EnumerateArray().Select(CorpusTests.ColumnOf)];
			var sheet = book.Sheet(sheetIndex.GetInt32(), OptionsOf(@case.GetProperty("options")) with { Notes = false }, plan);
			var rows = @case.GetProperty("rows");
			var seen = 0;
			var problems = new List<string>();
			TabularException? failure = null;
			try
			{
				while (sheet.Read() is { } batch)
				{
					CorpusTests.AppendProblems(name, problems, batch, seen);
					for (var row = 0; row < batch.Rows; row++, seen++)
						for (var column = 0; column < plan.Length; column++)
							CorpusTests.AssertCell($"{name}, row {seen}, column {column}", batch, column, row, rows[seen][column], notes: false);
				}
			}
			catch (TabularException e)
			{
				failure = e;
			}
			seen.ShouldBe(rows.GetArrayLength(), name);
			CorpusTests.AssertFailure(name, failure, @case);
			CorpusTests.AssertProblems(name, problems, @case, notes: false);
		}
	}

	/// <summary>
	/// Every case that says what its sheet suggests: through the sheet itself, whose plan then has to
	/// read back refusing no cell.
	/// </summary>
	[Fact]
	void Every_workbook_suggestion_is_what_the_corpus_says()
	{
		var seen = 0;
		foreach (var @case in CorpusTests.Corpus("workbook.json"))
		{
			if (!@case.TryGetProperty("suggest", out var suggest))
				continue;
			seen++;
			var name = @case.GetProperty("name").GetString()!;
			using var book = Workbook.Open(Path.Combine(CorpusTests._corpusDirectory, @case.GetProperty("file").GetString()!));
			var index = @case.GetProperty("sheet").GetInt32();
			var options = OptionsOf(@case.GetProperty("options"));
			var found = book.Sheet(index, options).Suggest(CorpusTests.SuggestOptionsOf(suggest));
			CorpusTests.AssertSuggestion(name, found, suggest);
			AssertReadsBack(name, book.Sheet(index, options, [.. found.Plan]));
		}
		seen.ShouldBeGreaterThanOrEqualTo(15);
	}

	/// <summary>No cell of a sheet its plan was suggested from is refused.</summary>
	internal static void AssertReadsBack(string label, Sheet sheet)
	{
		while (sheet.Read() is { } batch)
			foreach (var problem in batch.Faults())
				if (problem.Reason is CastFailure.Malformed or CastFailure.OutOfRange)
					throw new ShouldAssertException($"{label}: read back refused {problem}");
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

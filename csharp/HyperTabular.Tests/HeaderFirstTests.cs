using System.Text;
using HyperCast;

namespace HyperTabular.Tests;

/// <summary>
/// The surface the corpus replays do not reach on their own: opening header-first and
/// binding by name, the row view, UTF-16 text without strings, workbooks from streams, and
/// reading asynchronously with cancellation.
/// </summary>
public sealed class HeaderFirstTests
{
	static readonly byte[] _countries = Encoding.UTF8.GetBytes(
		"Region Code,ISO-alpha2 Code,M49 Code,Region Code\n" +
		"150,FR,250,x\n" +
		"019,BR,076,y\n");

	static string Basic(string extension) => Path.Combine(CorpusTests._corpusDirectory, "workbook", $"basic.{extension}");

	[Fact]
	void A_header_resolves_names_to_ordinals_first_match_exact()
	{
		using var reader = new DelimitedReader(_countries.AsMemory(), Dialect.Csv);
		var header = reader.Header.ShouldNotBeNull();
		header.ShouldBe(["Region Code", "ISO-alpha2 Code", "M49 Code", "Region Code"]);
		header.Ordinal("M49 Code").ShouldBe(2);
		header.Ordinal("ISO-alpha2 Code"u8).ShouldBe(1);
		// A name that appears twice is the first.
		header.Ordinal("Region Code").ShouldBe(0);
		header.Ordinal("Region Code"u8).ShouldBe(0);
		// Exact: case, and space, count.
		header.TryOrdinal("m49 code", out var missing).ShouldBeFalse();
		missing.ShouldBe(-1);
		header.TryOrdinal(" M49 Code", out _).ShouldBeFalse();
		header.TryOrdinal("M49"u8, out _).ShouldBeFalse();
		Should.Throw<KeyNotFoundException>(() => header.Ordinal("Population")).Message.ShouldContain("Population");
		Should.Throw<KeyNotFoundException>(() => header.Ordinal("Population"u8)).Message.ShouldContain("Population");
		header.Utf8(1).SequenceEqual("ISO-alpha2 Code"u8).ShouldBeTrue();
		reader.ColumnCount.ShouldBe(4);
	}

	[Fact]
	void Open_read_the_header_bind_by_name_then_read()
	{
		using var reader = new DelimitedReader(_countries.AsMemory(), Dialect.Csv);
		reader.IsBound.ShouldBeFalse();
		reader.Plan.ShouldBeEmpty();
		// No plan, no read; and that is not final.
		Should.Throw<InvalidOperationException>(() => reader.Read());
		// A plan that cannot be honoured leaves the reader as it was.
		Should.Throw<ArgumentException>(() => reader.Bind([default]));
		reader.IsBound.ShouldBeFalse();

		var header = reader.Header!;
		reader.Bind([Column.Text(header.Ordinal("ISO-alpha2 Code")), Column.Int32(header.Ordinal("M49 Code"))], batchRows: 1);
		reader.IsBound.ShouldBeTrue();
		Should.Throw<InvalidOperationException>(() => reader.Bind([Column.Text(0)]));

		var codes = new List<(string, int)>();
		foreach (var row in reader.Rows())
			codes.Add((row.GetString(0)!, row.Get<int>(1) is Success<int> number ? number.Value : -1));
		codes.ShouldBe([("FR", 250), ("BR", 76)]);
	}

	[Fact]
	void Every_input_opens_header_first()
	{
		var path = Path.GetTempFileName();
		try
		{
			File.WriteAllBytes(path, _countries);
			foreach (var (source, open) in new (string, Func<DelimitedReader>)[]
			{
				("memory", () => new DelimitedReader(_countries.AsMemory(), Dialect.Csv)),
				("stream", () => new DelimitedReader(new MemoryStream(_countries), Dialect.Csv, bufferBytes: 3)),
				("path", () => DelimitedReader.Open(path, Dialect.Csv, bufferBytes: 3)),
			})
			{
				using var reader = open();
				reader.Bind([Column.Int32(reader.Header!.Ordinal("M49 Code"))]);
				reader.Read()!.Values<int>(0).ToArray().ShouldBe([250, 76], source);
			}
		}
		finally
		{
			File.Delete(path);
		}
	}

	[Fact]
	void A_source_with_no_header_binds_by_position()
	{
		using var reader = new DelimitedReader("1,a\n2,b\n"u8.ToArray().AsMemory(), Dialect.Csv with { HasHeader = false });
		reader.Header.ShouldBeNull();
		reader.ColumnCount.ShouldBeNull("the width is the first record's, and none has been read");
		reader.Bind([Column.Text(1), Column.Int64(0)]);
		var batch = reader.Read()!;
		reader.ColumnCount.ShouldBe(2);
		batch.GetString(0, 1).ShouldBe("b");
		batch.Values<long>(1).ToArray().ShouldBe([1L, 2L]);
	}

	[Fact]
	void A_sheet_opens_header_first_by_index_and_by_name()
	{
		using var book = Workbook.Open(Basic("xlsx"));
		foreach (var sheet in (Sheet[])[book.Sheet(0, SheetOptions.Default), book.Sheet("Data", SheetOptions.Default)])
		{
			sheet.IsBound.ShouldBeFalse();
			Should.Throw<InvalidOperationException>(() => sheet.Read());
			var header = sheet.Header!;
			sheet.Bind([Column.Text(header.Ordinal("name")), Column.Int64(header.Ordinal("id"))]);
			Should.Throw<InvalidOperationException>(() => sheet.Bind([Column.Text(0)]));
			var batch = sheet.Read()!;
			batch.Get<long>(1, 0).ShouldBe(1L);
			batch.GetString(0, 1).ShouldBe("bob, jr");
		}
		Should.Throw<KeyNotFoundException>(() => book.Sheet("No such sheet", SheetOptions.Default));
		Should.Throw<ArgumentOutOfRangeException>(() => book.Sheet(99, SheetOptions.Default));
	}

	[Fact]
	void A_sheet_of_a_disposed_workbook_cannot_be_bound()
	{
		var book = Workbook.Open(Basic("xlsx"));
		var sheet = book.Sheet(0, SheetOptions.Default);
		book.Dispose();
		Should.Throw<ObjectDisposedException>(() => sheet.Bind([Column.Text(0)]));
		sheet.IsBound.ShouldBeFalse();
	}

	[Fact]
	void The_rows_of_a_reader_span_its_batches_and_stop_at_its_end()
	{
		var text = new StringBuilder("n\n");
		for (var row = 0; row < 10; row++)
			text.Append(row).Append('\n');
		using var reader = new DelimitedReader(Encoding.UTF8.GetBytes(text.ToString()).AsMemory(), Dialect.Csv, [Column.Int32(0)], batchRows: 3);
		var seen = new List<(int Index, int Line, int Value)>();
		foreach (var row in reader.Rows())
			seen.Add((row.Index, row.Line, row.Get<int>(0) is Success<int> value ? value.Value : -1));
		seen.Select(entry => entry.Value).ShouldBe(Enumerable.Range(0, 10));
		seen.Select(entry => entry.Index).ShouldBe([0, 1, 2, 0, 1, 2, 0, 1, 2, 0], "four batches");
		seen.Select(entry => entry.Line).ShouldBe(Enumerable.Range(2, 10));
		reader.Read().ShouldBeNull();

		using var book = Workbook.Open(Basic("xlsx"));
		var sheet = book.Sheet(0, SheetOptions.Default with { BatchRows = 1 }, [Column.Int64(0)]);
		var rows = 0;
		foreach (var row in sheet.Rows())
		{
			row.Index.ShouldBe(0);
			rows++;
		}
		rows.ShouldBeGreaterThanOrEqualTo(3);
		sheet.Read().ShouldBeNull();
	}

	[Fact]
	void A_batch_is_a_sequence_of_rows_and_a_row_is_found_by_index()
	{
		using var reader = new DelimitedReader(_countries.AsMemory(), Dialect.Csv, [Column.Text(1), Column.Int32(2)]);
		var batch = reader.Read()!;
		var visited = 0;
		foreach (var row in batch)
			row.Index.ShouldBe(visited++);
		visited.ShouldBe(batch.Rows);
		batch.Row(1).GetString(0).ShouldBe("BR");
		batch.Row(1).Verdict(1).IsOk.ShouldBeTrue();
		Encoding.UTF8.GetString(batch.Row(0).Raw(1)).ShouldBe("250");
		Should.Throw<ArgumentOutOfRangeException>(() => batch.Row(2));
	}

	[Fact]
	void Chars_from_the_arena_stay_put_while_the_batch_is_current()
	{
		Batch.Stingy = true;
		Batch.CharsGrown = 0;
		try
		{
			var text = new StringBuilder();
			for (var row = 0; row < 200; row++)
				text.Append($"name {row},\"quo\"\"ted {row}\",é{row}\n");
			using var reader = new DelimitedReader(Encoding.UTF8.GetBytes(text.ToString()).AsMemory(),
				Dialect.Csv with { HasHeader = false }, [Column.Text(0), Column.Text(1), Column.Text(2)]);
			var batch = reader.Read()!;
			var first = batch.GetChars(0, 0);
			for (var row = 0; row < batch.Rows; row++)
				for (var column = 0; column < 3; column++)
					batch.GetChars(column, row).ToString().ShouldBe(batch.GetString(column, row));
			// Every call after the first one grew the one-element arena, and the first span
			// still says what it said.
			Batch.CharsGrown.ShouldBeGreaterThan(100);
			first.ToString().ShouldBe("name 0");
			Should.Throw<InvalidOperationException>(() =>
			{
				using var typed = new DelimitedReader("1\n"u8.ToArray().AsMemory(), Dialect.Csv with { HasHeader = false }, [Column.Int32(0)]);
				typed.Read()!.GetChars(0, 0);
			});
		}
		finally
		{
			Batch.Stingy = false;
		}
	}

	[Fact]
	void Rows_and_chars_allocate_nothing_once_warmed_up()
	{
		static ReadOnlyMemory<byte> Csv(int rows)
		{
			var text = new StringBuilder("id,name\n");
			for (var row = 0; row < rows; row++)
				text.Append($"{row:D6},name {row:D6}\n");
			return Encoding.UTF8.GetBytes(text.ToString());
		}

		long Consume(Batch batch)
		{
			long sum = 0;
			foreach (var row in batch)
			{
				if (row.Get<int>(0) is Success<int> id)
					sum += id.Value;
				sum += row.GetChars(1).Length + batch.GetChars(1, row.Index)[^1];
				sum += row.Verdict(0).IsOk ? 1 : 0;
			}
			return sum;
		}

		// What a pass allocates once its reader has read a batch: the reader's own buffers are
		// made and grown by then, so the rest of the input has nothing left to ask for.
		(long Allocated, int Rows) Pass(ReadOnlyMemory<byte> utf8)
		{
			using var reader = new DelimitedReader(utf8, Dialect.Csv);
			reader.Bind([Column.Int32(reader.Header!.Ordinal("id")), Column.Text(reader.Header.Ordinal("name"))]);
			Consume(reader.Read()!);
			var before = GC.GetAllocatedBytesForCurrentThread();
			var rows = 0;
			long total = 0;
			while (reader.Read() is { } batch)
			{
				total += Consume(batch);
				rows += batch.Rows;
			}
			var allocated = GC.GetAllocatedBytesForCurrentThread() - before;
			total.ShouldBeGreaterThan(0);
			return (allocated, rows);
		}

		// Allocation-free means what a read allocates does not grow with what it reads, and
		// that is what is measured: a pass of 4 batches against one of 25. What the runtime
		// does the first time it meets code — loading a type, tiering a hot loop up, on its
		// own schedule per platform — is the same in both and cancels; a value or a span made
		// a row or a batch scales with the input and cannot. An allocation of ours is the
		// same every time, so an attempt that shows no growth clears it, and the runtime gets
		// three.
		var (shortInput, longInput) = (Csv(4 * DelimitedReader.DefaultBatchRows), Csv(25 * DelimitedReader.DefaultBatchRows));
		var attempts = new List<string>();
		for (var attempt = 0; attempt < 3; attempt++)
		{
			var few = Pass(shortInput);
			var many = Pass(longInput);
			few.Rows.ShouldBe(3 * DelimitedReader.DefaultBatchRows);
			many.Rows.ShouldBe(24 * DelimitedReader.DefaultBatchRows);
			if (many.Allocated <= few.Allocated)
				return;
			attempts.Add($"{few.Allocated} B over 3 batches, {many.Allocated} B over 24");
		}
		throw new ShouldAssertException($"a read's allocations grow with the input: {string.Join("; ", attempts)}");
	}

	[Fact]
	void A_workbook_opens_from_a_stream_and_disposes_it_unless_asked_not_to()
	{
		var bytes = File.ReadAllBytes(Basic("ods"));
		foreach (var leaveOpen in (bool[])[false, true])
		{
			var stream = new ShortReadStream(bytes, 7);
			using (var book = new Workbook(stream, leaveOpen))
			{
				stream.Disposed.ShouldBe(!leaveOpen, "the stream is done with once read, not when the workbook is");
				book.Format.ShouldBe(WorkbookFormat.Ods);
				book.Sheet("Second", SheetOptions.Default, [Column.Text(0)]).Read()!.GetString(0, 0).ShouldBe("42");
			}
			var seekable = new MemoryStream(bytes);
			using (Workbook.Open(seekable, leaveOpen))
				seekable.CanRead.ShouldBe(leaveOpen);
		}

		// A stream that says it holds more than an array can is refused before it is read.
		Should.Throw<TabularException>(() => new Workbook(new HugeStream())).Failure.ShouldBe(TabularFailure.TooLarge);
	}

	[Fact]
	async Task A_workbook_opens_asynchronously()
	{
		var bytes = File.ReadAllBytes(Basic("xlsx"));
		var stream = new ShortReadStream(bytes, 5);
		using (var book = await Workbook.OpenAsync(stream, leaveOpen: true, TestContext.Current.CancellationToken))
		{
			stream.Disposed.ShouldBeFalse();
			book.Sheets[0].Name.ShouldBe("Data");
		}
		using var cancelled = new CancellationTokenSource();
		await cancelled.CancelAsync();
		var untouched = new ShortReadStream(bytes);
		await Should.ThrowAsync<OperationCanceledException>(async () => await Workbook.OpenAsync(untouched, cancellationToken: cancelled.Token));
		untouched.Reads.ShouldBe(0);
		await Should.ThrowAsync<TabularException>(async () => await Workbook.OpenAsync(new HugeStream()));
	}

	static byte[] Numbers(int count)
	{
		var text = new StringBuilder("n\n");
		for (var row = 0; row < count; row++)
			text.Append(row).Append('\n');
		return Encoding.UTF8.GetBytes(text.ToString());
	}

	[Fact]
	async Task A_cancelled_read_throws_and_loses_nothing()
	{
		var input = Numbers(2_000);

		// Cancelled before it starts: nothing is done, and the reader reads on afterwards.
		await using var reader = await DelimitedReader.OpenAsync(new ShortReadStream(input), Dialect.Csv, [Column.Int32(0)], batchRows: 100, bufferBytes: 16, cancellationToken: TestContext.Current.CancellationToken);
		using (var cancelled = new CancellationTokenSource())
		{
			await cancelled.CancelAsync();
			await Assert.ThrowsAnyAsync<OperationCanceledException>(async () => await reader.ReadAsync(cancelled.Token));
			await Assert.ThrowsAnyAsync<OperationCanceledException>(async () =>
				await DelimitedReader.OpenAsync(new ShortReadStream(input), Dialect.Csv, cancellationToken: cancelled.Token));
		}

		// Cancelled mid-file, by the stream's own read: the next refill throws, and the
		// reader goes on from where it was.
		var stream = new ShortReadStream(input);
		await using var interrupted = await DelimitedReader.OpenAsync(stream, Dialect.Csv, [Column.Int32(0)], batchRows: 100, bufferBytes: 16, cancellationToken: TestContext.Current.CancellationToken);
		using var source = new CancellationTokenSource();
		stream.Cancel = source;
		stream.CancelAfterReads = stream.Reads + 400;
		var values = new List<int>();
		var cancellations = 0;
		while (true)
		{
			try
			{
				var batch = await interrupted.ReadAsync(source.IsCancellationRequested ? CancellationToken.None : source.Token);
				if (batch is null)
					break;
				values.AddRange(batch.Values<int>(0).ToArray());
			}
			catch (OperationCanceledException)
			{
				cancellations++;
			}
		}
		cancellations.ShouldBe(1);
		values.ShouldBe(Enumerable.Range(0, 2_000));
	}

	[Fact]
	async Task Disposing_asynchronously_disposes_the_stream_asynchronously()
	{
		var stream = new ShortReadStream(Numbers(3));
		var reader = await DelimitedReader.OpenAsync(stream, Dialect.Csv, cancellationToken: TestContext.Current.CancellationToken);
		await reader.DisposeAsync();
		stream.DisposedAsynchronously.ShouldBeTrue();
		await Should.ThrowAsync<ObjectDisposedException>(async () => await reader.ReadAsync());

		var kept = new ShortReadStream(Numbers(3));
		await (await DelimitedReader.OpenAsync(kept, Dialect.Csv, leaveOpen: true, cancellationToken: TestContext.Current.CancellationToken)).DisposeAsync();
		kept.Disposed.ShouldBeFalse();
	}
}

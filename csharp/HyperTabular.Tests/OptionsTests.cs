using System.IO.Compression;
using System.Text;
using HyperCast;
using HyperCast.Interop;

namespace HyperTabular.Tests;

/// <summary>What the corpus cannot say of the declarations a read takes and of what it gives back: the errors, the limits, and what is left unsaid in a case's JSON.</summary>
public sealed class OptionsTests
{
	static readonly NumFormat _commaDecimal = new(',', '.', NumStyles.All);
	static readonly NumFormat _indian = NumFormat.Invariant with { Grouping = NumGrouping.Indian };

	static ReadOnlyMemory<byte> Utf8(string text) => Encoding.UTF8.GetBytes(text);

	static string Corpus(string file) => Path.Combine(CorpusTests._corpusDirectory, "workbook", file);

	[Fact]
	void A_declaration_that_is_not_one_is_refused()
	{
		Column[] plan = [Column.Text(0)];
		var input = Utf8("a\n1\n");
		Should.Throw<ArgumentException>(() => new DelimitedReader(input, Dialect.Csv with { ShortRows = (ShortRows)2 }, plan)).Message.ShouldContain("ShortRows");
		Should.Throw<ArgumentException>(() => new DelimitedReader(input, Dialect.Csv with { LongRows = (LongRows)3 }, plan)).Message.ShouldContain("LongRows");
		Should.Throw<ArgumentException>(() => new DelimitedReader(input, Dialect.Csv with { Encoding = (TextEncoding)4 }, plan)).Message.ShouldContain("TextEncoding");
		Should.Throw<ArgumentException>(() => DelimitedReader.Suggest(input, Dialect.Csv with { LongRows = (LongRows)3 })).Message.ShouldContain("LongRows");
		Should.Throw<ArgumentException>(() => new DelimitedReader(input, Dialect.Csv, [Column.Date(0).WithDateOptions(new DateOptions(DateShapes.None, 5))]))
			.Message.ShouldContain("DateOptions no door accepts");
		Should.Throw<ArgumentException>(() => new DelimitedReader(input, Dialect.Csv, [Column.Date(0).WithDateOptions(new DateOptions((DateShapes)(1 << 7), 0))]))
			.Message.ShouldContain("DateOptions no door accepts");
	}

	/// <summary>Two lists are the same list when they hold the same tokens in order, and a dialect and a column stay comparable with them in.</summary>
	[Fact]
	void Tokens_are_comparable()
	{
		new Tokens("NA", "-").ShouldBe(new Tokens("NA", "-"));
		new Tokens("NA", "-").ShouldNotBe(new Tokens("-", "NA"));
		new Tokens().ShouldBe(default);
		new Tokens("NA").GetHashCode().ShouldBe(new Tokens("NA").GetHashCode());
		// A token's length is its own: these two would be one string if they were joined.
		new Tokens("ab", "c").ShouldNotBe(new Tokens("a", "bc"));
		// A column that opts out of its dialect's tokens is not one that inherits them.
		Column.Text(0).WithTokens().ShouldNotBe(Column.Text(0));
		Column.Text(0).WithTokens("NA").ShouldBe(Column.Text(0).WithTokens("NA"));
		(Dialect.Csv with { Tokens = new Tokens("NA") }).ShouldNotBe(Dialect.Csv);
		(Dialect.Csv with { Tokens = new Tokens("NA") }).ShouldBe(Dialect.Csv with { Tokens = new Tokens("NA") });
	}

	/// <summary>A column's list wins over the dialect's, an empty one takes it off, and neither reaches a column that says nothing.</summary>
	[Fact]
	void A_column_s_tokens_replace_its_dialect_s()
	{
		var dialect = Dialect.Csv with { Tokens = new Tokens("NA") };
		using var reader = new DelimitedReader(Utf8("a,b,c\nNA,NA,NA\nx,x,x\n"), dialect,
			[Column.Text(0), Column.Text(1).WithTokens(), Column.Text(2).WithTokens("x")]);
		var batch = reader.Read().ShouldNotBeNull();
		batch.Origin(0, 0).ShouldBe(Origin.Token);
		batch.Origin(1, 0).ShouldBe(Origin.Cell);
		batch.GetString(1, 0).ShouldBe("NA");
		batch.Origin(2, 0).ShouldBe(Origin.Cell);
		batch.Origin(2, 1).ShouldBe(Origin.Token);
		batch.Verdicts(2)[1].Reason.ShouldBe(CastFailure.Empty);
	}

	static byte[] Utf16(string text, bool lone)
	{
		var bytes = new List<byte>(Encoding.Unicode.GetBytes(text));
		if (lone)
			bytes.AddRange([0x00, 0xD8]); // a high surrogate, and nothing to pair it with
		return [.. bytes];
	}

	static (List<string> Rows, Exception? Ended) ReadAll(DelimitedReader reader)
	{
		var rows = new List<string>();
		try
		{
			while (reader.Read() is { } batch)
				for (var row = 0; row < batch.Rows; row++)
					rows.Add(batch.GetString(0, row) ?? "");
		}
		catch (Exception e) when (e is TabularException or IOException)
		{
			return (rows, e);
		}
		return (rows, null);
	}

	/// <summary>A \r the fill is holding when the encoding fails ends its row, and the row comes first.</summary>
	[Theory]
	[InlineData("a\nb\n", "a,b", 2, 3, 8)]
	[InlineData("a\nb\r", "a,b", 2, 3, 8)]
	[InlineData("a\nb", "a", 1, 2, 6)]
	[InlineData("a\n\"b\r", "a", 1, 2, 10)]
	void An_encoding_failure_comes_after_every_intact_row(string text, string rows, long record, int line, long at)
	{
		var dialect = Dialect.Csv with { HasHeader = false, Encoding = TextEncoding.Utf16Le };
		void Holds(DelimitedReader reader, string label)
		{
			var (read, ended) = ReadAll(reader);
			string.Join(',', read).ShouldBe(rows, label);
			var failure = ended.ShouldBeOfType<TabularException>(label);
			(failure.Failure, failure.Record, failure.Line, failure.Byte).ShouldBe((TabularFailure.Encoding, record, line, at), label);
			failure.Message.ShouldBe($"Record {record} (line {line}) holds text with no character in its encoding, at byte {at}.", label);
		}
		var input = Utf16(text, lone: true);
		foreach (var buffer in (int[])[1, 7, DelimitedReader.DefaultBufferBytes])
		{
			using var reader = new DelimitedReader(new MemoryStream(input), dialect, [Column.Text(0)], bufferBytes: buffer);
			Holds(reader, $"{buffer} bytes");
		}
		using var memory = new DelimitedReader(input.AsMemory(), dialect, [Column.Text(0)]);
		Holds(memory, "memory");
	}

	/// <summary>Bytes a source gave and then a failure: the intact rows in them come out, and the source's failure is what the read ends on, undisguised.</summary>
	[Theory]
	[InlineData(TextEncoding.Utf8)]
	[InlineData(TextEncoding.Latin1)]
	async Task A_decoded_source_fails_as_any_other_does(TextEncoding encoding)
	{
		var broken = new IOException("the wire is cut");
		var dialect = Dialect.Csv with { Encoding = encoding };
		using var reader = new DelimitedReader(new FailingStream("a\n1\n2\n"u8.ToArray(), broken), dialect, [Column.Text(0)]);
		var (rows, ended) = ReadAll(reader);
		string.Join(',', rows).ShouldBe("1,2");
		ended.ShouldBeSameAs(broken);

		await using var awaited = await DelimitedReader.OpenAsync(new FailingStream("a\n1\n2\n"u8.ToArray(), broken), dialect, [Column.Text(0)], cancellationToken: TestContext.Current.CancellationToken);
		var seen = new List<string>();
		await Should.ThrowAsync<IOException>(async () =>
		{
			while (await awaited.ReadAsync(TestContext.Current.CancellationToken) is { } batch)
				for (var row = 0; row < batch.Rows; row++)
					seen.Add(batch.GetString(0, row)!);
		});
		string.Join(',', seen).ShouldBe("1,2");
	}

	/// <summary>Hands over what it has, and fails the next read.</summary>
	sealed class FailingStream(byte[] bytes, Exception failure) : MemoryStream(bytes)
	{
		public override int Read(byte[] buffer, int offset, int count)
		{
			var read = base.Read(buffer, offset, count);
			return read > 0 ? read : throw failure;
		}

		public override int Read(Span<byte> buffer)
		{
			var held = new byte[buffer.Length];
			var read = Read(held, 0, held.Length);
			held.AsSpan(0, read).CopyTo(buffer);
			return read;
		}

		public override ValueTask<int> ReadAsync(Memory<byte> buffer, CancellationToken cancellationToken = default) =>
			new(Read(buffer.Span));

		public override Task<int> ReadAsync(byte[] buffer, int offset, int count, CancellationToken cancellationToken) =>
			Task.FromResult(Read(buffer, offset, count));
	}

	/// <summary>Text in memory under an encoding goes through a buffer of its own, and the ceiling holds there too.</summary>
	[Theory]
	[InlineData(TextEncoding.Utf8)]
	[InlineData(TextEncoding.Latin1)]
	void A_record_past_the_ceiling_is_a_failure(TextEncoding encoding)
	{
		// Longer than the buffer a reader of memory starts with, as well as a stream's one byte.
		var input = Encoding.ASCII.GetBytes(new string('x', DelimitedReader.DefaultBufferBytes + 64) + "\n");
		var dialect = Dialect.Csv with { HasHeader = false, Encoding = encoding };
		DelimitedReader.RowCeiling = 8;
		try
		{
			using var stream = new DelimitedReader(new MemoryStream(input), dialect, [Column.Text(0)], bufferBytes: 1);
			Should.Throw<TabularException>(() => stream.Read()).Failure.ShouldBe(TabularFailure.RowTooLong);
			if (encoding != TextEncoding.Utf8)
			{
				using var memory = new DelimitedReader(input.AsMemory(), dialect, [Column.Text(0)]);
				Should.Throw<TabularException>(() => memory.Read()).Failure.ShouldBe(TabularFailure.RowTooLong);
			}
			// A memory stream's reads finish where they're asked, so the ceiling stays on this thread.
			using var waited = new DelimitedReader(new MemoryStream(input), dialect, [Column.Text(0)], bufferBytes: 1);
			Should.Throw<TabularException>(() => waited.ReadAsync().AsTask().GetAwaiter().GetResult()).Failure.ShouldBe(TabularFailure.RowTooLong);
		}
		finally
		{
			DelimitedReader.RowCeiling = 0;
		}
	}

	/// <summary>One record of <c>x</c> and its line ending, as long as it's told, from no memory of its own.</summary>
	sealed class OneLongRecord(long bytes) : Stream
	{
		long _left = bytes;

		public override bool CanRead => true;
		public override bool CanSeek => false;
		public override bool CanWrite => false;
		public override long Length => throw new NotSupportedException();
		public override long Position { get => throw new NotSupportedException(); set => throw new NotSupportedException(); }
		public override void Flush() { }
		public override long Seek(long offset, SeekOrigin origin) => throw new NotSupportedException();
		public override void SetLength(long value) => throw new NotSupportedException();
		public override void Write(byte[] buffer, int offset, int count) => throw new NotSupportedException();
		public override int Read(byte[] buffer, int offset, int count) => Read(buffer.AsSpan(offset, count));

		public override int Read(Span<byte> buffer)
		{
			if (_left < 0 || buffer.IsEmpty)
				return 0;
			if (_left == 0)
			{
				_left = -1;
				buffer[0] = (byte)'\n';
				return 1;
			}
			var written = (int)Math.Min(buffer.Length, _left);
			buffer[..written].Fill((byte)'x');
			_left -= written;
			return written;
		}
	}

	/// <summary>The ceiling itself, the one Rust's MAX_ROW_BYTES says, either side of it: a second or so, and 3.4 GB of footprint while it runs (measured on an Apple M6), so a host with less than 12 GiB skips it out loud.</summary>
	[Fact]
	void A_record_is_read_up_to_the_real_ceiling_and_no_further()
	{
		var available = GC.GetGCMemoryInfo().TotalAvailableMemoryBytes;
		if (available < 12L << 30)
			Assert.Skip($"The real ceiling needs 12 GiB available; this host has {available} bytes.");
		DelimitedReader.MaxRowBytes.ShouldBe(1 << 30);
		var dialect = Dialect.Csv with { HasHeader = false };
		using (var under = new DelimitedReader(new OneLongRecord(DelimitedReader.MaxRowBytes - 8), dialect, [Column.Text(0)]))
			under.Read()!.Rows.ShouldBe(1);
		using var past = new DelimitedReader(new OneLongRecord(DelimitedReader.MaxRowBytes + 8), dialect, [Column.Text(0)]);
		Should.Throw<TabularException>(() => past.Read()).Failure.ShouldBe(TabularFailure.RowTooLong);
	}

	[Fact]
	void A_range_is_what_the_core_says_it_is()
	{
		var range = SheetRange.Parse("B4:E12");
		(range.FirstRow, range.FirstColumn, range.LastRow, range.LastColumn).ShouldBe((4u, 2u, 12u, 5u));
		new SheetRange(4, 2, 12, 5).ShouldBe(range);
		foreach (var text in (string[])["", "A5", "a5:f200", "F200:A5", "A0:B2", "A1:B", " A1:B2"])
		{
			Should.Throw<FormatException>(() => SheetRange.Parse(text)).Message.ShouldContain($"\"{text}\"");
			SheetRange.TryParse(text, out var none).ShouldBeFalse(text);
			none.ShouldBe(default, text);
		}
		var open = new SheetRange(5, 0, 0, 0);
		(open.FirstRow, open.LastRow).ShouldBe((5u, 0u));
		Should.Throw<ArgumentException>(() => new SheetRange(5, 0, 4, 0));
		Should.Throw<ArgumentException>(() => new SheetRange(0, 3, 0, 2));
	}

	[Fact]
	void A_sheet_takes_its_options()
	{
		using var book = Workbook.Open(Corpus("ranges.xlsx"));
		var cut = SheetRange.Parse("B4:E12");
		var options = SheetOptions.Default with { Range = cut, BatchRows = 2, Notes = false };
		var sheet = book.Sheet(0, options, [Column.Text(0)]);
		sheet.Options.ShouldBe(options);
		sheet.Read().ShouldNotBeNull().Rows.ShouldBe(2);
		Should.Throw<ArgumentOutOfRangeException>(() => book.Sheet(0, options with { BatchRows = 0 }, [Column.Text(0)]));
	}

	/// <summary>
	/// The core decides how much a batch may ask for, and every way in asks it before allocating. These sizes are just
	/// over its limit, so that a regression asks for little more than the limit.
	/// </summary>
	[Fact]
	void The_core_judges_a_batchs_size()
	{
		var input = Utf8("a\n1\n");
		var over = Should.Throw<ArgumentOutOfRangeException>(() => new DelimitedReader(input, Dialect.Csv, [Column.Int32(0)], 1 << 28));
		over.ParamName.ShouldBe("batchRows");
		over.Message.ShouldContain("batchRows asks for 5377130496 bytes, past the limit of 4294967296");
		Should.Throw<ArgumentOutOfRangeException>(() => new DelimitedReader(new MemoryStream(input.ToArray()), Dialect.Csv, [Column.Int32(0)], 1 << 28))
			.Message.ShouldContain("past the limit of 4294967296");

		using (var late = new DelimitedReader(input, Dialect.Csv))
		{
			Should.Throw<ArgumentOutOfRangeException>(() => late.Bind([Column.Int32(0)], 1 << 28))
				.Message.ShouldContain("past the limit of 4294967296");
			Should.Throw<ArgumentOutOfRangeException>(() => late.Bind([Column.Int32(0)], 0))
				.Message.ShouldContain("batchRows must be at least 1");
			late.IsBound.ShouldBeFalse();
			late.Bind([Column.Int32(0)]);
			late.Read().ShouldNotBeNull().Values<int>(0)[0].ShouldBe(1);
		}

		using var book = Workbook.Open(Corpus("basic.xlsx"));
		var large = SheetOptions.Default with { BatchRows = 1 << 27 };
		Should.Throw<ArgumentOutOfRangeException>(() => book.Sheet(0, large, [Column.Int32(0)]))
			.Message.ShouldContain("batchRows asks for 4831838224 bytes, past the limit of 4294967296");
		var unplanned = book.Sheet(0, large);
		Should.Throw<ArgumentOutOfRangeException>(() => unplanned.Bind([Column.Int32(0)]))
			.Message.ShouldContain("past the limit of 4294967296");
		unplanned.IsBound.ShouldBeFalse();

		// A modest batch reads.
		using var modest = new DelimitedReader(input, Dialect.Csv, [Column.Int32(0)], 1 << 16);
		modest.Read().ShouldNotBeNull().Values<int>(0)[0].ShouldBe(1);
	}

	/// <summary>A path's sizes are judged before the file is touched, so a bad one beats a missing file.</summary>
	[Fact]
	void A_paths_sizes_are_judged_before_the_file_is_opened()
	{
		const string nowhere = "no such file";
		Should.Throw<FileNotFoundException>(() => DelimitedReader.Open(nowhere, Dialect.Csv));
		Should.Throw<ArgumentOutOfRangeException>(() => DelimitedReader.Open(nowhere, Dialect.Csv, 0))
			.Message.ShouldContain("bufferBytes must be at least 1");
		Should.Throw<ArgumentOutOfRangeException>(() => DelimitedReader.Open(nowhere, Dialect.Csv, [Column.Int32(0)], 0))
			.Message.ShouldContain("batchRows must be at least 1");
		Should.Throw<ArgumentOutOfRangeException>(() => DelimitedReader.Open(nowhere, Dialect.Csv, [Column.Int32(0)], 1 << 28))
			.Message.ShouldContain("past the limit of 4294967296");
	}

	/// <summary>A size the core allows, past what .NET holds in one array, says so as an argument, before anything is allocated.</summary>
	[Fact]
	void One_array_past_what_dotnet_holds_is_an_argument_error()
	{
		var unnoted = Dialect.Csv with { Notes = false };
		var over = Should.Throw<ArgumentOutOfRangeException>(() => new DelimitedReader(Utf8("a\n1\n"), unnoted, [Column.Uuid(0)], 1 << 27));
		over.ParamName.ShouldBe("batchRows");
		over.Message.ShouldContain("batchRows asks for 2147483648 bytes in one array");
	}

	/// <summary>A sampler's batch is never why a suggestion is refused: nobody set its size.</summary>
	[Fact]
	void A_suggestion_is_not_refused_for_the_size_of_its_own_batch()
	{
		const int width = 268_000;
		var names = string.Join(',', Enumerable.Range(0, width).Select(column => $"c{column}"));
		var row = string.Join(',', Enumerable.Repeat("1", width));
		var wide = DelimitedReader.Suggest(Utf8($"{names}\n{row}\n{row}\n"), Dialect.Csv);
		wide.Report.Count.ShouldBe(width);
		wide.Report[width - 1].Sampled.ShouldBe(2);
		wide.Truncated.ShouldBeFalse();

		// At exactly the limit with no plan, and a sample past every batch.
		using var book = Workbook.Open(Corpus("basic.xlsx"));
		var atTheLimit = book.Sheet(0, SheetOptions.Default with { BatchRows = 1 << 29 });
		var large = atTheLimit.Suggest(new SuggestOptions { SampleRows = int.MaxValue });
		var modest = book.Sheet(0, SheetOptions.Default).Suggest(new SuggestOptions { SampleRows = 4096 });
		large.Report.Select(column => (column.Door, column.Sampled)).ShouldBe(modest.Report.Select(column => (column.Door, column.Sampled)));
	}

	/// <summary>A sample says the same in any number of batches, delimited and sheet.</summary>
	[Fact]
	void A_sample_says_the_same_in_any_number_of_batches()
	{
		var text = Utf8("id,price,when,note\n1,\"1,234.50\",2024-01-02,plain\n2,1.234,2024-02-03,\n3,,2024-03-04,\"a, b\"\nx,0.5,03/04/2024,\"two\nlines\"\n5,12,2024-05-06,more\n6,13,,more\n");
		var unnoted = Dialect.Csv with { Notes = false };
		foreach (var limit in (int[])[1, 2, 3, 5, 6, 1000])
		{
			var options = new SuggestOptions { SampleRows = limit };
			var whole = Words(DelimitedReader.Suggest(text, Dialect.Csv, options));
			foreach (var rows in (int[])[1, 2, 3, 7])
			{
				using var reader = new DelimitedReader(text, unnoted);
				var sample = Sample.Of(reader, plan => reader.Bind(plan, rows), reader.Header!.Count, limit, sheet: false);
				Words(sample.Judge(reader.Header, options)).ShouldBe(whole, $"sample {limit}, batches of {rows}");
			}
		}
		using var book = Workbook.Open(Corpus("basic.xlsx"));
		foreach (var limit in (int[])[1, 2, 3, 1000])
		{
			var options = new SuggestOptions { SampleRows = limit };
			var whole = Words(book.Sheet(0, SheetOptions.Default with { BatchRows = 4096 }).Suggest(options));
			foreach (var rows in (int[])[1, 2, 3])
			{
				var sheet = book.Sheet(0, SheetOptions.Default with { BatchRows = rows, Notes = true });
				var sample = Sample.Of(sheet, plan => sheet.Bind(plan), sheet.Header!.Count, limit, sheet: true);
				Words(sample.Judge(sheet.Header, options)).ShouldBe(whole, $"sheet sample {limit}, batches of {rows}");
			}
		}
		// A control: the sample's size does move the answer, so the comparison above can fail.
		Words(DelimitedReader.Suggest(text, Dialect.Csv, new SuggestOptions { SampleRows = 1 }))
			.ShouldNotBe(Words(DelimitedReader.Suggest(text, Dialect.Csv, new SuggestOptions { SampleRows = 6 })));
	}

	/// <summary>With no header, a short and a long row, and row widths kept, across each batch boundary.</summary>
	[Fact]
	void A_ragged_headerless_sample_says_the_same_in_any_number_of_batches()
	{
		var ragged = Utf8("1,x\n2,y,long\n3\n4,z\n5,w\n");
		var headerless = Dialect.Csv with { HasHeader = false, ShortRows = ShortRows.Pad, LongRows = LongRows.Report };
		foreach (var limit in (int[])[1, 2, 3, 4, 5, 6])
		{
			var options = new SuggestOptions { SampleRows = limit };
			var whole = DelimitedReader.Suggest(ragged, headerless, options);
			whole.Report[0].Sampled.ShouldBe(Math.Min(limit, 5));
			whole.Truncated.ShouldBe(limit < 5);
			foreach (var rows in (int[])[1, 2, 3, 4])
			{
				using var reader = new DelimitedReader(ragged, headerless with { Notes = false });
				var sample = Sample.Of(reader, plan => reader.Bind(plan, rows), 2, limit, sheet: false);
				Words(sample.Judge(reader.Header, options)).ShouldBe(Words(whole), $"ragged {limit}, batches of {rows}");
			}
		}
	}

	static string Words(Suggestion found) =>
		string.Join(';', found.Report.Select(column => $"{column.Ordinal}:{column.Door}:{column.Status}:{column.Matched}/{column.Sampled}")) + $":{found.Truncated}";

	/// <summary>One row past the limit at any batch size names its width: no public plan gets there, so this asks directly.</summary>
	[Fact]
	void A_row_past_the_limit_names_its_width()
	{
		var far = Columns.SizeSpecs([Column.Int32(0)]);
		far[0].Ordinal = uint.MaxValue;
		Should.Throw<ArgumentException>(() => Columns.CheckSizes(far, 1, 1, Columns.SheetFlag))
			.Message.ShouldContain("A row of 4294967296 columns asks for 68719476768 bytes, past the limit of 4294967296");
	}

	/// <summary>An explicit zero is refused by the core, for each option, where an omitted option has its default.</summary>
	[Fact]
	void The_core_refuses_a_zero_where_an_omitted_size_has_its_default()
	{
		var input = Utf8("a\n1\n");
		Should.Throw<ArgumentOutOfRangeException>(() => new DelimitedReader(input, Dialect.Csv, [Column.Int32(0)], 0))
			.Message.ShouldContain("batchRows must be at least 1");
		Should.Throw<ArgumentOutOfRangeException>(() => new DelimitedReader(new MemoryStream(input.ToArray()), Dialect.Csv, [Column.Int32(0)], 0))
			.Message.ShouldContain("batchRows must be at least 1");
		var noBuffer = Should.Throw<ArgumentOutOfRangeException>(() => new DelimitedReader(new MemoryStream(input.ToArray()), Dialect.Csv, 0));
		noBuffer.ParamName.ShouldBe("bufferBytes");
		noBuffer.Message.ShouldContain("bufferBytes must be at least 1");
		using var book = Workbook.Open(Corpus("basic.xlsx"));
		Should.Throw<ArgumentOutOfRangeException>(() => book.Sheet(0, SheetOptions.Default with { BatchRows = 0 }, [Column.Text(0)]))
			.Message.ShouldContain("batchRows must be at least 1");
		Should.Throw<ArgumentOutOfRangeException>(() => book.Sheet(0, SheetOptions.Default with { BatchRows = 0 }))
			.Message.ShouldContain("batchRows must be at least 1");

		SheetOptions.Default.BatchRows.ShouldBe(DelimitedReader.DefaultBatchRows);
		var rows = Utf8("a\n" + string.Concat(Enumerable.Repeat("1\n", DelimitedReader.DefaultBatchRows + 1)));
		using var omitted = new DelimitedReader(rows, Dialect.Csv, [Column.Int32(0)]);
		omitted.Read().ShouldNotBeNull().Rows.ShouldBe(DelimitedReader.DefaultBatchRows);
		using var omittedStream = new DelimitedReader(new MemoryStream(input.ToArray()), Dialect.Csv, [Column.Int32(0)]);
		omittedStream.Read().ShouldNotBeNull().Rows.ShouldBe(1);
		DelimitedReader.Suggest(input, Dialect.Csv, new SuggestOptions { SampleRows = 0 }).Report[0].Sampled.ShouldBe(1);
	}

	/// <summary>Notes off is what a read asks the core for: no word on a refused cell, where the default has one, delimited and sheet.</summary>
	[Fact]
	void Notes_off_asks_for_no_notes()
	{
		foreach (var notes in (bool[])[true, false])
		{
			using var reader = new DelimitedReader(Utf8("a\n1x\n"), Dialect.Csv with { Notes = notes }, [Column.Int32(0)]);
			var batch = reader.Read().ShouldNotBeNull();
			batch.Verdicts(0)[0].Reason.ShouldBe(CastFailure.Malformed);
			(batch.Fault(0, 0).ShouldNotBeNull().DetailCode != 0).ShouldBe(notes);
			(batch.Note(0, 0).Detail != 0).ShouldBe(notes);
			Dialect.Csv.Notes.ShouldBeTrue();
			SheetOptions.Default.Notes.ShouldBeTrue();
			new SheetOptions(HasHeader: false).Notes.ShouldBeTrue();
		}
	}

	/// <summary>Fault detail reaches every door the verdict does: the union's, the row's and the batch's.</summary>
	[Fact]
	void A_fault_carries_its_detail_everywhere_it_is_given()
	{
		using var reader = new DelimitedReader(Utf8("a\n\"1,23,456\"\n"), Dialect.Csv, [Column.Int32(0)]);
		var batch = reader.Read().ShouldNotBeNull();
		var fault = batch.Get<int>(0, 0).TryGetValue(out Fault found) ? found : throw new ShouldAssertException("no fault");
		fault.Detail.ShouldBe(FaultDetail.GroupSize);
		batch.Fault(0, 0).ShouldNotBeNull().Detail.ShouldBe(FaultDetail.GroupSize);
		batch.Row(0).Fault(0).ShouldNotBeNull().Detail.ShouldBe(FaultDetail.GroupSize);
		batch.Faults().Single().Detail.ShouldBe(FaultDetail.GroupSize);
		// HyperCast's verdict has no room for one, and a fault's equality leaves it out.
		batch.Verdicts(0)[0].ToFault().DetailCode.ShouldBe(0u);
		batch.Verdicts(0)[0].ToFault().ShouldBe(fault);
	}

	/// <summary>A batch's list can be longer than a call gives back, and asked again it starts again.</summary>
	[Fact]
	void A_batch_s_faults_are_as_long_as_they_are()
	{
		const int rows = 3 * 64 + 1;
		var input = "a,b\n" + string.Concat(Enumerable.Repeat("x,q\n", rows));
		using var reader = new DelimitedReader(Utf8(input), Dialect.Csv, [Column.Int32(0), Column.Text(1), Column.Boolean(1)]);
		var batch = reader.Read().ShouldNotBeNull();
		var listed = 0;
		foreach (var problem in batch.Faults())
		{
			var column = listed % 2 == 1 ? 2 : 0;
			problem.Row.ShouldBe(listed / 2);
			problem.PlanColumn.ShouldBe(column);
			problem.SourceColumn.ShouldBe(column / 2);
			problem.Reason.ShouldBe(CastFailure.Malformed);
			problem.Line.ShouldBe(listed / 2 + 2);
			problem.DetailCode.ShouldNotBe(0u);
			listed++;
		}
		listed.ShouldBe(2 * rows);
		batch.Faults().First().Row.ShouldBe(0);
		batch.Faults().Count().ShouldBe(2 * rows);
	}

	/// <summary>The list is the batch's it was asked of: asked and then walked after the reader moved on, it says so, whether the reader found another batch or the end, and whether the walk had begun.</summary>
	[Fact]
	void Faults_asked_of_a_batch_stay_that_batch_s()
	{
		var input = Utf8("a\nx\ny\n");
		using (var reader = new DelimitedReader(input, Dialect.Csv, [Column.Int32(0)], batchRows: 1))
		{
			var list = reader.Read().ShouldNotBeNull().Faults();
			reader.Read().ShouldNotBeNull();
			Should.Throw<InvalidOperationException>(() => list.ToList());
		}
		using (var reader = new DelimitedReader(input, Dialect.Csv, [Column.Int32(0)], batchRows: 1))
		{
			var list = reader.Read().ShouldNotBeNull().Faults();
			while (reader.Read() is not null) { }
			using var walk = list.GetEnumerator();
			Should.Throw<InvalidOperationException>(() => walk.MoveNext());
		}
		using (var reader = new DelimitedReader(input, Dialect.Csv, [Column.Int32(0)]))
		{
			var list = reader.Read().ShouldNotBeNull().Faults();
			using var walk = list.GetEnumerator();
			walk.MoveNext().ShouldBeTrue();
			reader.Read().ShouldBeNull();
			Should.Throw<InvalidOperationException>(() => walk.MoveNext());
		}
	}

	/// <summary>Every door code there could be, 0 and past the last included, with a param beside it: a report and its plan say a door that exists and a param it declares, and only a resolved one is read through.</summary>
	[Fact]
	void Every_door_the_core_can_name_is_a_column()
	{
		for (var code = 0u; code <= (uint)Door.ExcelSerial + 1; code++)
		{
			var door = (Door)code;
			var known = door is >= Door.Boolean and <= Door.ExcelSerial;
			foreach (var param in (uint[])[0, 1, 2, 3, 99])
			{
				var report = Sample.ReportOf(0, new Native.RawSuggestion { Door = code, Param = param, Status = 1, Matched = 1, Sampled = 1 }, null, null);
				if (!known)
				{
					report.Door.ShouldBe(Door.Text, $"door {code}");
					continue;
				}
				var planned = Sample.PlanOf(report);
				using var read = new DelimitedReader(Utf8("1\n"), Dialect.Csv with { HasHeader = false }, [planned]);
				read.Read().ShouldNotBeNull().Columns[0].ShouldBe(planned, $"door {door}, param {param}");
				var valid = door switch
				{
					Door.Unix => Abi.UnixPrecisionFrom(param) is not null,
					Door.ExcelSerial => Abi.ExcelEpochFrom(param) is not null,
					Door.DateOrdered or Door.DateTime => Abi.DateOrderFrom(param) is not null,
					_ => true,
				};
				report.Door.ShouldBe(valid ? door : Door.Text, $"door {door}, param {param}");
				planned.Declared.ShouldBe(door is Door.Unix or Door.ExcelSerial or Door.DateOrdered or Door.DateTime && valid ? param : 0u, $"door {door}, param {param}");
			}
			// Ambiguous and unresolved read as text, whatever the door.
			foreach (var status in (uint[])[2, 3, 0, 9])
			{
				var planned = Sample.PlanOf(Sample.ReportOf(0, new Native.RawSuggestion { Door = code, Param = 1 << 2, Status = status }, null, null));
				(planned.Door, planned.Ordinal).ShouldBe((Door.Text, 0), $"door {code}, status {status}");
			}
		}
	}

	/// <summary>The notations the suggest export names by number, as this binding declares them: each Western.</summary>
	[Fact]
	void A_proved_notation_is_Western()
	{
		NumFormat.Invariant.ToRaw().Flags.ShouldBe(95u);
		_commaDecimal.ToRaw().Flags.ShouldBe(95u);
		const uint commaGroups = 1 << 1, commaDecimal = 1 << 2;
		static SuggestedColumn Report(Door door, uint param, uint status, NumFormat? declared = null) =>
			Sample.ReportOf(0, new Native.RawSuggestion { Door = (uint)door, Param = param, Status = status }, null, declared);

		// Proved by a , or not, the plan is the same: a later 1,23,456 is refused.
		foreach (var param in (uint[])[0, commaGroups])
		{
			var report = Report(Door.Int32, param, 1);
			report.Format.ShouldBe(NumFormat.Invariant);
			report.Param.ShouldBe(0u);
			using var reader = new DelimitedReader(Utf8("a\n\"1,234\"\n\"1,23,456\"\n"), Dialect.Csv, [Sample.PlanOf(report)]);
			var batch = reader.Read().ShouldNotBeNull();
			batch.Values<int>(0)[0].ShouldBe(1234);
			batch.Verdicts(0)[1].Reason.ShouldBe(CastFailure.Malformed);
		}
		// What only the comma decimal reads is planned in it, and a later group it can't read is refused.
		var decimals = Report(Door.Double, commaDecimal, 1);
		decimals.Format.ShouldBe(_commaDecimal);
		(decimals.Param, decimals.Candidates.Count).ShouldBe((0u, 0));
		using (var reader = new DelimitedReader(Utf8("a\n\"0,125\"\n\"1.234,5\"\n\"1,234,567\"\n"), Dialect.Csv, [Sample.PlanOf(decimals)]))
		{
			var batch = reader.Read().ShouldNotBeNull();
			batch.Values<double>(0)[0].ShouldBe(0.125);
			batch.Values<double>(0)[1].ShouldBe(1234.5);
			batch.Verdicts(0)[2].Reason.ShouldBe(CastFailure.Malformed);
		}
		// A date's param of 2 is its order, no notation, and it names no format.
		Report(Door.DateOrdered, 2, 1).Format.ShouldBeNull();
		// A column's declaration wins, and the report names it, whatever param says.
		foreach (var param in (uint[])[commaGroups, commaDecimal])
			Report(Door.Int32, param, 1, NumFormat.Invariant).Format.ShouldBe(NumFormat.Invariant);
		var declared = Report(Door.Int32, commaGroups, 1, _commaDecimal);
		declared.Format.ShouldBe(_commaDecimal);
		Sample.PlanOf(declared).Format.ShouldBe(_commaDecimal);
		// A column that isn't a resolved number names no format.
		Report(Door.Text, 0, 1).Format.ShouldBeNull();
		Report(Door.Int32, commaGroups | commaDecimal, 2).Format.ShouldBeNull();
		Report(Door.Int32, 0, 3).Format.ShouldBeNull();
		// Ambiguous numbers offer both notations, and ambiguous dates, either date door, the orders.
		var both = Report(Door.Int32, commaGroups | commaDecimal, 2);
		both.Candidates.ShouldBe([new Candidate(null, NumFormat.Invariant), new Candidate(null, _commaDecimal)]);
		foreach (var door in (Door[])[Door.DateOrdered, Door.DateTime])
		{
			var open = Report(door, 1 << 2 | 1 << 3, 2);
			(open.Door, open.Param).ShouldBe((door, 2u));
			open.Candidates.ShouldBe([new Candidate(DateOrder.MonthDayYear, null), new Candidate(DateOrder.DayMonthYear, null)]);
		}
	}

	[Fact]
	void Suggest_takes_its_options()
	{
		var text = Utf8("a,b\n\"1,234\",x\n\"5,678\",y\n9,z\n");
		Should.Throw<ArgumentOutOfRangeException>(() => DelimitedReader.Suggest(text, Dialect.Csv, new SuggestOptions { SampleRows = -1 }));
		// As Rust's sample_rows: 0 is one row, and the row past it says there's more.
		var zero = DelimitedReader.Suggest(Utf8("a\n1\n2.5\n"), Dialect.Csv, new SuggestOptions { SampleRows = 0 });
		(zero.Report[0].Door, zero.Report[0].Sampled, zero.Truncated).ShouldBe((Door.Boolean, 1, true));
		var whole = DelimitedReader.Suggest(Utf8("a\n1\n2.5\n"), Dialect.Csv);
		(whole.Report[0].Door, whole.Report[0].Sampled, whole.Truncated).ShouldBe((Door.Double, 2, false));
		var one = DelimitedReader.Suggest(text, Dialect.Csv, new SuggestOptions { SampleRows = 1 });
		one.Truncated.ShouldBeTrue();
		one.Report[0].Sampled.ShouldBe(1);
		(one.Report[0].Name, one.Report[1].Name).ShouldBe(("a", "b"));

		// A notation for one column, ahead of another for all of them.
		var own = DelimitedReader.Suggest(text, Dialect.Csv, new SuggestOptions
		{
			Format = NumFormat.Invariant,
			ColumnFormats = new Dictionary<int, NumFormat> { [0] = _commaDecimal },
		});
		own.Plan[0].Door.ShouldBe(Door.Double);
		own.Plan[0].Format.ShouldBe(_commaDecimal);

		// With no header, the width is the first record's, and no column has a name.
		var headerless = Dialect.Csv with { HasHeader = false };
		var none = DelimitedReader.Suggest(Utf8("1,x,2\n3,y,4\n"), headerless);
		none.Plan.Count.ShouldBe(3);
		none.Report[0].Name.ShouldBeNull();
		// Nothing to read is nothing to say.
		var nothing = DelimitedReader.Suggest(ReadOnlyMemory<byte>.Empty, headerless);
		(nothing.Plan.Count, nothing.Report.Count, nothing.Truncated).ShouldBe((0, 0, false));
		// A failure among the sampled rows is the suggestion's, and just past them says only that there's more.
		var broken = Utf8("a,b\n1,2\n3\n");
		Should.Throw<TabularException>(() => DelimitedReader.Suggest(broken, Dialect.Csv));
		DelimitedReader.Suggest(broken, Dialect.Csv, new SuggestOptions { SampleRows = 1 }).Truncated.ShouldBeTrue();
		// A file that isn't there is the open's error, not a suggestion's.
		Should.Throw<FileNotFoundException>(() => DelimitedReader.Suggest(Path.Combine(CorpusTests._corpusDirectory, "missing.csv"), Dialect.Csv));
	}

	/// <summary>A source too wide to plan is refused before it is read, and one with no columns takes none.</summary>
	[Fact]
	void A_sample_fits_a_plan()
	{
		var source = new Unreadable();
		Should.Throw<ArgumentException>(() => Sample.Of(source, _ => throw new ShouldAssertException("bound"), Columns.MaxOrdinal + 2, 1, sheet: false))
			.Message.ShouldContain("2^20");
		Sample.Of(source, _ => throw new ShouldAssertException("bound"), 0, 1, sheet: false).ShouldNotBeNull();
	}

	sealed class Unreadable : IBatchSource
	{
		public Batch? Read() => throw new ShouldAssertException("read");
	}

	/// <summary>A sheet's suggestion, as delimited text's: a read of its own, from the start, that leaves this one where it was; a header's names, or a width from the first row; and a range's ordinals.</summary>
	[Fact]
	void A_sheet_suggests()
	{
		using var book = Workbook.Open(Corpus("ranges.xlsx"));
		var options = SheetOptions.Default with { Range = SheetRange.Parse("B4:E12") };
		var sheet = book.Sheet(0, options);
		var found = sheet.Suggest();
		sheet.IsBound.ShouldBeFalse();
		found.Report.Select(report => report.Name).ShouldBe(["id", "name", "amt", "day"]);
		// "merged" beside the ids is a cell no door takes, beside cells one does.
		found.Plan[0].Door.ShouldBe(Door.Text);
		(found.Report[0].Status, found.Report[0].Matched).ShouldBe((SuggestStatus.Unresolved, 6));
		found.Plan[1].Door.ShouldBe(Door.Text);
		// A typed date's canonical text isn't taken as a date: it's text, resolved, by hand.
		found.Plan[3].Door.ShouldBe(Door.Text);
		// The plan reads this sheet, from its start.
		sheet.Bind([.. found.Plan]);
		sheet.Read().ShouldNotBeNull().Rows.ShouldBe(7);

		// No header: the width is the first row's.
		var headless = book.Sheet(0, options with { HasHeader = false }).Suggest(new SuggestOptions { SampleRows = 2 });
		headless.Plan.Count.ShouldBe(4);
		headless.Report[0].Name.ShouldBeNull();
		headless.Truncated.ShouldBeTrue();
		headless.Report[0].Sampled.ShouldBe(2);
	}

	static bool ReadFails(Workbook book, int index)
	{
		try
		{
			var sheet = book.Sheet(index, SheetOptions.Default, [Column.Text(0)]);
			while (sheet.Read() is not null)
			{
			}
			return false;
		}
		catch (TabularException)
		{
			return true;
		}
	}

	/// <summary>Every sheet of every package, under every declaration: a plan that reads what it sampled, and a suggestion that fails only where a read does.</summary>
	[Fact]
	void No_sheet_s_plan_refuses_a_cell_it_sampled()
	{
		var (planned, typed, refused) = (0, 0, 0);
		foreach (var path in Directory.GetFiles(Path.Combine(CorpusTests._corpusDirectory, "workbook")))
		{
			if (path.EndsWith(".py", StringComparison.Ordinal))
				continue;
			Workbook book;
			try
			{
				book = Workbook.Open(path);
			}
			catch (TabularException)
			{
				continue;
			}
			using (book)
				for (var index = 0; index < book.Sheets.Count; index++)
					foreach (var declared in (NumFormat?[])[null, NumFormat.Invariant, _commaDecimal, _indian])
					{
						Suggestion found;
						try
						{
							found = book.Sheet(index, SheetOptions.Default).Suggest(new SuggestOptions { Format = declared });
						}
						catch (TabularException failure)
						{
							refused++;
							ReadFails(book, index).ShouldBeTrue($"{path}, sheet {index}: {failure.Message}, where a read has no failure");
							continue;
						}
						if (found.Plan.Count == 0)
							continue;
						planned++;
						typed += found.Plan.Count(column => column.Door != Door.Text);
						var read = book.Sheet(index, SheetOptions.Default, [.. found.Plan]);
						for (var rows = 0; rows < 1000 && read.Read() is { } batch; rows += batch.Rows)
							foreach (var problem in batch.Faults())
								if (problem.Row + rows < 1000 && problem.Reason is not (null or CastFailure.Empty) && found.Plan[problem.PlanColumn!.Value].Door != Door.Text)
									throw new ShouldAssertException($"{path}, sheet {index}, {declared}: {problem}");
					}
		}
		planned.ShouldBeGreaterThan(80);
		refused.ShouldBeGreaterThanOrEqualTo(2);
		typed.ShouldBeGreaterThan(350);
	}

	static string Said(Suggestion found) =>
		string.Join('\n', found.Report.Select(report =>
			$"{report.Ordinal} {report.Name} {report.Door} {report.Param} {report.Format} {report.Status} {report.Matched} {report.Sampled} {string.Join(',', report.Candidates)}"));

	/// <summary>A typed number has no locale: a workbook's number column is a number whether a notation is declared or not, and whichever.</summary>
	[Theory]
	[InlineData("basic.xlsx", "amount", "ratio")]
	[InlineData("basic.ods", "amount", "pct")]
	[InlineData("excel-web-data.xlsx", "pct", "money")]
	void A_workbook_s_number_column_is_numeric_under_any_format(string file, string first, string second)
	{
		using var book = Workbook.Open(Corpus(file));
		foreach (var declared in (NumFormat?[])[null, NumFormat.Invariant, _commaDecimal])
		{
			var found = book.Sheet(0, SheetOptions.Default).Suggest(new SuggestOptions { Format = declared });
			foreach (var name in (string[])[first, second])
			{
				var report = found.Report.Single(column => column.Name == name);
				report.Status.ShouldBe(SuggestStatus.Resolved, $"{file} {name}");
				Column.IsNumeric(report.Door).ShouldBeTrue($"{file} {name}");
			}
			// The sample's notes are its own, whatever a read of the sheet was to have.
			var quiet = book.Sheet(0, SheetOptions.Default with { Notes = false }).Suggest(new SuggestOptions { Format = declared });
			Said(quiet).ShouldBe(Said(found));
		}
	}

	/// <summary>The smallest package that holds these strings in column A, each as text in its own row.</summary>
	static Workbook Strings(params string[] cells)
	{
		var package = new MemoryStream();
		using (var zip = new ZipArchive(package, ZipArchiveMode.Create, leaveOpen: true))
		{
			void Part(string name, string text)
			{
				using var part = zip.CreateEntry(name).Open();
				part.Write(Encoding.UTF8.GetBytes(text));
			}
			const string Ns = "http://schemas.openxmlformats.org";
			Part("[Content_Types].xml", $"<Types xmlns=\"{Ns}/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/><Override PartName=\"/xl/worksheets/sheet1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/></Types>");
			Part("_rels/.rels", $"<Relationships xmlns=\"{Ns}/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"{Ns}/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/></Relationships>");
			Part("xl/workbook.xml", $"<workbook xmlns=\"{Ns}/spreadsheetml/2006/main\" xmlns:r=\"{Ns}/officeDocument/2006/relationships\"><sheets><sheet name=\"Data\" sheetId=\"1\" r:id=\"rId1\"/></sheets></workbook>");
			Part("xl/_rels/workbook.xml.rels", $"<Relationships xmlns=\"{Ns}/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"{Ns}/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet1.xml\"/></Relationships>");
			var rows = string.Concat(cells.Select((cell, at) => $"<row r=\"{at + 1}\"><c r=\"A{at + 1}\" t=\"inlineStr\"><is><t>{cell}</t></is></c></row>"));
			Part("xl/worksheets/sheet1.xml", $"<worksheet xmlns=\"{Ns}/spreadsheetml/2006/main\"><sheetData>{rows}</sheetData></worksheet>");
		}
		package.Position = 0;
		return Workbook.Open(package);
	}

	/// <summary>One batch is the most a sample can be, whatever SampleRows asks.</summary>
	[Fact]
	void A_sheet_s_sample_is_one_of_its_batches()
	{
		using var book = Strings("a", "1", "2", "3", "4", "5", "6", "7", "8");
		var options = SheetOptions.Default with { BatchRows = 5 };
		var found = book.Sheet(0, options).Suggest();
		(found.Report[0].Sampled, found.Truncated).ShouldBe((5, true));
		var whole = book.Sheet(0, SheetOptions.Default).Suggest();
		(whole.Report[0].Sampled, whole.Truncated).ShouldBe((8, false));
	}

	/// <summary>
	/// The sample takes notes of its own: with none, every text cell the second door refuses would pass
	/// for a typed one, and <c>1,234</c> in a string would settle as a number.
	/// </summary>
	[Fact]
	void A_sheet_with_notes_off_still_tells_text_from_a_typed_cell()
	{
		using var book = Strings("a", "1,234", "5,678");
		var noted = book.Sheet(0, SheetOptions.Default).Suggest();
		noted.Report[0].Status.ShouldBe(SuggestStatus.Ambiguous);
		var quiet = book.Sheet(0, SheetOptions.Default with { Notes = false }).Suggest();
		Said(quiet).ShouldBe(Said(noted));
	}

	/// <summary>Delivers batches, then breaks, as a source does.</summary>
	sealed class BreaksAfter(DelimitedReader reader, int batches) : IBatchSource
	{
		int _left = batches;

		public Batch? Read() => _left-- > 0 ? reader.Read() : throw new IOException("the source broke");
	}

	/// <summary>A failure just past a whole sample only says there's more; among the sampled rows it's the caller's.</summary>
	[Fact]
	void A_failure_just_past_the_sample_says_truncated()
	{
		static Sample Sampled(int batchRows)
		{
			using var reader = new DelimitedReader(Utf8("a\n1\n2\n3\n4\n"), Dialect.Csv, [Column.Text(0)], batchRows: batchRows);
			return Sample.Of(new BreaksAfter(reader, 1), _ => { }, 1, 2, sheet: false);
		}
		Sampled(2).Judge(null, SuggestOptions.Default).Truncated.ShouldBeTrue();
		Should.Throw<IOException>(() => Sampled(1)).Message.ShouldBe("the source broke");
	}

	/// <summary>A declared order answers the sample's question and only that.</summary>
	[Fact]
	void A_declared_date_order_settles_a_date_the_sample_left_open()
	{
		var open = Utf8("day\n01/02/2026\n03/04/2026\n");
		static (SuggestedColumn Report, Column Plan) Ask(ReadOnlyMemory<byte> input, SuggestOptions? options = null)
		{
			var found = DelimitedReader.Suggest(input, Dialect.Csv, options);
			return (found.Report[0], found.Plan[0]);
		}

		var (report, plan) = Ask(open);
		(report.Status, plan.Door).ShouldBe((SuggestStatus.Ambiguous, Door.Text));

		(report, plan) = Ask(open, new SuggestOptions { DateOrder = DateOrder.DayMonthYear });
		(report.Status, report.Candidates.Count, plan.Door, report.Param).ShouldBe((SuggestStatus.Resolved, 0, Door.DateOrdered, Abi.Code(DateOrder.DayMonthYear)));

		(report, plan) = Ask(open, new SuggestOptions
		{
			DateOrder = DateOrder.DayMonthYear,
			ColumnDateOrders = new Dictionary<int, DateOrder> { [0] = DateOrder.MonthDayYear },
		});
		(report.Status, plan.Door, report.Param).ShouldBe((SuggestStatus.Resolved, Door.DateOrdered, Abi.Code(DateOrder.MonthDayYear)));

		(report, _) = Ask(Utf8("day\n13/02/2026\n"), new SuggestOptions { DateOrder = DateOrder.MonthDayYear });
		(report.Status, report.Param).ShouldBe((SuggestStatus.Resolved, Abi.Code(DateOrder.DayMonthYear)));

		(report, plan) = Ask(open, new SuggestOptions { DateOrder = DateOrder.YearMonthDay });
		(report.Status, plan.Door).ShouldBe((SuggestStatus.Ambiguous, Door.Text));
	}

	/// <summary>A sheet answers from its first row whatever it has been up to, and carries on where it was.</summary>
	[Fact]
	void A_part_read_sheet_answers_the_same_and_stays_where_it_is()
	{
		using var book = Workbook.Open(Corpus("libreoffice-data.xlsx"));
		var options = SheetOptions.Default with { BatchRows = 1 };
		var sample = new SuggestOptions { SampleRows = 1 };
		var fresh = book.Sheet(0, options).Suggest(sample);
		fresh.Plan.Count.ShouldBeGreaterThan(0);
		Column[] plan = [.. Enumerable.Range(0, fresh.Plan.Count).Select(Column.Text)];
		var whole = book.Sheet(0, options, plan);
		whole.Read().ShouldNotBeNull();
		var line = whole.Read().ShouldNotBeNull().Line(0);

		var part = book.Sheet(0, options, plan);
		part.Read().ShouldNotBeNull();
		var answer = part.Suggest(sample);
		answer.Plan.ShouldBe(fresh.Plan);
		Said(answer).ShouldBe(Said(fresh));
		part.Read().ShouldNotBeNull().Line(0).ShouldBe(line);
	}

	/// <summary>Takes back what the collector can and writes over it, so memory read after it was freed doesn't still say what it said.</summary>
	static void Churn()
	{
		for (var pass = 0; pass < 4; pass++)
		{
			GC.Collect(GC.MaxGeneration, GCCollectionMode.Forced, blocking: true, compacting: true);
			GC.WaitForPendingFinalizers();
			foreach (var size in new[] { 256, 4096, 1 << 16, 1 << 18, 1 << 20 })
			{
				for (var i = 0; i < 16; i++)
					GC.AllocateUninitializedArray<byte>(size, pinned: true).AsSpan().Fill(0xA5);
			}
		}
	}

	[System.Runtime.CompilerServices.MethodImpl(System.Runtime.CompilerServices.MethodImplOptions.NoInlining)]
	static Batch ReaderLeftBehind(string text) =>
		new DelimitedReader(new MemoryStream(Encoding.UTF8.GetBytes(text)), Dialect.Csv, [Column.Text(0), Column.Int32(1)]).Read()!;

	[System.Runtime.CompilerServices.MethodImpl(System.Runtime.CompilerServices.MethodImplOptions.NoInlining)]
	static Batch ReaderDisposed(string text)
	{
		using var reader = new DelimitedReader(new MemoryStream(Encoding.UTF8.GetBytes(text)), Dialect.Csv, [Column.Text(0)]);
		return reader.Read()!;
	}

	[System.Runtime.CompilerServices.MethodImpl(System.Runtime.CompilerServices.MethodImplOptions.NoInlining)]
	static Batch SheetLeftBehind(string file, SheetOptions options, bool disposed)
	{
		var book = Workbook.Open(Corpus(file));
		var batch = book.Sheet(0, options, [Column.Text(0), Column.Text(1)]).Read()!;
		if (disposed)
			book.Dispose();
		return batch;
	}

	/// <summary>A batch handed back past its reader or its workbook never reads what the collector has freed.</summary>
	[Fact]
	void A_batch_outliving_its_reader_reads_no_freed_memory()
	{
		var left = ReaderLeftBehind("name,n\nalice,1\nbob,2x\n");
		Churn();
		left.GetString(0, 0).ShouldBe("alice");
		left.GetString(0, 1).ShouldBe("bob");
		left.Raw(1, 1).ToArray().ShouldBe("2x"u8.ToArray());
		left.Line(1).ShouldBe(3);

		// Disposed, the batch is over: it refuses, as a batch past the next read does.
		var over = ReaderDisposed("name\nalice\n");
		Churn();
		over.Rows.ShouldBe(0);
		Should.Throw<ArgumentOutOfRangeException>(() => over.Line(0));
	}

	/// <summary>The same of a sheet: the shared strings are the workbook's, and a batch's text is in them.</summary>
	[Theory]
	[InlineData("ranges.xlsx")]
	[InlineData("ranges.ods")]
	[InlineData("ranges.numbers")]
	void A_batch_outliving_its_workbook_reads_no_freed_memory(string file)
	{
		var options = SheetOptions.Default with { Range = SheetRange.Parse("B4:C12") };
		using var live = Workbook.Open(Corpus(file));
		var held = live.Sheet(0, options, [Column.Text(0), Column.Text(1)]).Read()!;
		var expected = Enumerable.Range(0, held.Rows).Select(row => (held.GetString(0, row), held.GetString(1, row))).ToArray();
		foreach (var disposed in new[] { false, true })
		{
			var left = SheetLeftBehind(file, options, disposed);
			Churn();
			Enumerable.Range(0, left.Rows).Select(row => (left.GetString(0, row), left.GetString(1, row))).ToArray().ShouldBe(expected);
		}
	}
}

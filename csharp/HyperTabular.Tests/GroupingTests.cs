using System.Reflection;
using HyperCast;
using HyperCast.Interop;

namespace HyperTabular.Tests;

/// <summary>
/// One Indian column beside a Western one in one read, and the retired policy word refused at every door
/// with nothing read. The bytes hold a shape of each policy in each column, so a plan reads them both ways.
/// </summary>
public sealed class GroupingTests
{
	const string _input = "indian,western\n\"1,23,456\",\"1,23,456\"\n\"1,234,567\",\"1,234,567\"\n";

	static readonly byte[] _bytes = System.Text.Encoding.UTF8.GetBytes(_input);
	static readonly NumFormat _indian = NumFormat.Invariant with { Grouping = NumGrouping.Indian };

	/// <summary>
	/// A stale format, which only a reflective decoder can make, since HyperCast's own setters refuse the
	/// retired bit 8: its Styles member holds 351, the same format as 95 with that bit on.
	/// </summary>
	static NumFormat Worded(NumFormat around)
	{
		object boxed = around;
		typeof(NumFormat)
			.GetField("<Styles>k__BackingField", BindingFlags.Instance | BindingFlags.NonPublic)!
			.SetValue(boxed, (NumStyles)351);
		return (NumFormat)boxed;
	}

	/// <summary>The words 351 and 863 hold the retired bit; 95 and 607 are those without it, the control that a door refuses for the word and nothing else.</summary>
	static (uint Flags, NumFormat Format, bool Refused)[] Policies =>
	[
		(351, Worded(NumFormat.Invariant), true),
		(863, Worded(_indian), true),
		(95, NumFormat.Invariant, false),
		(607, _indian, false),
	];

	/// <summary>Holds a door to what a row wants: refused for the format, with HyperCast's word inside; or let through.</summary>
	static void Holds(string what, NumFormat format, bool refused, Action call)
	{
		if (!refused)
		{
			call();
			return;
		}
		var refusal = Should.Throw<ArgumentException>(call, what);
		refusal.ShouldNotBeAssignableTo<TabularException>(what);
		refusal.Message.ShouldContain("no door accepts", Case.Sensitive, what);
		var word = Should.Throw<ArgumentException>(() => format.ToRaw()).Message;
		refusal.InnerException.ShouldNotBeNull(what).Message.ShouldBe(word, what);
	}

	[Fact]
	void The_policies_here_are_the_words_they_say()
	{
		foreach (var (flags, format, refused) in Policies)
		{
			if (refused)
				Should.Throw<ArgumentException>(() => format.ToRaw());
			else
				format.ToRaw().Flags.ShouldBe(flags);
		}
		Should.Throw<ArgumentException>(() => new NumFormat('.', ',', (NumStyles)351));
		Should.Throw<ArgumentException>(() => NumFormat.Invariant with { Grouping = (NumGrouping)256 });
		Should.Throw<ArgumentException>(() => NumFormat.Invariant with { Grouping = (NumGrouping)768 });
		Should.Throw<ArgumentException>(() => new NumFormat('.', ',', (NumStyles)607));
	}

	[Fact]
	void An_Indian_column_reads_beside_a_Western_one()
	{
		// Twice: named, and with no policy said, which is Western.
		foreach (var western in (NumFormat[])[NumFormat.Invariant, new NumFormat('.', ',', NumStyles.All)])
		{
			using var reader = new DelimitedReader(_bytes.AsMemory(), Dialect.Csv, [Column.Int32(0, _indian), Column.Int32(1, western)]);
			var batch = reader.Read().ShouldNotBeNull();
			batch.Rows.ShouldBe(2);
			batch.Verdicts(0)[0].IsOk.ShouldBeTrue();
			batch.Values<int>(0)[0].ShouldBe(123456);
			batch.Verdicts(0)[1].Reason.ShouldBe(CastFailure.Malformed);
			batch.Verdicts(1)[0].Reason.ShouldBe(CastFailure.Malformed);
			batch.Verdicts(1)[1].IsOk.ShouldBeTrue();
			batch.Values<int>(1)[1].ShouldBe(1234567);
		}
	}

	[Fact]
	void A_column_with_no_format_is_Western()
	{
		using var reader = new DelimitedReader(_bytes.AsMemory(), Dialect.Csv, [Column.Int32(0)]);
		var batch = reader.Read().ShouldNotBeNull();
		batch.Verdicts(0)[0].Reason.ShouldBe(CastFailure.Malformed);
		batch.Values<int>(0)[1].ShouldBe(1234567);
		Column.Int32(0).Format.ToRaw().Flags.ShouldBe(95u);
	}

	[Theory]
	[InlineData("grouping.xlsx")]
	[InlineData("grouping.ods")]
	[InlineData("grouping.numbers")]
	void An_Indian_sheet_column_reads_beside_a_Western_one(string file)
	{
		using var book = Workbook.Open(Path.Combine(CorpusTests._corpusDirectory, "workbook", file));
		var sheet = book.Sheet(0, SheetOptions.Default, [Column.Int32(2, _indian), Column.Int32(2, NumFormat.Invariant)]);
		var batch = sheet.Read().ShouldNotBeNull();
		batch.Rows.ShouldBe(3);
		batch.Verdicts(0)[0].IsOk.ShouldBeTrue();
		batch.Values<int>(0)[0].ShouldBe(123456);
		batch.Verdicts(0)[1].Reason.ShouldBe(CastFailure.Malformed);
		batch.Values<int>(0)[2].ShouldBe(1234);
		batch.Verdicts(1)[0].Reason.ShouldBe(CastFailure.Malformed);
		batch.Verdicts(1)[1].IsOk.ShouldBeTrue();
		batch.Values<int>(1)[1].ShouldBe(1234567);
		batch.Values<int>(1)[2].ShouldBe(1234);
	}

	[Fact]
	async Task A_stale_policy_is_refused_at_every_delimited_door()
	{
		var directory = Directory.CreateTempSubdirectory("hypertabular-grouping");
		try
		{
			var path = Path.Combine(directory.FullName, "grouping.csv");
			File.WriteAllBytes(path, _bytes);
			foreach (var (flags, format, refused) in Policies)
			{
				Column[] plan = [Column.Int32(0, format)];
				Holds($"memory, {flags}", format, refused, () => new DelimitedReader(_bytes.AsMemory(), Dialect.Csv, plan).Dispose());
				Holds($"stream, {flags}", format, refused, () => new DelimitedReader(new MemoryStream(_bytes), Dialect.Csv, plan).Dispose());
				Holds($"path, {flags}", format, refused, () => DelimitedReader.Open(path, Dialect.Csv, plan).Dispose());
				Holds($"async, {flags}", format, refused, () => DelimitedReader.OpenAsync(new MemoryStream(_bytes), Dialect.Csv, plan).AsTask().GetAwaiter().GetResult().Dispose());

				DelimitedReader[] unbound =
				[
					new DelimitedReader(_bytes.AsMemory(), Dialect.Csv),
					new DelimitedReader(new MemoryStream(_bytes), Dialect.Csv),
					DelimitedReader.Open(path, Dialect.Csv),
					await DelimitedReader.OpenAsync(new MemoryStream(_bytes), Dialect.Csv, cancellationToken: TestContext.Current.CancellationToken),
				];
				foreach (var reader in unbound)
				{
					Holds($"bind, {flags}", format, refused, () => reader.Bind(plan));
					reader.IsBound.ShouldBe(!refused);
					if (refused)
						Should.Throw<InvalidOperationException>(() => reader.Read()).Message.ShouldNotContain("door");
					else
						reader.Read().ShouldNotBeNull().Rows.ShouldBe(2);
					await reader.DisposeAsync();
				}

				Holds($"suggest memory, {flags}", format, refused, () => DelimitedReader.Suggest(_bytes.AsMemory(), Dialect.Csv, new SuggestOptions { Format = format }));
				Holds($"suggest path, {flags}", format, refused, () => DelimitedReader.Suggest(path, Dialect.Csv, new SuggestOptions { ColumnFormats = new Dictionary<int, NumFormat> { [1] = format } }));
			}
		}
		finally
		{
			directory.Delete(recursive: true);
		}
	}

	[Fact]
	void A_stale_policy_is_refused_at_every_sheet_door()
	{
		using var book = Workbook.Open(Path.Combine(CorpusTests._corpusDirectory, "workbook", "grouping.xlsx"));
		foreach (var (flags, format, refused) in Policies)
		{
			Column[] plan = [Column.Int32(0, format)];
			Holds($"sheet, {flags}", format, refused, () => book.Sheet(0, SheetOptions.Default, plan));
			Holds($"sheet by name, {flags}", format, refused, () => book.Sheet(book.Sheets[0].Name, SheetOptions.Default, plan));

			var sheet = book.Sheet(0, SheetOptions.Default);
			Holds($"suggest, {flags}", format, refused, () => sheet.Suggest(new SuggestOptions { Format = format }));
			Holds($"bind, {flags}", format, refused, () => sheet.Bind(plan));
			sheet.IsBound.ShouldBe(!refused);
			if (refused)
				Should.Throw<InvalidOperationException>(() => sheet.Read());
			else
				sheet.Read().ShouldNotBeNull();
		}
	}

	/// <summary>
	/// A suggest judges every format it is given before it opens anything: one never met, one replaced,
	/// one past the sample, an empty source and a broken one. A word without the retired bit is the
	/// control, which gets as far as the source does.
	/// </summary>
	[Fact]
	void A_stale_format_is_refused_before_a_suggest_opens_anything()
	{
		var headerless = Dialect.Csv with { HasHeader = false };
		foreach (var (flags, format, refused) in Policies)
		{
			var every = new SuggestOptions { Format = format };
			var past = new SuggestOptions { ColumnFormats = new Dictionary<int, NumFormat> { [99] = format } };
			var replaced = new SuggestOptions { Format = format, ColumnFormats = new Dictionary<int, NumFormat> { [0] = _indian } };
			foreach (var (what, text, dialect, options) in ((string, string, Dialect, SuggestOptions)[])
			[
				("a sample", "a\n1\n", Dialect.Csv, every),
				("empty", "", Dialect.Csv, every),
				("empty, headerless", "", headerless, every),
				("past the sample", "a\n1\n", Dialect.Csv, past),
				("replaced for a column", "a\n1\n", Dialect.Csv, replaced),
			])
				Holds($"{what}, {flags}", format, refused, () => DelimitedReader.Suggest(Utf8(text), dialect, options));

			// A broken source says so only of a format that is sound.
			foreach (var (text, dialect) in ((string, Dialect)[])[("a\n\"1\n", Dialect.Csv), ("\"1\n", headerless)])
			{
				if (refused)
					Holds($"a broken source, {flags}", format, true, () => DelimitedReader.Suggest(Utf8(text), dialect, every));
				else
					Should.Throw<TabularException>(() => DelimitedReader.Suggest(Utf8(text), dialect, every));
			}
			var missing = Path.Combine(Path.GetTempPath(), "hypertabular-no-such-file.csv");
			if (refused)
				Holds($"no such file, {flags}", format, true, () => DelimitedReader.Suggest(missing, Dialect.Csv, every));
			else
				Should.Throw<FileNotFoundException>(() => DelimitedReader.Suggest(missing, Dialect.Csv, every));

			// A sheet: the same, and one whose sample would meet its broken part.
			using var book = Workbook.Open(Path.Combine(CorpusTests._corpusDirectory, "workbook", "grouping.xlsx"));
			var sheet = book.Sheet(0, SheetOptions.Default);
			foreach (var (what, options) in ((string, SuggestOptions)[])[("every", every), ("past", past), ("replaced", replaced)])
				Holds($"a sheet, {what}, {flags}", format, refused, () => sheet.Suggest(options));
			using var broken = Workbook.Open(Path.Combine(CorpusTests._corpusDirectory, "workbook", "broken.xlsx"));
			var part = broken.Sheet(0, SheetOptions.Default);
			if (refused)
				Holds($"a broken sheet, {flags}", format, true, () => part.Suggest(every));
			else
				Should.Throw<TabularException>(() => part.Suggest(every));
		}
	}

	/// <summary>What the delimiter rule says stays: nothing says 1,23,456 is Indian. Declared, it is a number.</summary>
	[Fact]
	void An_Indian_shape_has_no_culture_until_one_is_declared()
	{
		static SuggestedColumn Ask(string text, SuggestOptions? options = null) =>
			DelimitedReader.Suggest(Utf8(text), Dialect.Csv, options).Report[0];

		var alone = Ask("a\n\"1,23,456\"\n");
		(alone.Door, alone.Status).ShouldBe((Door.Text, SuggestStatus.Resolved));
		var beside = Ask("a\n\"1,23,456\"\n5\n");
		(beside.Door, beside.Status).ShouldBe((Door.Text, SuggestStatus.Unresolved));
		Ask("a\n\"1,234,567\"\n").Format.ShouldNotBeNull().ToRaw().Flags.ShouldBe(95u);

		var declared = new SuggestOptions { ColumnFormats = new Dictionary<int, NumFormat> { [0] = _indian } };
		var read = Ask("a\n\"1,23,456\"\n", declared);
		(read.Door, read.Status).ShouldBe((Door.Int32, SuggestStatus.Resolved));
		read.Format.ShouldBe(_indian);
		read.Format!.Value.ToRaw().Flags.ShouldBe(607u);
		var western = Ask("a\n\"1,234,567\"\n", declared);
		western.Door.ShouldBe(Door.Text);
		western.Format.ShouldBeNull();
	}

	static ReadOnlyMemory<byte> Utf8(string text) => System.Text.Encoding.UTF8.GetBytes(text);
}

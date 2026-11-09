using System.Globalization;
using System.Text;
using System.Text.Json;
using System.Text.RegularExpressions;
using HyperCast;
using HyperCast.Interop;

namespace HyperTabular.Tests;

/// <summary>
/// Replays the shared conformance corpus (<c>corpus/delimited.json</c> at the repository
/// root) — the same file the Rust binding replays — through this binding: from memory and
/// from a stream read through buffers too small for a record, in batches of one row and of
/// many, with notes on and off. How the input is cut up is the binding's business and must not
/// change the answer.
/// </summary>
public sealed unsafe class CorpusTests
{
	internal static readonly string _corpusDirectory = FindCorpusDirectory();

	internal static string FindCorpusDirectory()
	{
		for (var dir = new DirectoryInfo(AppContext.BaseDirectory); dir is not null; dir = dir.Parent)
		{
			var corpus = Path.Combine(dir.FullName, "corpus");
			if (File.Exists(Path.Combine(corpus, "delimited.json")))
				return corpus;
		}
		throw new DirectoryNotFoundException($"corpus directory not found above {AppContext.BaseDirectory}");
	}

	internal static JsonElement[] Corpus(string name)
	{
		using var document = JsonDocument.Parse(File.ReadAllText(Path.Combine(_corpusDirectory, name)));
		return [.. document.RootElement.EnumerateArray().Select(vector => vector.Clone())];
	}

	/// <summary>A corpus entry's format: <see langword="null"/> where it has none, so the plan column is left to its default.</summary>
	internal static NumFormat? FormatOf(JsonElement entry)
	{
		if (!entry.TryGetProperty("format", out var format))
			return null;
		// A corpus flags value mixes leniences and grouping, which HyperCast's type keeps apart.
		var flags = format.GetProperty("flags").GetUInt32();
		var declared = new NumFormat(
			format.GetProperty("decimal_sep").GetString()![0],
			format.GetProperty("group_sep").GetString()![0],
			(NumStyles)(flags & 0xFF),
			(NumGrouping)(flags & ~0xFFu));
		return format.TryGetProperty("currency", out var symbol) ? declared with { CurrencySymbol = symbol.GetString()! } : declared;
	}

	static DateOptions DateOptionsOf(JsonElement options)
	{
		var shapes = DateShapes.None;
		if (options.TryGetProperty("flags", out var flags))
			foreach (var flag in flags.EnumerateArray())
				shapes |= flag.GetString() switch
				{
					"compact_date" => DateShapes.CompactDate,
					"space_separator" => DateShapes.SpaceSeparator,
					"hour_offset" => DateShapes.HourOffset,
					var other => throw new InvalidOperationException($"unknown date flag {other}"),
				};
		var pivot = options.TryGetProperty("pivot", out var year) ? year.GetInt32() : 0;
		var months = options.TryGetProperty("months", out var names)
			? new MonthNames(names.EnumerateArray().Select(name => (name[0].GetInt32(), name[1].GetString()!)))
			: null;
		return new DateOptions(shapes, pivot) { Months = months };
	}

	internal static Tokens TokensOf(JsonElement tokens) =>
		new([.. tokens.EnumerateArray().Select(token => token.GetString()!)]);

	internal static Column ColumnOf(JsonElement entry)
	{
		var column = DoorOf(entry);
		if (entry.TryGetProperty("tokens", out var tokens))
			column = column.WithTokens([.. tokens.EnumerateArray().Select(token => token.GetString()!)]);
		if (entry.TryGetProperty("options", out var options))
			column = column.WithDateOptions(DateOptionsOf(options));
		return column;
	}

	static Column DoorOf(JsonElement entry)
	{
		var ordinal = entry.GetProperty("ordinal").GetInt32();
		var format = FormatOf(entry);
		return entry.GetProperty("door").GetString() switch
		{
			"bool" => Column.Boolean(ordinal),
			"i8" => format is { } f ? Column.SByte(ordinal, f) : Column.SByte(ordinal),
			"i16" => format is { } f ? Column.Int16(ordinal, f) : Column.Int16(ordinal),
			"i32" => format is { } f ? Column.Int32(ordinal, f) : Column.Int32(ordinal),
			"i64" => format is { } f ? Column.Int64(ordinal, f) : Column.Int64(ordinal),
			"u8" => format is { } f ? Column.Byte(ordinal, f) : Column.Byte(ordinal),
			"u16" => format is { } f ? Column.UInt16(ordinal, f) : Column.UInt16(ordinal),
			"u32" => format is { } f ? Column.UInt32(ordinal, f) : Column.UInt32(ordinal),
			"u64" => format is { } f ? Column.UInt64(ordinal, f) : Column.UInt64(ordinal),
			"f32" => format is { } f ? Column.Single(ordinal, f) : Column.Single(ordinal),
			"f64" => format is { } f ? Column.Double(ordinal, f) : Column.Double(ordinal),
			"decimal" => format is { } f ? Column.Decimal(ordinal, f) : Column.Decimal(ordinal),
			"uuid" => Column.Uuid(ordinal),
			"timestamp" => Column.Timestamp(ordinal),
			"unix" => Column.Unix(ordinal, (UnixPrecision)entry.GetProperty("precision").GetInt32()),
			"excel_serial" => Column.ExcelSerial(ordinal, (ExcelEpoch)entry.GetProperty("epoch").GetInt32()),
			"date" => Column.Date(ordinal),
			"date_ordered" => Column.Date(ordinal, (DateOrder)entry.GetProperty("order").GetInt32()),
			"datetime" => Column.DateTime(ordinal, (DateOrder)entry.GetProperty("order").GetInt32()),
			"time" => Column.Time(ordinal),
			"duration" => Column.Duration(ordinal),
			"text" => Column.Text(ordinal),
			var other => throw new InvalidOperationException($"unknown door {other}"),
		};
	}

	/// <summary>A door's name in the corpus.</summary>
	internal static string DoorName(Door door) =>
		door switch
		{
			Door.Boolean => "bool",
			Door.SByte => "i8",
			Door.Int16 => "i16",
			Door.Int32 => "i32",
			Door.Int64 => "i64",
			Door.Byte => "u8",
			Door.UInt16 => "u16",
			Door.UInt32 => "u32",
			Door.UInt64 => "u64",
			Door.Single => "f32",
			Door.Double => "f64",
			Door.Decimal => "decimal",
			Door.Uuid => "uuid",
			Door.Timestamp => "timestamp",
			Door.Unix => "unix",
			Door.ExcelSerial => "excel_serial",
			Door.Date => "date",
			Door.DateOrdered => "date_ordered",
			Door.DateTime => "datetime",
			Door.Time => "time",
			Door.Duration => "duration",
			Door.Text => "text",
			_ => throw new InvalidOperationException($"no corpus name for {door}"),
		};

	/// <summary>HyperCast's name for what failed, as the corpus spells it: <c>GroupSize</c> is <c>group_size</c>.</summary>
	internal static string DetailName(FaultDetail detail) =>
		Regex.Replace(detail.ToString(), "(?<!^)([A-Z])", "_$1").ToLowerInvariant();

	internal static string OriginName(Origin origin) =>
		origin switch
		{
			Origin.Cell => "cell",
			Origin.Token => "token",
			Origin.Padded => "padded",
			Origin.Uncached => "uncached",
			_ => throw new InvalidOperationException($"no corpus name for {origin}"),
		};

	internal static string FailureName(TabularFailure failure) =>
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
			TabularFailure.Iwa => "iwa",
			TabularFailure.Encoding => "encoding",
			_ => throw new InvalidOperationException($"no corpus name for {failure}"),
		};

	/// <summary>Holds a failure, whole, to what the corpus says of it: every field for every kind.</summary>
	internal static void AssertFailure(string label, TabularException? actual, JsonElement @case)
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
		actual.Expected.ShouldBe(expected.TryGetProperty("expected", out var width) ? width.GetInt32() : 0, label);
		actual.Found.ShouldBe(expected.TryGetProperty("found", out var found) ? found.GetInt32() : 0, label);
	}

	/// <summary>The problems a batch lists, in the corpus's shape, its rows counted across the read.</summary>
	internal static void AppendProblems(string label, List<string> problems, Batch batch, int @base)
	{
		foreach (var problem in batch.Faults())
		{
			problem.Line.ShouldBe(batch.Line(problem.Row), label);
			var entry = new SortedDictionary<string, object> { ["row"] = @base + problem.Row, ["line"] = problem.Line };
			if (problem.PlanColumn is { } plan)
			{
				entry["plan_column"] = plan;
				entry["source_column"] = problem.SourceColumn!.Value;
				entry["reason"] = problem.Reason switch
				{
					CastFailure.Empty => "empty",
					CastFailure.Malformed => "malformed",
					_ => "out_of_range",
				};
				if (problem.Reason != CastFailure.Empty)
					entry["fault"] = new[] { problem.Offset, problem.Length };
				else
					(problem.Offset, problem.Length).ShouldBe((0, 0), label);
				(problem.Expected, problem.Found).ShouldBe((0, 0), label);
			}
			else
			{
				problem.SourceColumn.ShouldBeNull(label);
				problem.Reason.ShouldBeNull(label);
				(problem.Offset, problem.Length, problem.DetailCode).ShouldBe((0, 0, 0u), label);
				entry["expected"] = problem.Expected;
				entry["found"] = problem.Found;
			}
			if (problem.DetailCode != 0)
				entry["detail"] = DetailName(problem.Detail);
			if (problem.Origin != Origin.Cell)
				entry["origin"] = OriginName(problem.Origin);
			problems.Add(JsonSerializer.Serialize(entry));
		}
	}

	/// <summary>
	/// Holds what the faults listed to what the corpus says. With notes off the list has no detail or
	/// origin to give, and no formula with no cached value.
	/// </summary>
	internal static void AssertProblems(string label, List<string> found, JsonElement @case, bool notes)
	{
		var expected = new List<string>();
		foreach (var problem in @case.GetProperty("problems").EnumerateArray())
		{
			var entry = new SortedDictionary<string, object>();
			foreach (var property in problem.EnumerateObject())
				entry[property.Name] = property.Value;
			if (!notes)
			{
				if (problem.TryGetProperty("origin", out var origin) && origin.GetString() == "uncached")
					continue;
				entry.Remove("detail");
				entry.Remove("origin");
			}
			expected.Add(JsonSerializer.Serialize(entry));
		}
		found.ShouldBe(expected, label);
	}

	/// <summary>The bytes of a case's input.</summary>
	internal static byte[] InputOf(JsonElement vector) =>
		vector.TryGetProperty("input_hex", out var hex)
			? Convert.FromHexString(hex.GetString()!)
			: Encoding.UTF8.GetBytes(vector.GetProperty("input").GetString()!);

	static DateTimeOffset Instant(JsonElement cell) =>
		new(DateTime.UnixEpoch.Ticks
			+ cell.GetProperty("seconds").GetInt64() * TimeSpan.TicksPerSecond
			+ cell.GetProperty("nanos").GetInt32() / 100, TimeSpan.Zero);

	/// <summary>Holds one cell of a batch to what the corpus says of it.</summary>
	internal static void AssertCell(string label, Batch batch, int column, int row, JsonElement expected, bool notes = true)
	{
		var verdict = batch.Verdicts(column)[row];
		var door = batch.Columns[column].Door;
		var expect = expected.GetProperty("expect").GetString();

		// The note beside the verdict: with notes off there is none, and it says so.
		var note = batch.Note(column, row);
		DetailName((FaultDetail)note.Detail).ShouldBe(notes && expected.TryGetProperty("detail", out var detail) ? detail.GetString() : "none", label);
		OriginName(batch.Origin(column, row)).ShouldBe(notes && expected.TryGetProperty("origin", out var origin) ? origin.GetString() : "cell", label);
		(batch.Fault(column, row) is null).ShouldBe(expect == "ok", label);
		if (batch.Fault(column, row) is { } listed)
		{
			listed.ShouldBe(verdict.ToFault(), label);
			listed.DetailCode.ShouldBe((uint)note.Detail, label);
		}

		if (expect != "ok")
		{
			verdict.IsOk.ShouldBeFalse(label);
			verdict.Reason.ShouldBe(expect switch
			{
				"empty" => CastFailure.Empty,
				"malformed" => CastFailure.Malformed,
				_ => CastFailure.OutOfRange,
			}, label);
			if (expected.TryGetProperty("fault", out var span))
			{
				(verdict.Offset, verdict.Length).ShouldBe((span[0].GetInt32(), span[1].GetInt32()), label);
				// The cell's own text is still to hand, for the diagnostic a fault deserves.
				if (expected.TryGetProperty("raw_hex", out var hex))
					Convert.ToHexStringLower(batch.Raw(column, row)).ShouldBe(hex.GetString(), label);
				else
					Encoding.UTF8.GetString(batch.Raw(column, row)).ShouldBe(expected.GetProperty("raw").GetString(), label);
			}
			if (door == Door.Text)
			{
				batch.TryGetText(column, row, out _).ShouldBeFalse(label);
				// An empty cell is an empty span, where GetString says null.
				batch.GetChars(column, row).IsEmpty.ShouldBeTrue(label);
				batch.TryGetChars(column, row, [], out var none).ShouldBeTrue(label);
				none.ShouldBe(0, label);
			}
			else
			{
				var typed = FaultOf(batch, column, row, door);
				typed.ShouldBe(verdict.ToFault(), label);
				typed.DetailCode.ShouldBe((uint)note.Detail, label);
			}
			return;
		}

		verdict.IsOk.ShouldBeTrue(label);
		switch (door)
		{
			case Door.Boolean:
				Value(batch.Get<bool>(column, row)).ShouldBe(expected.GetProperty("value").GetBoolean(), label);
				break;
			case Door.SByte:
				Value(batch.Get<sbyte>(column, row)).ShouldBe(expected.GetProperty("value").GetSByte(), label);
				break;
			case Door.Int16:
				Value(batch.Get<short>(column, row)).ShouldBe(expected.GetProperty("value").GetInt16(), label);
				break;
			case Door.Int32:
				Value(batch.Get<int>(column, row)).ShouldBe(expected.GetProperty("value").GetInt32(), label);
				break;
			case Door.Int64:
				Value(batch.Get<long>(column, row)).ShouldBe(expected.GetProperty("value").GetInt64(), label);
				break;
			case Door.Byte:
				Value(batch.Get<byte>(column, row)).ShouldBe(expected.GetProperty("value").GetByte(), label);
				break;
			case Door.UInt16:
				Value(batch.Get<ushort>(column, row)).ShouldBe(expected.GetProperty("value").GetUInt16(), label);
				break;
			case Door.UInt32:
				Value(batch.Get<uint>(column, row)).ShouldBe(expected.GetProperty("value").GetUInt32(), label);
				break;
			case Door.UInt64:
				Value(batch.Get<ulong>(column, row)).ShouldBe(expected.GetProperty("value").GetUInt64(), label);
				break;
			case Door.Single:
				Value(batch.Get<float>(column, row)).ShouldBe((float)expected.GetProperty("value").GetDouble(), label);
				break;
			case Door.Double:
				Value(batch.Get<double>(column, row)).ShouldBe(expected.GetProperty("value").GetDouble(), label);
				break;
			case Door.Decimal:
				// The raw triple is the contract; decimal's own constructor is exact for it.
				var magnitude = UInt128.Parse(expected.GetProperty("magnitude").GetString()!, CultureInfo.InvariantCulture);
				var lo = (ulong)magnitude;
				Value(batch.Get<decimal>(column, row)).ShouldBe(new decimal(
					(int)lo, (int)(lo >> 32), (int)(uint)(magnitude >> 64),
					expected.GetProperty("negative").GetBoolean(),
					expected.GetProperty("scale").GetByte()), label);
				break;
			case Door.Uuid:
				Value(batch.Get<Guid>(column, row)).ShouldBe(Guid.ParseExact(expected.GetProperty("value").GetString()!, "N"), label);
				break;
			case Door.Timestamp or Door.Unix or Door.ExcelSerial:
				Value(batch.Get<DateTimeOffset>(column, row)).ShouldBe(Instant(expected), label);
				break;
			case Door.Date or Door.DateOrdered:
				Value(batch.Get<DateOnly>(column, row)).ShouldBe(new DateOnly(
					expected.GetProperty("year").GetInt32(),
					expected.GetProperty("month").GetInt32(),
					expected.GetProperty("day").GetInt32()), label);
				break;
			case Door.DateTime:
				Value(batch.Get<DateTime>(column, row)).ShouldBe(new DateTime(
					expected.GetProperty("year").GetInt32(),
					expected.GetProperty("month").GetInt32(),
					expected.GetProperty("day").GetInt32(), 0, 0, 0, DateTimeKind.Unspecified)
					.AddTicks((long)(expected.GetProperty("nanos_of_day").GetUInt64() / 100)), label);
				break;
			case Door.Time:
				Value(batch.Get<TimeOnly>(column, row)).ShouldBe(new TimeOnly((long)(expected.GetProperty("nanos").GetUInt64() / 100)), label);
				break;
			case Door.Duration:
				Value(batch.Get<TimeSpan>(column, row)).ShouldBe(new TimeSpan(
					expected.GetProperty("seconds").GetInt64() * TimeSpan.TicksPerSecond
					+ expected.GetProperty("nanos").GetInt32() / 100), label);
				break;
			case Door.Text:
				batch.TryGetText(column, row, out var utf8).ShouldBeTrue(label);
				Encoding.UTF8.GetString(utf8).ShouldBe(expected.GetProperty("text").GetString(), label);
				batch.GetString(column, row).ShouldBe(expected.GetProperty("text").GetString(), label);
				// The same text as UTF-16 without a string: into the batch's arena, and into the caller's buffer.
				var text = batch.GetString(column, row)!;
				batch.GetChars(column, row).ToString().ShouldBe(text, label);
				var chars = new char[text.Length + 1];
				batch.TryGetChars(column, row, chars, out var written).ShouldBeTrue(label);
				new string(chars, 0, written).ShouldBe(text, label);
				if (text.Length > 0)
					batch.TryGetChars(column, row, chars.AsSpan(0, text.Length - 1), out _).ShouldBeFalse(label);
				break;
			default:
				throw new InvalidOperationException($"{label}: no accessor for {door}");
		}
	}

	static T Value<T>(Verdict<T> verdict) where T : struct =>
		verdict.TryGetValue(out Success<T> success)
			? success.Value
			: throw new InvalidOperationException($"expected a value, got {verdict}");

	static Fault Fault<T>(Verdict<T> verdict) where T : struct =>
		verdict.TryGetValue(out Fault fault)
			? fault
			: throw new InvalidOperationException($"expected a fault, got {verdict}");

	/// <summary>The generic accessor's own fault for a cell that did not cast.</summary>
	static Fault FaultOf(Batch batch, int column, int row, Door door) =>
		door switch
		{
			Door.Boolean => Fault(batch.Get<bool>(column, row)),
			Door.SByte => Fault(batch.Get<sbyte>(column, row)),
			Door.Int16 => Fault(batch.Get<short>(column, row)),
			Door.Int32 => Fault(batch.Get<int>(column, row)),
			Door.Int64 => Fault(batch.Get<long>(column, row)),
			Door.Byte => Fault(batch.Get<byte>(column, row)),
			Door.UInt16 => Fault(batch.Get<ushort>(column, row)),
			Door.UInt32 => Fault(batch.Get<uint>(column, row)),
			Door.UInt64 => Fault(batch.Get<ulong>(column, row)),
			Door.Single => Fault(batch.Get<float>(column, row)),
			Door.Double => Fault(batch.Get<double>(column, row)),
			Door.Decimal => Fault(batch.Get<decimal>(column, row)),
			Door.Uuid => Fault(batch.Get<Guid>(column, row)),
			Door.Timestamp or Door.Unix or Door.ExcelSerial => Fault(batch.Get<DateTimeOffset>(column, row)),
			Door.Date or Door.DateOrdered => Fault(batch.Get<DateOnly>(column, row)),
			Door.DateTime => Fault(batch.Get<DateTime>(column, row)),
			Door.Time => Fault(batch.Get<TimeOnly>(column, row)),
			Door.Duration => Fault(batch.Get<TimeSpan>(column, row)),
			_ => throw new InvalidOperationException($"no typed accessor for {door}"),
		};

	static Dialect DialectOf(JsonElement vector)
	{
		var settings = vector.GetProperty("dialect");
		var dialect = new Dialect(
			settings.GetProperty("separator").GetString()![0],
			settings.GetProperty("quoting").GetBoolean(),
			settings.GetProperty("has_header").GetBoolean(),
			settings.GetProperty("skip_blank_lines").GetBoolean());
		if (settings.TryGetProperty("skip", out var skip))
			dialect = dialect with { Skip = skip.GetUInt32() };
		if (settings.TryGetProperty("header_row", out var headerRow))
			dialect = dialect with { HeaderRow = headerRow.GetUInt32() };
		if (settings.TryGetProperty("short_rows", out var shortRows))
			dialect = dialect with
			{
				ShortRows = shortRows.GetString() switch
				{
					"reject" => ShortRows.Reject,
					"pad" => ShortRows.Pad,
					var other => throw new InvalidOperationException($"unknown short_rows {other}"),
				},
			};
		if (settings.TryGetProperty("long_rows", out var longRows))
			dialect = dialect with
			{
				LongRows = longRows.GetString() switch
				{
					"reject" => LongRows.Reject,
					"report" => LongRows.Report,
					"truncate" => LongRows.Truncate,
					var other => throw new InvalidOperationException($"unknown long_rows {other}"),
				},
			};
		if (settings.TryGetProperty("tokens", out var tokens))
			dialect = dialect with { Tokens = TokensOf(tokens) };
		if (settings.TryGetProperty("encoding", out var encoding))
			dialect = dialect with
			{
				Encoding = encoding.GetString() switch
				{
					"utf-8" => TextEncoding.Utf8,
					"windows-1252" => TextEncoding.Windows1252,
					"latin-1" => TextEncoding.Latin1,
					"utf-16le" => TextEncoding.Utf16Le,
					var other => throw new InvalidOperationException($"unknown encoding {other}"),
				},
			};
		return dialect;
	}

	/// <summary>
	/// Holds a header the reader read before a plan was bound to what the reader with no plan
	/// promises — names resolved first-match, by string and by bytes; no plan, no read — and
	/// binds <paramref name="plan"/>.
	/// </summary>
	static void BindHeaderFirst(string label, Header? header, IReadOnlyList<Column> unbound, bool isBound,
		Action read, Action<Column[]> bind, Column[] plan)
	{
		isBound.ShouldBeFalse(label);
		unbound.ShouldBeEmpty(label);
		Should.Throw<InvalidOperationException>(read, label);
		if (header is not null)
			for (var ordinal = 0; ordinal < header.Count; ordinal++)
			{
				header.TryOrdinal(header[ordinal], out var found).ShouldBeTrue(label);
				found.ShouldBeLessThanOrEqualTo(ordinal, label);
				header[found].ShouldBe(header[ordinal], label);
				header.TryOrdinal(header.Utf8(ordinal), out var byBytes).ShouldBeTrue(label);
				byBytes.ShouldBeLessThanOrEqualTo(ordinal, label);
				header.Utf8(byBytes).SequenceEqual(header.Utf8(ordinal)).ShouldBeTrue(label);
			}
		bind(plan);
		Should.Throw<InvalidOperationException>(() => bind(plan), label);
	}

	/// <summary>Holds every <see cref="Row"/> of a batch, met by <c>foreach</c>, to the batch's own accessors.</summary>
	internal static void AssertRows(string label, Batch batch)
	{
		var index = 0;
		foreach (var row in batch)
		{
			row.Index.ShouldBe(index, label);
			row.Line.ShouldBe(batch.Line(index), label);
			for (var column = 0; column < batch.Columns.Count; column++)
			{
				row.Verdict(column).ShouldBe(batch.Verdicts(column)[index], label);
				row.Raw(column).SequenceEqual(batch.Raw(column, index)).ShouldBeTrue(label);
				row.Origin(column).ShouldBe(batch.Origin(column, index), label);
				row.Fault(column).ShouldBe(batch.Fault(column, index), label);
				var door = batch.Columns[column].Door;
				if (door == Door.Text)
				{
					row.TryGetText(column, out var mine).ShouldBe(batch.TryGetText(column, index, out var its), label);
					mine.SequenceEqual(its).ShouldBeTrue(label);
					row.GetString(column).ShouldBe(batch.GetString(column, index), label);
					row.GetChars(column).SequenceEqual(batch.GetChars(column, index)).ShouldBeTrue(label);
					continue;
				}
				var (a, b) = door switch
				{
					Door.Boolean => (row.Get<bool>(column).ToString(), batch.Get<bool>(column, index).ToString()),
					Door.SByte => (row.Get<sbyte>(column).ToString(), batch.Get<sbyte>(column, index).ToString()),
					Door.Int16 => (row.Get<short>(column).ToString(), batch.Get<short>(column, index).ToString()),
					Door.Int32 => (row.Get<int>(column).ToString(), batch.Get<int>(column, index).ToString()),
					Door.Int64 => (row.Get<long>(column).ToString(), batch.Get<long>(column, index).ToString()),
					Door.Byte => (row.Get<byte>(column).ToString(), batch.Get<byte>(column, index).ToString()),
					Door.UInt16 => (row.Get<ushort>(column).ToString(), batch.Get<ushort>(column, index).ToString()),
					Door.UInt32 => (row.Get<uint>(column).ToString(), batch.Get<uint>(column, index).ToString()),
					Door.UInt64 => (row.Get<ulong>(column).ToString(), batch.Get<ulong>(column, index).ToString()),
					Door.Single => (row.Get<float>(column).ToString(), batch.Get<float>(column, index).ToString()),
					Door.Double => (row.Get<double>(column).ToString(), batch.Get<double>(column, index).ToString()),
					Door.Decimal => (row.Get<decimal>(column).ToString(), batch.Get<decimal>(column, index).ToString()),
					Door.Uuid => (row.Get<Guid>(column).ToString(), batch.Get<Guid>(column, index).ToString()),
					Door.Timestamp or Door.Unix or Door.ExcelSerial =>
						(row.Get<DateTimeOffset>(column).ToString(), batch.Get<DateTimeOffset>(column, index).ToString()),
					Door.Date or Door.DateOrdered => (row.Get<DateOnly>(column).ToString(), batch.Get<DateOnly>(column, index).ToString()),
					Door.DateTime => (row.Get<DateTime>(column).ToString(), batch.Get<DateTime>(column, index).ToString()),
					Door.Time => (row.Get<TimeOnly>(column).ToString(), batch.Get<TimeOnly>(column, index).ToString()),
					Door.Duration => (row.Get<TimeSpan>(column).ToString(), batch.Get<TimeSpan>(column, index).ToString()),
					_ => throw new InvalidOperationException($"no typed accessor for {door}"),
				};
				a.ShouldBe(b, label);
			}
			index++;
		}
		index.ShouldBe(batch.Rows, label);
	}

	/// <summary>
	/// Reads a corpus vector through a reader <paramref name="open"/> makes — bound already,
	/// or opened header-first and bound here, in batches of <paramref name="batchRows"/> — with
	/// <see cref="DelimitedReader.Read"/> or, when <paramref name="async"/>,
	/// <see cref="DelimitedReader.ReadAsync"/>, and holds every row and the failure to it.
	/// </summary>
	static async Task Replay(string label, JsonElement vector, Func<Dialect, Column[], ValueTask<DelimitedReader>> open,
		int batchRows, bool async = false, bool notes = true)
	{
		var dialect = DialectOf(vector) with { Notes = notes };
		Column[] plan = [.. vector.GetProperty("plan").EnumerateArray().Select(ColumnOf)];
		var rows = vector.GetProperty("rows");

		// A failure in the header is the opening's: no reader, and no rows before it.
		DelimitedReader opened;
		try
		{
			opened = await open(dialect, plan);
		}
		catch (TabularException e)
		{
			rows.GetArrayLength().ShouldBe(0, label);
			AssertFailure(label, e, vector);
			AssertProblems(label, [], vector, notes);
			return;
		}
		await using var reader = opened;
		var header = vector.GetProperty("header");
		if (header.ValueKind == JsonValueKind.Null)
			reader.Header.ShouldBeNull(label);
		else
		{
			reader.Header.ShouldBe(header.EnumerateArray().Select(name => name.GetString()!), label);
			if (reader.Header!.Count > 0)
				reader.ColumnCount.ShouldBe(reader.Header.Count, label);
		}
		if (!reader.IsBound)
			BindHeaderFirst(label, reader.Header, reader.Plan, reader.IsBound, () => reader.Read(),
				resolved => reader.Bind(resolved, batchRows), plan);
		reader.Plan.ShouldBe(plan, label);

		var seen = 0;
		var problems = new List<string>();
		TabularException? failure = null;
		try
		{
			while ((async ? await reader.ReadAsync() : reader.Read()) is { } batch)
			{
				batch.Rows.ShouldBeGreaterThan(0, label);
				AssertRows(label, batch);
				AppendProblems(label, problems, batch, seen);
				for (var row = 0; row < batch.Rows; row++, seen++)
				{
					seen.ShouldBeLessThan(rows.GetArrayLength(), label);
					rows[seen].GetArrayLength().ShouldBe(plan.Length, label);
					for (var column = 0; column < plan.Length; column++)
						AssertCell($"{label}, row {seen}, column {column}", batch, column, row, rows[seen][column], notes);
				}
			}
		}
		catch (TabularException e)
		{
			failure = e;
			// A structure failure is final: the same one, again.
			Should.Throw<TabularException>(() => reader.Read()).ShouldBeSameAs(e, label);
		}
		seen.ShouldBe(rows.GetArrayLength(), label);
		AssertFailure(label, failure, vector);
		AssertProblems(label, problems, vector, notes);
	}

	[Fact]
	async Task Delimited_corpus()
	{
		var corpus = Corpus("delimited.json");
		corpus.Length.ShouldBeGreaterThanOrEqualTo(123);
		CorpusKeys.UnknownDelimited(corpus).ShouldBeEmpty();
		foreach (var vector in corpus)
		{
			var name = vector.GetProperty("name").GetString()!;
			var input = InputOf(vector);
			foreach (var batchRows in (int[])[1, 2, 1024])
			{
				await Replay($"{name} (memory, {batchRows} rows a batch)", vector,
					(dialect, plan) => new(new DelimitedReader(input.AsMemory(), dialect, plan, batchRows)), batchRows);
				await Replay($"{name} (memory, header first, {batchRows} rows a batch)", vector,
					(dialect, _) => new(new DelimitedReader(input.AsMemory(), dialect)), batchRows);
				await Replay($"{name} (memory, notes off, {batchRows} rows a batch)", vector,
					(dialect, plan) => new(new DelimitedReader(input.AsMemory(), dialect, plan, batchRows)), batchRows, notes: false);
				foreach (var bufferBytes in (int[])[1, 5, 64, DelimitedReader.DefaultBufferBytes])
				{
					await Replay($"{name} (stream through {bufferBytes} bytes, {batchRows} rows a batch)", vector,
						(dialect, plan) => new(new DelimitedReader(new MemoryStream(input), dialect, plan, batchRows, bufferBytes)), batchRows);
					await Replay($"{name} (stream through {bufferBytes} bytes, header first, {batchRows} rows a batch)", vector,
						(dialect, _) => new(new DelimitedReader(new MemoryStream(input), dialect, bufferBytes)), batchRows);
				}
			}
		}
	}

	/// <summary>
	/// The corpus read asynchronously from a stream that hands out 1 to 5 bytes a read, so
	/// that every batch takes many refills: opened with the plan and header-first, the header
	/// awaited too, and every batch the same as the synchronous read makes.
	/// </summary>
	[Fact]
	async Task Delimited_corpus_read_asynchronously_through_short_reads()
	{
		foreach (var vector in Corpus("delimited.json"))
		{
			var name = vector.GetProperty("name").GetString()!;
			var input = InputOf(vector);
			foreach (var batchRows in (int[])[1, 2, 1024])
				foreach (var bufferBytes in (int[])[1, 7, DelimitedReader.DefaultBufferBytes])
				{
					await Replay($"{name} (async, {bufferBytes}-byte buffer, {batchRows} rows a batch)", vector,
						(dialect, plan) => DelimitedReader.OpenAsync(new ShortReadStream(input), dialect, plan, batchRows, bufferBytes),
						batchRows, async: true);
					await Replay($"{name} (async, header first, {bufferBytes}-byte buffer, {batchRows} rows a batch)", vector,
						(dialect, _) => DelimitedReader.OpenAsync(new ShortReadStream(input), dialect, bufferBytes),
						batchRows, async: true);
				}
			// A reader of memory reads asynchronously too, completing synchronously.
			await Replay($"{name} (async from memory)", vector,
				(dialect, plan) => new(new DelimitedReader(input.AsMemory(), dialect, plan)),
				DelimitedReader.DefaultBatchRows, async: true);
		}
	}

	[Fact]
	void Every_delimited_corpus_key_is_read() =>
		CorpusKeys.AJunkKeyIsSeenEverywhere("delimited.json", CorpusKeys.UnknownDelimited);

	/// <summary>Holds a suggestion to what a corpus case says of it, its param put back together from the report.</summary>
	internal static void AssertSuggestion(string label, Suggestion found, JsonElement expected)
	{
		found.Truncated.ShouldBe(expected.GetProperty("truncated").GetBoolean(), label);
		var columns = expected.GetProperty("columns");
		found.Report.Count.ShouldBe(columns.GetArrayLength(), label);
		found.Plan.Count.ShouldBe(columns.GetArrayLength(), label);
		var declared = FormatOf(expected);
		for (var index = 0; index < found.Report.Count; index++)
		{
			var report = found.Report[index];
			var want = columns[index];
			var where = $"{label}, column {index}";
			var param = want.GetProperty("param").GetUInt32();
			var got = report.Param;
			if (report.Candidates.Count > 0)
			{
				got = 0;
				foreach (var candidate in report.Candidates)
					got |= candidate.Format is { } notation
						? notation.DecimalSeparator == "." ? 1u << 1 : 1u << 2
						: 1u << (int)Abi.Code(candidate.Order!.Value);
			}
			if (report.Status == SuggestStatus.Resolved && Column.IsNumeric(report.Door))
			{
				// The plan is in the one notation the param left, and comma groups is also what's used
				// with no separator to prove it. The export's param is held to in its own replay.
				var planned = declared ?? (param == 1u << 2 ? new NumFormat(',', '.', NumStyles.All) : NumFormat.Invariant);
				param = 0;
				report.Format.ShouldBe(planned, where);
				found.Plan[index].Format.ShouldBe(planned, where);
			}
			else
				report.Format.ShouldBeNull(where);
			DoorName(report.Door).ShouldBe(want.GetProperty("door").GetString(), where);
			got.ShouldBe(param, where);
			report.Status.ToString().ToLowerInvariant().ShouldBe(want.GetProperty("status").GetString(), where);
			report.Matched.ShouldBe(want.GetProperty("matched").GetInt32(), where);
			report.Sampled.ShouldBe(want.GetProperty("sampled").GetInt32(), where);
			report.Ordinal.ShouldBe(index, where);
			found.Plan[index].Ordinal.ShouldBe(index, where);
			found.Plan[index].Door.ShouldBe(report.Status == SuggestStatus.Resolved ? report.Door : Door.Text, where);
		}
	}

	/// <summary>What the corpus declares for a case's suggestion.</summary>
	internal static SuggestOptions SuggestOptionsOf(JsonElement suggest) =>
		new()
		{
			SampleRows = suggest.TryGetProperty("sample_rows", out var rows) ? rows.GetInt32() : 1000,
			Format = FormatOf(suggest),
		};

	/// <summary>
	/// Every case that says what to suggest: from memory and from a file; through the export, over the
	/// cells the case's own rows say; and a plan that reads back without refusing what it resolved.
	/// </summary>
	[Fact]
	void Every_suggestion_is_what_the_corpus_says()
	{
		var directory = Directory.CreateTempSubdirectory("hypertabular-suggest");
		try
		{
			var seen = 0;
			foreach (var vector in Corpus("delimited.json"))
			{
				if (!vector.TryGetProperty("suggest", out var suggest))
					continue;
				seen++;
				var name = vector.GetProperty("name").GetString()!;
				var input = InputOf(vector);
				var dialect = DialectOf(vector);
				var options = SuggestOptionsOf(suggest);
				var found = DelimitedReader.Suggest(input.AsMemory(), dialect, options);
				AssertSuggestion($"{name} (memory)", found, suggest);
				var path = Path.Combine(directory.FullName, $"suggest-{seen}.txt");
				File.WriteAllBytes(path, input);
				AssertSuggestion($"{name} (file)", DelimitedReader.Suggest(path, dialect, options), suggest);
				AssertTheExportSays(name, vector, suggest, options);

				// What's resolved takes every cell it saw, and refuses none.
				var sample = suggest.TryGetProperty("sample_rows", out var rows) ? rows.GetInt32() : int.MaxValue;
				using var reader = new DelimitedReader(input.AsMemory(), dialect, [.. found.Plan]);
				var read = 0;
				while (read < sample && reader.Read() is { } batch)
				{
					foreach (var problem in batch.Faults())
						if (problem.Row + read < sample && problem.PlanColumn is { } plan
							&& found.Report[plan] is { Status: SuggestStatus.Resolved, Door: not Door.Text })
							throw new ShouldAssertException($"{name}: read back refused {problem}");
					read += batch.Rows;
				}
			}
			seen.ShouldBeGreaterThanOrEqualTo(19);
		}
		finally
		{
			directory.Delete(recursive: true);
		}
	}

	/// <summary>The export, over a case's own rows: a cell with no text in the case is blank here, as a read's empty verdict is.</summary>
	static void AssertTheExportSays(string name, JsonElement vector, JsonElement suggest, SuggestOptions options)
	{
		var rows = vector.GetProperty("rows");
		var sample = suggest.TryGetProperty("sample_rows", out var limit) ? limit.GetInt32() : rows.GetArrayLength();
		var width = vector.GetProperty("plan").GetArrayLength();
		var text = new List<byte>();
		var cells = new List<Native.RawSpan>();
		for (var column = 0; column < width; column++)
			for (var row = 0; row < sample; row++)
			{
				var cell = rows[row][column];
				var bytes = cell.TryGetProperty("raw_hex", out var hex) ? Convert.FromHexString(hex.GetString()!)
					: cell.TryGetProperty("text", out var said) ? Encoding.UTF8.GetBytes(said.GetString()!)
					: [];
				cells.Add(new Native.RawSpan { Offset = (uint)text.Count, Len = (uint)bytes.Length });
				text.AddRange(bytes);
			}
		var formats = new RawNumFormat[width];
		if (options.Format is { } format)
			Array.Fill(formats, format.ToRaw());
		var found = new Native.RawSuggestion[width];
		var input = text.ToArray();
		var spans = cells.ToArray();
		fixed (byte* inputAt = input)
		fixed (Native.RawSpan* spansAt = spans)
		fixed (RawNumFormat* formatsAt = formats)
		fixed (Native.RawSuggestion* output = found)
			Native.hypertabular_suggest(inputAt, (nuint)input.Length, null, 0, spansAt, null, (nuint)sample, (nuint)width, formatsAt, output)
				.ShouldBe(Native.Ok, name);
		var columns = suggest.GetProperty("columns");
		for (var column = 0; column < width; column++)
		{
			var want = columns[column];
			var where = $"{name}: the export, column {column}";
			DoorName((Door)found[column].Door).ShouldBe(want.GetProperty("door").GetString(), where);
			found[column].Param.ShouldBe(want.GetProperty("param").GetUInt32(), where);
			((SuggestStatus)found[column].Status).ToString().ToLowerInvariant().ShouldBe(want.GetProperty("status").GetString(), where);
			found[column].Matched.ShouldBe(want.GetProperty("matched").GetUInt32(), where);
			found[column].Sampled.ShouldBe(want.GetProperty("sampled").GetUInt32(), where);
		}
	}

	/// <summary>Every case the corpus has: a suggestion fails where a read of the case fails, and nowhere else.</summary>
	[Fact]
	void A_suggestion_fails_only_where_a_read_of_the_case_fails()
	{
		var (planned, failed) = (0, 0);
		foreach (var vector in Corpus("delimited.json"))
			try
			{
				DelimitedReader.Suggest(InputOf(vector).AsMemory(), DialectOf(vector));
				planned++;
			}
			catch (TabularException e)
			{
				failed++;
				vector.TryGetProperty("failure", out _).ShouldBeTrue($"{vector.GetProperty("name")}: {e.Message}");
			}
		planned.ShouldBeGreaterThan(80);
		failed.ShouldBeGreaterThan(0);
	}
}

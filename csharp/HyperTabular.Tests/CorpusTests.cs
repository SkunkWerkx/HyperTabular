using System.Globalization;
using System.Text;
using System.Text.Json;
using HyperCast;

namespace HyperTabular.Tests;

/// <summary>
/// Replays the shared conformance corpus (<c>corpus/delimited.json</c> at the repository
/// root) — the same file the Rust binding replays — through this binding: from memory and
/// from a stream read through buffers too small for a record, in batches of one row and of
/// many. How the input is cut up is the binding's business and must not change the answer.
/// </summary>
public sealed class CorpusTests
{
	static readonly string _corpusDirectory = FindCorpusDirectory();

	static string FindCorpusDirectory()
	{
		for (var dir = new DirectoryInfo(AppContext.BaseDirectory); dir is not null; dir = dir.Parent)
		{
			var corpus = Path.Combine(dir.FullName, "corpus");
			if (File.Exists(Path.Combine(corpus, "delimited.json")))
				return corpus;
		}
		throw new DirectoryNotFoundException($"corpus directory not found above {AppContext.BaseDirectory}");
	}

	static JsonElement[] Corpus(string name)
	{
		using var document = JsonDocument.Parse(File.ReadAllText(Path.Combine(_corpusDirectory, name)));
		return [.. document.RootElement.EnumerateArray().Select(vector => vector.Clone())];
	}

	static NumFormat FormatOf(JsonElement entry)
	{
		if (!entry.TryGetProperty("format", out var format))
			return NumFormat.Invariant;
		var currency = format.TryGetProperty("currency", out var symbol) ? symbol.GetString()! : "";
		return new(
			format.GetProperty("decimal_sep").GetString()![0],
			format.GetProperty("group_sep").GetString()![0],
			(NumStyles)format.GetProperty("flags").GetUInt32(),
			currency);
	}

	static Column ColumnOf(JsonElement entry)
	{
		var ordinal = entry.GetProperty("ordinal").GetInt32();
		var format = FormatOf(entry);
		return entry.GetProperty("door").GetString() switch
		{
			"bool" => Column.Boolean(ordinal),
			"i8" => Column.SByte(ordinal, format),
			"i16" => Column.Int16(ordinal, format),
			"i32" => Column.Int32(ordinal, format),
			"i64" => Column.Int64(ordinal, format),
			"u8" => Column.Byte(ordinal, format),
			"u16" => Column.UInt16(ordinal, format),
			"u32" => Column.UInt32(ordinal, format),
			"u64" => Column.UInt64(ordinal, format),
			"f32" => Column.Single(ordinal, format),
			"f64" => Column.Double(ordinal, format),
			"decimal" => Column.Decimal(ordinal, format),
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

	static DateTimeOffset Instant(JsonElement cell) =>
		new(DateTime.UnixEpoch.Ticks
			+ cell.GetProperty("seconds").GetInt64() * TimeSpan.TicksPerSecond
			+ cell.GetProperty("nanos").GetInt32() / 100, TimeSpan.Zero);

	/// <summary>Holds one cell of the batch in hand to what the corpus says of it.</summary>
	static void AssertCell(string label, DelimitedReader reader, int column, int row, JsonElement expected)
	{
		var verdict = reader.Verdicts(column)[row];
		var door = reader.Column(column).Door;
		var expect = expected.GetProperty("expect").GetString();
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
				Encoding.UTF8.GetString(reader.Raw(column, row)).ShouldBe(expected.GetProperty("raw").GetString(), label);
			}
			if (door == Door.Text)
				reader.TryGetText(column, row, out _).ShouldBeFalse(label);
			else
				FaultOf(reader, column, row, door).ShouldBe(verdict.ToFault(), label);
			return;
		}

		verdict.IsOk.ShouldBeTrue(label);
		switch (door)
		{
			case Door.Boolean:
				Value(reader.Boolean(column, row)).ShouldBe(expected.GetProperty("value").GetBoolean(), label);
				break;
			case Door.SByte:
				Value(reader.SByte(column, row)).ShouldBe(expected.GetProperty("value").GetSByte(), label);
				break;
			case Door.Int16:
				Value(reader.Int16(column, row)).ShouldBe(expected.GetProperty("value").GetInt16(), label);
				break;
			case Door.Int32:
				Value(reader.Int32(column, row)).ShouldBe(expected.GetProperty("value").GetInt32(), label);
				break;
			case Door.Int64:
				Value(reader.Int64(column, row)).ShouldBe(expected.GetProperty("value").GetInt64(), label);
				break;
			case Door.Byte:
				Value(reader.Byte(column, row)).ShouldBe(expected.GetProperty("value").GetByte(), label);
				break;
			case Door.UInt16:
				Value(reader.UInt16(column, row)).ShouldBe(expected.GetProperty("value").GetUInt16(), label);
				break;
			case Door.UInt32:
				Value(reader.UInt32(column, row)).ShouldBe(expected.GetProperty("value").GetUInt32(), label);
				break;
			case Door.UInt64:
				Value(reader.UInt64(column, row)).ShouldBe(expected.GetProperty("value").GetUInt64(), label);
				break;
			case Door.Single:
				Value(reader.Single(column, row)).ShouldBe((float)expected.GetProperty("value").GetDouble(), label);
				break;
			case Door.Double:
				Value(reader.Double(column, row)).ShouldBe(expected.GetProperty("value").GetDouble(), label);
				break;
			case Door.Decimal:
				// The raw triple is the contract; decimal's own constructor is exact for it.
				var magnitude = UInt128.Parse(expected.GetProperty("magnitude").GetString()!, CultureInfo.InvariantCulture);
				var lo = (ulong)magnitude;
				Value(reader.Decimal(column, row)).ShouldBe(new decimal(
					(int)lo, (int)(lo >> 32), (int)(uint)(magnitude >> 64),
					expected.GetProperty("negative").GetBoolean(),
					expected.GetProperty("scale").GetByte()), label);
				break;
			case Door.Uuid:
				Value(reader.Uuid(column, row)).ShouldBe(Guid.ParseExact(expected.GetProperty("value").GetString()!, "N"), label);
				break;
			case Door.Timestamp or Door.Unix or Door.ExcelSerial:
				Value(reader.Timestamp(column, row)).ShouldBe(Instant(expected), label);
				break;
			case Door.Date or Door.DateOrdered:
				Value(reader.Date(column, row)).ShouldBe(new DateOnly(
					expected.GetProperty("year").GetInt32(),
					expected.GetProperty("month").GetInt32(),
					expected.GetProperty("day").GetInt32()), label);
				break;
			case Door.DateTime:
				Value(reader.DateTime(column, row)).ShouldBe(new DateTime(
					expected.GetProperty("year").GetInt32(),
					expected.GetProperty("month").GetInt32(),
					expected.GetProperty("day").GetInt32(), 0, 0, 0, DateTimeKind.Unspecified)
					.AddTicks((long)(expected.GetProperty("nanos_of_day").GetUInt64() / 100)), label);
				break;
			case Door.Time:
				Value(reader.Time(column, row)).ShouldBe(new TimeOnly((long)(expected.GetProperty("nanos").GetUInt64() / 100)), label);
				break;
			case Door.Duration:
				Value(reader.Duration(column, row)).ShouldBe(new TimeSpan(
					expected.GetProperty("seconds").GetInt64() * TimeSpan.TicksPerSecond
					+ expected.GetProperty("nanos").GetInt32() / 100), label);
				break;
			case Door.Text:
				reader.TryGetText(column, row, out var utf8).ShouldBeTrue(label);
				Encoding.UTF8.GetString(utf8).ShouldBe(expected.GetProperty("text").GetString(), label);
				reader.GetString(column, row).ShouldBe(expected.GetProperty("text").GetString(), label);
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

	/// <summary>The typed accessor's own fault for a cell that did not cast.</summary>
	static Fault FaultOf(DelimitedReader reader, int column, int row, Door door) =>
		door switch
		{
			Door.Boolean => Fault(reader.Boolean(column, row)),
			Door.SByte => Fault(reader.SByte(column, row)),
			Door.Int16 => Fault(reader.Int16(column, row)),
			Door.Int32 => Fault(reader.Int32(column, row)),
			Door.Int64 => Fault(reader.Int64(column, row)),
			Door.Byte => Fault(reader.Byte(column, row)),
			Door.UInt16 => Fault(reader.UInt16(column, row)),
			Door.UInt32 => Fault(reader.UInt32(column, row)),
			Door.UInt64 => Fault(reader.UInt64(column, row)),
			Door.Single => Fault(reader.Single(column, row)),
			Door.Double => Fault(reader.Double(column, row)),
			Door.Decimal => Fault(reader.Decimal(column, row)),
			Door.Uuid => Fault(reader.Uuid(column, row)),
			Door.Timestamp or Door.Unix or Door.ExcelSerial => Fault(reader.Timestamp(column, row)),
			Door.Date or Door.DateOrdered => Fault(reader.Date(column, row)),
			Door.DateTime => Fault(reader.DateTime(column, row)),
			Door.Time => Fault(reader.Time(column, row)),
			Door.Duration => Fault(reader.Duration(column, row)),
			_ => throw new InvalidOperationException($"no typed accessor for {door}"),
		};

	static void AssertFailure(string label, TabularException? actual, JsonElement vector)
	{
		if (!vector.TryGetProperty("failure", out var expected))
		{
			actual.ShouldBeNull(label);
			return;
		}
		actual.ShouldNotBeNull(label);
		var kind = expected.GetProperty("kind").GetString();
		actual.Failure.ShouldBe(kind == "column_count" ? TabularFailure.ColumnCount : TabularFailure.UnclosedQuote, label);
		actual.Record.ShouldBe(expected.GetProperty("record").GetInt64(), label);
		actual.Line.ShouldBe(expected.GetProperty("line").GetInt32(), label);
		actual.Byte.ShouldBe(expected.GetProperty("byte").GetInt64(), label);
		if (kind == "column_count")
			(actual.Expected, actual.Found).ShouldBe(
				(expected.GetProperty("expected").GetInt32(), expected.GetProperty("found").GetInt32()), label);
	}

	static void Replay(string label, JsonElement vector, Func<Dialect, Column[], DelimitedReader> open)
	{
		var settings = vector.GetProperty("dialect");
		var dialect = new Dialect(
			settings.GetProperty("separator").GetString()![0],
			settings.GetProperty("quoting").GetBoolean(),
			settings.GetProperty("has_header").GetBoolean(),
			settings.GetProperty("skip_blank_lines").GetBoolean());
		Column[] plan = [.. vector.GetProperty("plan").EnumerateArray().Select(ColumnOf)];
		var rows = vector.GetProperty("rows");

		using var reader = open(dialect, plan);
		var header = vector.GetProperty("header");
		if (header.ValueKind == JsonValueKind.Null)
			reader.Header.ShouldBeNull(label);
		else
			reader.Header.ShouldBe(header.EnumerateArray().Select(name => name.GetString()!), label);

		var seen = 0;
		TabularException? failure = null;
		try
		{
			while (reader.Read())
			{
				for (var row = 0; row < reader.Rows; row++, seen++)
				{
					seen.ShouldBeLessThan(rows.GetArrayLength(), label);
					for (var column = 0; column < plan.Length; column++)
						AssertCell($"{label}, row {seen}, column {column}", reader, column, row, rows[seen][column]);
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
	}

	[Fact]
	void Delimited_corpus()
	{
		var corpus = Corpus("delimited.json");
		corpus.Length.ShouldBeGreaterThanOrEqualTo(30);
		foreach (var vector in corpus)
		{
			var name = vector.GetProperty("name").GetString()!;
			var input = Encoding.UTF8.GetBytes(vector.GetProperty("input").GetString()!);
			foreach (var batchRows in (int[])[1, 2, 1024])
			{
				Replay($"{name} (memory, {batchRows} rows a batch)", vector,
					(dialect, plan) => new DelimitedReader(input.AsMemory(), dialect, plan, batchRows));
				foreach (var bufferBytes in (int[])[1, 5, 64, DelimitedReader.DefaultBufferBytes])
					Replay($"{name} (stream through {bufferBytes} bytes, {batchRows} rows a batch)", vector,
						(dialect, plan) => new DelimitedReader(new MemoryStream(input), dialect, plan, batchRows, bufferBytes));
			}
		}
	}
}

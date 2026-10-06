// Proves the binding under Native AOT for real: this program publishes with PublishAot and
// crosses every native entry point the binding declares — the version probe, the delimited
// header and fill through every door, the unescape behind a raw read, and a workbook opened,
// listed, loaded and read — against the real native library. Exit code 0 only if every
// cell lands as expected.

using System.Text;
using HyperCast;
using HyperTabular;

var failures = 0;

void Check<T>(string name, Verdict<T> verdict, T expected) where T : struct
{
	if (verdict.TryGetValue(out Success<T> success) && success.Value.Equals(expected))
	{
		Console.WriteLine($"ok   {name} = {success.Value}");
		return;
	}
	Console.WriteLine($"FAIL {name}: {verdict} (expected {expected})");
	failures++;
}

void Expect(string name, bool condition)
{
	Console.WriteLine($"{(condition ? "ok  " : "FAIL")} {name}");
	if (!condition)
		failures++;
}

Expect($"native library {Tabular.NativeVersion}", Tabular.IsAvailable);

var text = Encoding.UTF8.GetBytes(string.Join(',',
	"flag", "i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "f32", "f64", "money", "id",
	"at", "unix", "serial", "day", "us", "civil", "time", "span", "\"say \"\"hi\"\"\"") + "\n"
	+ string.Join(',',
	"yes", "-128", "32767", "(1,234)".Replace(",", ""), "9223372036854775807", "255", "65535",
	"4294967295", "18446744073709551615", "2.5", "25.5%", "1234.50",
	"6ba7b810-9dad-11d1-80b4-00c04fd430c8", "2026-01-02T15:04:05+05:00", "1700000000123",
	"45292.75", "2024-01-31", "1/7/2026", "1/7/2026 3:04 PM", "15:04:05.5", "PT1H30M",
	"\"a \"\"quoted\"\" cell\"") + "\n");

Column[] plan =
[
	Column.Boolean(0), Column.SByte(1), Column.Int16(2), Column.Int32(3), Column.Int64(4),
	Column.Byte(5), Column.UInt16(6), Column.UInt32(7), Column.UInt64(8), Column.Single(9),
	Column.Double(10), Column.Decimal(11), Column.Uuid(12), Column.Timestamp(13),
	Column.Unix(14, UnixPrecision.Milliseconds), Column.ExcelSerial(15, ExcelEpoch.Y1900),
	Column.Date(16), Column.Date(17, DateOrder.MonthDayYear),
	Column.DateTime(18, DateOrder.MonthDayYear), Column.Time(19), Column.Duration(20),
	Column.Text(21), Column.Int32(21),
];

using (var reader = new DelimitedReader(new MemoryStream(text), Dialect.Csv, plan, batchRows: 8, bufferBytes: 16))
{
	Expect("header", reader.Header is { Count: 22 } header && header[21] == "say \"hi\"");
	var batch = reader.Read();
	Expect("read", batch is { Rows: 1 });
	if (batch is not null)
	{
		Check("bool", batch.Get<bool>(0, 0), true);
		Check("i8", batch.Get<sbyte>(1, 0), (sbyte)-128);
		Check("i16", batch.Get<short>(2, 0), (short)32767);
		Check("i32", batch.Get<int>(3, 0), -1234);
		Check("i64", batch.Get<long>(4, 0), long.MaxValue);
		Check("u8", batch.Get<byte>(5, 0), (byte)255);
		Check("u16", batch.Get<ushort>(6, 0), (ushort)65535);
		Check("u32", batch.Get<uint>(7, 0), 4294967295u);
		Check("u64", batch.Get<ulong>(8, 0), ulong.MaxValue);
		Check("f32", batch.Get<float>(9, 0), 2.5f);
		Check("f64", batch.Get<double>(10, 0), 0.255);
		Check("decimal", batch.Get<decimal>(11, 0), 1234.5m);
		Check("uuid", batch.Get<Guid>(12, 0), new Guid("6ba7b810-9dad-11d1-80b4-00c04fd430c8"));
		Check("timestamp", batch.Get<DateTimeOffset>(13, 0), new DateTimeOffset(2026, 1, 2, 10, 4, 5, TimeSpan.Zero));
		Check("unix", batch.Get<DateTimeOffset>(14, 0), DateTimeOffset.FromUnixTimeMilliseconds(1700000000123));
		Check("excel serial", batch.Get<DateTimeOffset>(15, 0), new DateTimeOffset(2024, 1, 1, 18, 0, 0, TimeSpan.Zero));
		Check("date", batch.Get<DateOnly>(16, 0), new DateOnly(2024, 1, 31));
		Check("ordered date", batch.Get<DateOnly>(17, 0), new DateOnly(2026, 1, 7));
		Check("civil", batch.Get<DateTime>(18, 0), new DateTime(2026, 1, 7, 15, 4, 0, DateTimeKind.Unspecified));
		Check("time", batch.Get<TimeOnly>(19, 0), new TimeOnly(15, 4, 5, 500));
		Check("duration", batch.Get<TimeSpan>(20, 0), TimeSpan.FromMinutes(90));
		Expect("text", batch.GetString(21, 0) == "a \"quoted\" cell");
		Expect("span", batch.Values<int>(3)[0] == -1234 && batch.Verdicts(3)[0].IsOk);
		// A cell that does not cast: the union's fault case, and its text still to hand.
		Expect("fault", batch.Get<int>(22, 0) is Fault fault
			&& fault.Reason == CastFailure.Malformed
			&& Encoding.UTF8.GetString(batch.Raw(22, 0)) == "a \"quoted\" cell");
	}
	Expect("end", reader.Read() is null);
}

// A structural failure is an exception, after the intact rows.
try
{
	using var broken = new DelimitedReader("a,b\n1,2\n3\n"u8.ToArray().AsMemory(), Dialect.Csv, [Column.Int32(0)]);
	Expect("intact row", broken.Read() is { Rows: 1 });
	broken.Read();
	Expect("structure", false);
}
catch (TabularException e)
{
	Expect("structure", e is { Failure: TabularFailure.ColumnCount, Record: 2, Line: 3, Byte: 8, Expected: 2, Found: 1 });
}

// A workbook: opened, its sheets listed, its strings and styles loaded, a sheet positioned on,
// its header read and its rows filled — every workbook entry point, through the same batch.
using (var resource = typeof(Program).Assembly.GetManifestResourceStream("basic.xlsx")!)
{
	var bytes = new byte[resource.Length];
	resource.ReadExactly(bytes);
	using var book = new Workbook(bytes);
	Expect("workbook", book is { Format: WorkbookFormat.Xlsx, DateSystem: ExcelEpoch.Y1900, Sheets.Count: > 0 });
	var sheet = book.Sheet(0, SheetOptions.Default, [Column.Int64(0), Column.Text(1), Column.Decimal(2)]);
	Expect("sheet header", sheet.Header is { Count: > 2 });
	var rows = 0;
	while (sheet.Read() is { } batch)
		rows += batch.Rows;
	Expect($"sheet rows ({rows})", rows > 0);
	Expect("not a workbook", Throws(() => new Workbook("not a zip"u8.ToArray()), TabularFailure.NotAZip));
}

static bool Throws(Action action, TabularFailure failure)
{
	try
	{
		action();
		return false;
	}
	catch (TabularException e)
	{
		return e.Failure == failure;
	}
}

Console.WriteLine(failures == 0 ? "all ok" : $"{failures} failure(s)");
return failures == 0 ? 0 : 1;

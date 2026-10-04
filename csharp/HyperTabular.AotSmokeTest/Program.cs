// Proves the binding under Native AOT for real: this program publishes with PublishAot and
// crosses every native entry point the binding declares — the version probe, the header,
// the fill through every door, and the unescape behind a raw read — against the real
// native library. Exit code 0 only if every cell lands as expected.

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
	Expect("read", reader.Read() && reader.Rows == 1);
	Check("bool", reader.Boolean(0, 0), true);
	Check("i8", reader.SByte(1, 0), (sbyte)-128);
	Check("i16", reader.Int16(2, 0), (short)32767);
	Check("i32", reader.Int32(3, 0), -1234);
	Check("i64", reader.Int64(4, 0), long.MaxValue);
	Check("u8", reader.Byte(5, 0), (byte)255);
	Check("u16", reader.UInt16(6, 0), (ushort)65535);
	Check("u32", reader.UInt32(7, 0), 4294967295u);
	Check("u64", reader.UInt64(8, 0), ulong.MaxValue);
	Check("f32", reader.Single(9, 0), 2.5f);
	Check("f64", reader.Double(10, 0), 0.255);
	Check("decimal", reader.Decimal(11, 0), 1234.5m);
	Check("uuid", reader.Uuid(12, 0), new Guid("6ba7b810-9dad-11d1-80b4-00c04fd430c8"));
	Check("timestamp", reader.Timestamp(13, 0), new DateTimeOffset(2026, 1, 2, 10, 4, 5, TimeSpan.Zero));
	Check("unix", reader.Timestamp(14, 0), DateTimeOffset.FromUnixTimeMilliseconds(1700000000123));
	Check("excel serial", reader.Timestamp(15, 0), new DateTimeOffset(2024, 1, 1, 18, 0, 0, TimeSpan.Zero));
	Check("date", reader.Date(16, 0), new DateOnly(2024, 1, 31));
	Check("ordered date", reader.Date(17, 0), new DateOnly(2026, 1, 7));
	Check("civil", reader.DateTime(18, 0), new DateTime(2026, 1, 7, 15, 4, 0, DateTimeKind.Unspecified));
	Check("time", reader.Time(19, 0), new TimeOnly(15, 4, 5, 500));
	Check("duration", reader.Duration(20, 0), TimeSpan.FromMinutes(90));
	Expect("text", reader.GetString(21, 0) == "a \"quoted\" cell");
	Expect("span", reader.Values<int>(3)[0] == -1234 && reader.Verdicts(3)[0].IsOk);
	// A cell that does not cast: the union's fault case, and its text still to hand.
	Expect("fault", reader.Int32(22, 0) switch
	{
		Success<int> => false,
		Fault fault => fault.Reason == CastFailure.Malformed
			&& Encoding.UTF8.GetString(reader.Raw(22, 0)) == "a \"quoted\" cell",
	});
	Expect("end", !reader.Read());
}

// A structural failure is an exception, after the intact rows.
try
{
	using var broken = new DelimitedReader("a,b\n1,2\n3\n"u8.ToArray().AsMemory(), Dialect.Csv, [Column.Int32(0)]);
	Expect("intact row", broken.Read() && broken.Rows == 1);
	broken.Read();
	Expect("structure", false);
}
catch (TabularException e)
{
	Expect("structure", e is { Failure: TabularFailure.ColumnCount, Record: 2, Line: 3, Byte: 8, Expected: 2, Found: 1 });
}

Console.WriteLine(failures == 0 ? "all ok" : $"{failures} failure(s)");
return failures == 0 ? 0 : 1;
